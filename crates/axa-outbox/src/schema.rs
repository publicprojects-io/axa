//! The tables, their triggers, and the import from 0.1.0. Every statement here is safe
//! to repeat.
//!
//! Three tables are the record, and are only ever inserted into: `axa_outbox_events`,
//! `axa_outbox_attempts`, and `axa_outbox_payloads`, whose one allowed change is the
//! blanking of an erased payload. Two are working state, and change: `axa_outbox_owed`,
//! which the database derives from the attempts, and `axa_outbox_leases`, which says
//! which relays are still alive.

/// The record of each event, without its payload.
const EVENTS: &str = "CREATE TABLE IF NOT EXISTS axa_outbox_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_id UUID NOT NULL UNIQUE,
    topic TEXT NOT NULL,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
)";

/// Each payload, in its own table so that an erasure can blank it and touch nothing else.
/// `payload` is plain JSON, and `sealed` is the encrypted form. Both are null only once
/// the event is erased.
const PAYLOADS: &str = "CREATE TABLE IF NOT EXISTS axa_outbox_payloads (
    event_id UUID PRIMARY KEY REFERENCES axa_outbox_events (event_id),
    payload JSONB,
    sealed BYTEA,
    CHECK (payload IS NULL OR sealed IS NULL)
)";

/// Everything that happened to each event. An event's state is its latest attempt.
const ATTEMPTS: &str = "CREATE TABLE IF NOT EXISTS axa_outbox_attempts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_id UUID NOT NULL REFERENCES axa_outbox_events (event_id),
    outcome TEXT NOT NULL CHECK (outcome IN ('claimed', 'returned', 'delivered', 'refused',
        'rejected', 'failed', 'lost', 'abandoned', 'released', 'erased', 'imported')),
    relay UUID NOT NULL,
    claim UUID NOT NULL,
    reason TEXT CHECK (length(reason) <= 200 AND reason !~ '[[:cntrl:]]'),
    by TEXT CHECK (length(by) <= 200 AND by !~ '[[:cntrl:]]'),
    retry_at TIMESTAMPTZ,
    at TIMESTAMPTZ NOT NULL DEFAULT now()
)";

/// Working state: the events still owed, derived from the attempts by
/// `axa_outbox_follow`, so that finding work never reads what was delivered. A hold is
/// kept here too, as it writes no attempt.
const OWED: &str = "CREATE TABLE IF NOT EXISTS axa_outbox_owed (
    event_id UUID PRIMARY KEY REFERENCES axa_outbox_events (event_id),
    id BIGINT NOT NULL,
    topic TEXT NOT NULL,
    key TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'claimed', 'failed')),
    claim UUID,
    charges INTEGER NOT NULL DEFAULT 0,
    reason TEXT,
    retry_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    held_since TIMESTAMPTZ,
    recheck_at TIMESTAMPTZ
)";

/// Working state: when each live claim last reported in.
const LEASES: &str = "CREATE TABLE IF NOT EXISTS axa_outbox_leases (
    claim UUID PRIMARY KEY,
    relay UUID NOT NULL,
    heard_at TIMESTAMPTZ NOT NULL DEFAULT now()
)";

/// What a claim walks, oldest first.
const OWED_READY: &str = "CREATE INDEX IF NOT EXISTS axa_outbox_owed_ready
    ON axa_outbox_owed (id) WHERE state = 'pending'";
/// Finds an earlier event for a key. A failed one frees its key.
const OWED_LINE: &str = "CREATE INDEX IF NOT EXISTS axa_outbox_owed_line
    ON axa_outbox_owed (topic, key, id) WHERE state <> 'failed'";
/// What recovery looks at, on every claim.
const OWED_CLAIMED: &str = "CREATE INDEX IF NOT EXISTS axa_outbox_owed_claimed
    ON axa_outbox_owed (claim) WHERE state = 'claimed'";
/// An event's attempts in order, for matching an outcome to its claim.
const ATTEMPTS_EVENT: &str = "CREATE INDEX IF NOT EXISTS axa_outbox_attempts_event
    ON axa_outbox_attempts (event_id, id)";
/// Recent outcomes, for `stats`, without reading old ones.
const ATTEMPTS_RECENT: &str = "CREATE INDEX IF NOT EXISTS axa_outbox_attempts_recent
    ON axa_outbox_attempts (outcome, at) WHERE outcome IN ('delivered', 'refused')";

/// SHA-256 over an event's id, topic, kind, key, and stored payload. A text column
/// cannot hold a zero byte, so the zero bytes between fields cannot be confused with
/// them, and the payload is last.
const FINGERPRINT: &str = r"CREATE OR REPLACE FUNCTION axa_outbox_fingerprint(
    event_id UUID, topic TEXT, kind TEXT, key TEXT, payload JSONB, sealed BYTEA)
RETURNS TEXT LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT encode(sha256(
        uuid_send(event_id)
        || convert_to(topic, 'UTF8') || '\x00'::bytea
        || convert_to(kind, 'UTF8') || '\x00'::bytea
        || convert_to(key, 'UTF8') || '\x00'::bytea
        || CASE WHEN sealed IS NOT NULL THEN '\x73'::bytea || sealed
                ELSE '\x6a'::bytea || convert_to(payload::text, 'UTF8') END
    ), 'hex')
$$";

/// Refuses whatever fired it, whoever sent it.
const REFUSE: &str = "CREATE OR REPLACE FUNCTION axa_outbox_refuse() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% on % is refused: the outbox record is only ever added to',
        TG_OP, TG_TABLE_NAME;
END
$$";

/// Allows one change to a payload: blanking it, and nothing else, once the event has an
/// `erased` attempt.
const GUARD: &str = "CREATE OR REPLACE FUNCTION axa_outbox_payloads_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE'
        AND NEW.payload IS NULL AND NEW.sealed IS NULL
        AND (OLD.payload IS NOT NULL OR OLD.sealed IS NOT NULL)
        AND to_jsonb(NEW) - 'payload' - 'sealed' = to_jsonb(OLD) - 'payload' - 'sealed'
        AND EXISTS (
            SELECT 1 FROM axa_outbox_attempts erased
            WHERE erased.event_id = OLD.event_id AND erased.outcome = 'erased'
        )
    THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION '% on axa_outbox_payloads is refused: a payload is only blanked, once erased',
        TG_OP;
END
$$";

/// A new event is owed.
const OWE: &str = "CREATE OR REPLACE FUNCTION axa_outbox_owe() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO axa_outbox_owed (event_id, id, topic, key)
    VALUES (NEW.event_id, NEW.id, NEW.topic, NEW.key);
    RETURN NULL;
END
$$";

/// Keeps `axa_outbox_owed` in step with each new attempt, so that the working state is
/// always what the record says.
const FOLLOW: &str = "CREATE OR REPLACE FUNCTION axa_outbox_follow() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.outcome = 'claimed' THEN
        UPDATE axa_outbox_owed
        SET state = 'claimed', claim = NEW.claim, reason = NULL,
            held_since = NULL, recheck_at = NULL
        WHERE event_id = NEW.event_id;
    ELSIF NEW.outcome IN ('returned', 'imported') THEN
        UPDATE axa_outbox_owed
        SET state = 'pending', claim = NULL, reason = NEW.reason,
            retry_at = coalesce(NEW.retry_at, NEW.at)
        WHERE event_id = NEW.event_id;
    ELSIF NEW.outcome IN ('rejected', 'lost') THEN
        UPDATE axa_outbox_owed
        SET state = 'pending', claim = NULL, reason = NEW.reason, charges = charges + 1,
            retry_at = coalesce(NEW.retry_at, NEW.at)
        WHERE event_id = NEW.event_id;
    ELSIF NEW.outcome IN ('failed', 'abandoned') THEN
        UPDATE axa_outbox_owed
        SET state = 'failed', claim = NULL, reason = NEW.reason, charges = charges + 1,
            held_since = NULL, recheck_at = NULL
        WHERE event_id = NEW.event_id;
    ELSIF NEW.outcome = 'released' THEN
        UPDATE axa_outbox_owed
        SET state = 'pending', claim = NULL, reason = NULL, charges = 0, retry_at = NEW.at
        WHERE event_id = NEW.event_id;
    ELSE
        -- delivered, refused, and erased: nothing more is owed.
        DELETE FROM axa_outbox_owed WHERE event_id = NEW.event_id;
    END IF;
    RETURN NULL;
END
$$";

/// Keeps `axa_outbox_owed` what the record says. Only the record's own triggers, which
/// run one level down, may add, remove, or change its rows. From outside them an update
/// may change only a hold, `held_since` and `recheck_at`, and only to a time no later
/// than a day ahead, so that a hold set by hand cannot stop an event for good.
/// `pg_trigger_depth()` is 1 for a statement sent directly, and 2 for one sent by
/// `axa_outbox_owe` or `axa_outbox_follow`.
const OWED_GUARD: &str = "CREATE OR REPLACE FUNCTION axa_outbox_owed_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'TRUNCATE' THEN
        RAISE EXCEPTION 'TRUNCATE on axa_outbox_owed is refused: it is derived from the record';
    END IF;
    IF pg_trigger_depth() > 1 THEN
        RETURN coalesce(NEW, OLD);
    END IF;
    IF TG_OP = 'UPDATE'
        AND to_jsonb(NEW) - 'held_since' - 'recheck_at'
            = to_jsonb(OLD) - 'held_since' - 'recheck_at'
        AND (NEW.held_since IS NULL OR NEW.held_since <= now())
        AND (NEW.recheck_at IS NULL OR NEW.recheck_at <= now() + interval '1 day')
    THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION '% on axa_outbox_owed is refused: it changes only as the record does',
        TG_OP;
END
$$";

/// Keeps a lease from being dated ahead, which would keep a dead relay's claim alive
/// for good, or moved to another claim or relay. Deleting a lease is allowed: its claim
/// is then recovered, and the recovery is recorded.
const LEASES_GUARD: &str = "CREATE OR REPLACE FUNCTION axa_outbox_leases_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.heard_at > now() THEN
        RAISE EXCEPTION 'a lease in axa_outbox_leases cannot be heard from in the future';
    END IF;
    IF TG_OP = 'UPDATE' AND (NEW.claim <> OLD.claim OR NEW.relay <> OLD.relay) THEN
        RAISE EXCEPTION 'a lease in axa_outbox_leases cannot change its claim or relay';
    END IF;
    RETURN NEW;
END
$$";

const EVENTS_REFUSE: &str = "CREATE OR REPLACE TRIGGER axa_outbox_events_refuse
    BEFORE UPDATE OR DELETE ON axa_outbox_events
    FOR EACH ROW EXECUTE FUNCTION axa_outbox_refuse()";
const EVENTS_REFUSE_TRUNCATE: &str = "CREATE OR REPLACE TRIGGER axa_outbox_events_refuse_truncate
    BEFORE TRUNCATE ON axa_outbox_events
    FOR EACH STATEMENT EXECUTE FUNCTION axa_outbox_refuse()";
const EVENTS_OWE: &str = "CREATE OR REPLACE TRIGGER axa_outbox_events_owe
    AFTER INSERT ON axa_outbox_events
    FOR EACH ROW EXECUTE FUNCTION axa_outbox_owe()";
const ATTEMPTS_REFUSE: &str = "CREATE OR REPLACE TRIGGER axa_outbox_attempts_refuse
    BEFORE UPDATE OR DELETE ON axa_outbox_attempts
    FOR EACH ROW EXECUTE FUNCTION axa_outbox_refuse()";
const ATTEMPTS_REFUSE_TRUNCATE: &str =
    "CREATE OR REPLACE TRIGGER axa_outbox_attempts_refuse_truncate
    BEFORE TRUNCATE ON axa_outbox_attempts
    FOR EACH STATEMENT EXECUTE FUNCTION axa_outbox_refuse()";
const ATTEMPTS_FOLLOW: &str = "CREATE OR REPLACE TRIGGER axa_outbox_attempts_follow
    AFTER INSERT ON axa_outbox_attempts
    FOR EACH ROW EXECUTE FUNCTION axa_outbox_follow()";
const PAYLOADS_GUARD: &str = "CREATE OR REPLACE TRIGGER axa_outbox_payloads_guard
    BEFORE UPDATE OR DELETE ON axa_outbox_payloads
    FOR EACH ROW EXECUTE FUNCTION axa_outbox_payloads_guard()";
const PAYLOADS_REFUSE_TRUNCATE: &str =
    "CREATE OR REPLACE TRIGGER axa_outbox_payloads_refuse_truncate
    BEFORE TRUNCATE ON axa_outbox_payloads
    FOR EACH STATEMENT EXECUTE FUNCTION axa_outbox_refuse()";

const OWED_GUARD_ROWS: &str = "CREATE OR REPLACE TRIGGER axa_outbox_owed_guard
    BEFORE INSERT OR UPDATE OR DELETE ON axa_outbox_owed
    FOR EACH ROW EXECUTE FUNCTION axa_outbox_owed_guard()";
const OWED_GUARD_TRUNCATE: &str = "CREATE OR REPLACE TRIGGER axa_outbox_owed_refuse_truncate
    BEFORE TRUNCATE ON axa_outbox_owed
    FOR EACH STATEMENT EXECUTE FUNCTION axa_outbox_owed_guard()";
const LEASES_GUARD_ROWS: &str = "CREATE OR REPLACE TRIGGER axa_outbox_leases_guard
    BEFORE INSERT OR UPDATE ON axa_outbox_leases
    FOR EACH ROW EXECUTE FUNCTION axa_outbox_leases_guard()";

// `ENABLE ALWAYS` keeps the refusals on when `session_replication_role` is `replica`,
// which otherwise turns triggers off.
const ALWAYS_EVENTS: &str = "ALTER TABLE axa_outbox_events
    ENABLE ALWAYS TRIGGER axa_outbox_events_refuse,
    ENABLE ALWAYS TRIGGER axa_outbox_events_refuse_truncate";
const ALWAYS_ATTEMPTS: &str = "ALTER TABLE axa_outbox_attempts
    ENABLE ALWAYS TRIGGER axa_outbox_attempts_refuse,
    ENABLE ALWAYS TRIGGER axa_outbox_attempts_refuse_truncate";
const ALWAYS_PAYLOADS: &str = "ALTER TABLE axa_outbox_payloads
    ENABLE ALWAYS TRIGGER axa_outbox_payloads_guard,
    ENABLE ALWAYS TRIGGER axa_outbox_payloads_refuse_truncate";
const ALWAYS_OWED: &str = "ALTER TABLE axa_outbox_owed
    ENABLE ALWAYS TRIGGER axa_outbox_owed_guard,
    ENABLE ALWAYS TRIGGER axa_outbox_owed_refuse_truncate";
const ALWAYS_LEASES: &str =
    "ALTER TABLE axa_outbox_leases ENABLE ALWAYS TRIGGER axa_outbox_leases_guard";

/// The statements that make the outbox's tables, indexes, functions, and triggers, in
/// order, each safe to repeat. A service whose role may not change the schema runs these
/// in a migration, and calls [`Outbox::connect_existing`](crate::Outbox::connect_existing).
/// They need Postgres 14 or later.
pub const SETUP_STATEMENTS: [&str; 33] = [
    EVENTS,
    PAYLOADS,
    ATTEMPTS,
    OWED,
    LEASES,
    OWED_READY,
    OWED_LINE,
    OWED_CLAIMED,
    ATTEMPTS_EVENT,
    ATTEMPTS_RECENT,
    FINGERPRINT,
    REFUSE,
    GUARD,
    OWED_GUARD,
    LEASES_GUARD,
    OWE,
    FOLLOW,
    EVENTS_REFUSE,
    EVENTS_REFUSE_TRUNCATE,
    EVENTS_OWE,
    ATTEMPTS_REFUSE,
    ATTEMPTS_REFUSE_TRUNCATE,
    ATTEMPTS_FOLLOW,
    PAYLOADS_GUARD,
    PAYLOADS_REFUSE_TRUNCATE,
    OWED_GUARD_ROWS,
    OWED_GUARD_TRUNCATE,
    LEASES_GUARD_ROWS,
    ALWAYS_EVENTS,
    ALWAYS_ATTEMPTS,
    ALWAYS_PAYLOADS,
    ALWAYS_OWED,
    ALWAYS_LEASES,
];

/// Triggers [`Outbox::connect_existing`](crate::Outbox::connect_existing) requires,
/// with whether each must fire always, by table.
pub(crate) const TRIGGERS: [(&str, &str, bool); 11] = [
    ("axa_outbox_events", "axa_outbox_events_refuse", true),
    (
        "axa_outbox_events",
        "axa_outbox_events_refuse_truncate",
        true,
    ),
    ("axa_outbox_events", "axa_outbox_events_owe", false),
    ("axa_outbox_attempts", "axa_outbox_attempts_refuse", true),
    (
        "axa_outbox_attempts",
        "axa_outbox_attempts_refuse_truncate",
        true,
    ),
    ("axa_outbox_attempts", "axa_outbox_attempts_follow", false),
    ("axa_outbox_payloads", "axa_outbox_payloads_guard", true),
    (
        "axa_outbox_payloads",
        "axa_outbox_payloads_refuse_truncate",
        true,
    ),
    ("axa_outbox_owed", "axa_outbox_owed_guard", true),
    ("axa_outbox_owed", "axa_outbox_owed_refuse_truncate", true),
    ("axa_outbox_leases", "axa_outbox_leases_guard", true),
];

/// Indexes [`Outbox::connect_existing`](crate::Outbox::connect_existing) requires.
pub(crate) const INDEXES: [&str; 5] = [
    "axa_outbox_owed_ready",
    "axa_outbox_owed_line",
    "axa_outbox_owed_claimed",
    "axa_outbox_attempts_event",
    "axa_outbox_attempts_recent",
];

const IMPORT_EVENTS: &str = "INSERT INTO axa_outbox_events
        (event_id, topic, kind, key, fingerprint, created_at)
    SELECT old.event_id, old.topic, old.kind, old.key,
        axa_outbox_fingerprint(old.event_id, old.topic, old.kind, old.key, old.payload, NULL),
        old.created_at
    FROM axa_outbox old
    WHERE NOT EXISTS (SELECT 1 FROM axa_outbox_events e WHERE e.event_id = old.event_id)
    ORDER BY old.id";
const IMPORT_PAYLOADS: &str = "INSERT INTO axa_outbox_payloads (event_id, payload)
    SELECT old.event_id, old.payload FROM axa_outbox old
    WHERE NOT EXISTS (SELECT 1 FROM axa_outbox_payloads p WHERE p.event_id = old.event_id)
    ORDER BY old.id";
const IMPORT_ATTEMPTS: &str = "INSERT INTO axa_outbox_attempts
        (event_id, outcome, relay, claim, reason, by, retry_at, at)
    SELECT old.event_id,
        CASE old.status WHEN 'sent' THEN 'delivered' WHEN 'failed' THEN 'failed'
            ELSE 'imported' END,
        '00000000-0000-0000-0000-000000000000', '00000000-0000-0000-0000-000000000000',
        'imported: ' || CASE old.status WHEN 'claimed' THEN 'pending' ELSE old.status END
            || ', ' || old.attempts || ' attempts',
        'axa-outbox 0.1.0',
        CASE WHEN old.status IN ('pending', 'claimed') THEN old.run_at END,
        coalesce(old.sent_at, old.updated_at)
    FROM axa_outbox old
    WHERE NOT EXISTS (SELECT 1 FROM axa_outbox_attempts a WHERE a.event_id = old.event_id)
    ORDER BY old.id";

/// The statements that copy each row of the 0.1.0 table `axa_outbox` into the record, in
/// order, each safe to repeat. Run them in a migration after [`SETUP_STATEMENTS`], with
/// every relay stopped. Each row keeps its id's order, its payload, and its time, and
/// gets one attempt for its state: `delivered` for a sent row, `failed` for a failed
/// one, and `imported` for one still owed, which a relay then sends. Its count of
/// attempts goes in the reason, and its error text does not, as it is not a code.
/// `axa_outbox` is left as it was, for a later migration of the service's own to drop.
pub const IMPORT_STATEMENTS: [&str; 3] = [IMPORT_EVENTS, IMPORT_PAYLOADS, IMPORT_ATTEMPTS];

const REBUILD_OFF: &str = "ALTER TABLE axa_outbox_owed DISABLE TRIGGER axa_outbox_owed_guard";
const REBUILD_EMPTY: &str = "DELETE FROM axa_outbox_owed";
/// What `axa_outbox_owe` and `axa_outbox_follow` would have made: an event is owed
/// unless it was ever delivered, refused, or erased, and its latest attempt says the
/// rest. Holds are not kept, so a held event is decided again.
const REBUILD_FILL: &str = "INSERT INTO axa_outbox_owed
        (event_id, id, topic, key, state, claim, charges, reason, retry_at)
    SELECT e.event_id, e.id, e.topic, e.key,
        CASE WHEN latest.outcome = 'claimed' THEN 'claimed'
             WHEN latest.outcome IN ('failed', 'abandoned') THEN 'failed'
             ELSE 'pending' END,
        CASE WHEN latest.outcome = 'claimed' THEN latest.claim END,
        (SELECT count(*) FROM axa_outbox_attempts charged
         WHERE charged.event_id = e.event_id
           AND charged.outcome IN ('rejected', 'lost', 'failed', 'abandoned')
           AND charged.id > coalesce((
               SELECT max(released.id) FROM axa_outbox_attempts released
               WHERE released.event_id = e.event_id AND released.outcome = 'released'
           ), 0))::integer,
        CASE WHEN latest.outcome IN ('claimed', 'released') THEN NULL ELSE latest.reason END,
        coalesce(latest.retry_at, latest.at, e.created_at)
    FROM axa_outbox_events e
    LEFT JOIN LATERAL (
        SELECT a.outcome, a.claim, a.reason, a.retry_at, a.at FROM axa_outbox_attempts a
        WHERE a.event_id = e.event_id ORDER BY a.id DESC LIMIT 1
    ) latest ON true
    WHERE NOT EXISTS (
        SELECT 1 FROM axa_outbox_attempts ended
        WHERE ended.event_id = e.event_id
          AND ended.outcome IN ('delivered', 'refused', 'erased')
    )";
const REBUILD_ON: &str = "ALTER TABLE axa_outbox_owed ENABLE ALWAYS TRIGGER axa_outbox_owed_guard";

/// The statements that make `axa_outbox_owed` again from the record, for when it no
/// longer matches, as after someone with the schema changed it. Run them in order, in
/// one transaction, as the tables' owner, with every relay stopped. They read the whole
/// record, and touch nothing in it. Holds are not kept, so a held event is decided again.
pub const REBUILD_OWED_STATEMENTS: [&str; 4] =
    [REBUILD_OFF, REBUILD_EMPTY, REBUILD_FILL, REBUILD_ON];
