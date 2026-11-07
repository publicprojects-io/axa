//! The relay: claiming, deciding, publishing, and recording.

use std::{fmt, future::Future, time::Duration};

use serde_json::Value;
use sqlx::{
    Acquire, PgConnection, PgPool, Postgres, Row, Transaction,
    postgres::{PgPoolOptions, PgRow},
};
use tokio::{
    sync::watch,
    time::{Instant, Interval, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{
    Attempt, Claimed, Decision, Failure, OutboxOptions, Outcome, SETUP_STATEMENTS, Stats, code,
    schema::{INDEXES, TRIGGERS},
};
#[cfg(feature = "encryption")]
use crate::{Keys, seal::Unopened};

/// Advisory lock key for table setup: `axa_o` as bytes.
const SETUP_LOCK: i64 = 0x0061_7861_5F6F;

/// Connections one [`Outbox`] holds: it works in sequence, and a heartbeat can
/// overlap a publish. Each `connect` gets its own, so a relay cannot take connections
/// from requests.
const CONNECTIONS: u32 = 2;

/// The longest wait before an event is tried again.
const MAX_BACKOFF: Duration = Duration::from_hours(1);

/// The longest wait while the broker is down, which is also how long a relay can lag
/// once it is back.
const MAX_PAUSE: Duration = Duration::from_mins(1);

/// Each column a relay reads, so that a table out of date fails at boot.
const PROBES: [&str; 5] = [
    "SELECT id, event_id, topic, kind, key, fingerprint, created_at FROM axa_outbox_events LIMIT 0",
    "SELECT event_id, payload, sealed FROM axa_outbox_payloads LIMIT 0",
    "SELECT id, event_id, outcome, relay, claim, reason, by, retry_at, at
     FROM axa_outbox_attempts LIMIT 0",
    "SELECT event_id, id, topic, key, state, claim, charges, reason, retry_at, held_since,
            recheck_at
     FROM axa_outbox_owed LIMIT 0",
    "SELECT claim, relay, heard_at FROM axa_outbox_leases LIMIT 0",
];

const MISSING_TRIGGERS: &str = "SELECT want.name
    FROM unnest($1::text[], $2::text[], $3::bool[]) AS want(tbl, name, always)
    WHERE NOT EXISTS (
        SELECT 1 FROM pg_trigger t
        WHERE t.tgrelid = to_regclass(want.tbl) AND t.tgname = want.name
          AND (t.tgenabled::text = 'A' OR (NOT want.always AND t.tgenabled::text = 'O'))
    )";
const MISSING_INDEXES: &str =
    "SELECT name FROM unnest($1::text[]) AS name WHERE to_regclass(name) IS NULL";
const MISSING_FINGERPRINT: &str = "SELECT to_regprocedure(
    'axa_outbox_fingerprint(uuid, text, text, text, jsonb, bytea)') IS NULL";

/// The events a claim may take, oldest first, locked for this claim alone. An earlier
/// event still owed for the same topic and key holds one back, unless its topic has no
/// order. `FOR UPDATE SKIP LOCKED` is what stops two relays taking one event, and the
/// state is checked again once the lock is had, because the claim changes the row.
const CANDIDATES: &str = "SELECT o.event_id, o.id, o.charges, e.topic, e.kind, e.key,
        p.payload, p.sealed,
        e.fingerprint IS NOT DISTINCT FROM axa_outbox_fingerprint(
            e.event_id, e.topic, e.kind, e.key, p.payload, p.sealed) AS intact
    FROM axa_outbox_owed o
    JOIN axa_outbox_events e ON e.event_id = o.event_id
    LEFT JOIN axa_outbox_payloads p ON p.event_id = o.event_id
    WHERE o.state = 'pending' AND o.retry_at <= now()
      AND (o.recheck_at IS NULL OR o.recheck_at <= now())
      AND (o.topic = ANY($2) OR NOT EXISTS (
          SELECT 1 FROM axa_outbox_owed earlier
          WHERE earlier.topic = o.topic AND earlier.key = o.key
            AND earlier.id < o.id AND earlier.state <> 'failed'
      ))
    ORDER BY o.id
    LIMIT $1
    FOR UPDATE OF o SKIP LOCKED";
const CLAIMED: &str = "INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim)
    SELECT event_id, 'claimed', $2, $3
    FROM unnest($1::uuid[]) WITH ORDINALITY AS taken(event_id, place)
    ORDER BY place";
const LEASE: &str = "INSERT INTO axa_outbox_leases (claim, relay) VALUES ($1, $2)";
/// An outcome reached in the claim itself, on a row the claim has locked.
const DECIDED: &str = "INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim, reason)
    VALUES ($1, $2, $3, $4, $5)
    RETURNING id";
const HOLD: &str = "UPDATE axa_outbox_owed
    SET held_since = coalesce(held_since, now()), recheck_at = now() + make_interval(secs => $2)
    WHERE event_id = $1";
const DEFER: &str = "UPDATE axa_outbox_owed
    SET recheck_at = now() + make_interval(secs => $2)
    WHERE event_id = $1";
/// Ends a live claim, only if it is still the event's latest: a refusal by the service,
/// or a failure charged an attempt, which is final once none are left.
const ENDED: &str = "WITH current AS (
        SELECT event_id, charges FROM axa_outbox_owed
        WHERE event_id = $1 AND state = 'claimed' AND claim = $3
        FOR UPDATE
    )
    INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim, reason, retry_at)
    SELECT event_id,
        CASE WHEN $4 THEN 'refused' WHEN charges + 1 >= $5 THEN 'failed' ELSE 'rejected' END,
        $2, $3, $6,
        CASE WHEN NOT $4 AND charges + 1 < $5 THEN now() + make_interval(secs => $7) END
    FROM current
    RETURNING id, outcome";
const LOCK_OWED: &str = "SELECT 1 FROM axa_outbox_owed WHERE event_id = $1 FOR UPDATE";
/// A success from a claim of this relay's that has not ended. The claim need not be
/// the latest: a late success is kept as its own attempt, as the broker has the event
/// whoever says so, and believing it spares a duplicate.
const DELIVERED: &str = "INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim)
    SELECT $1, 'delivered', $2, $3
    WHERE EXISTS (
        SELECT 1 FROM axa_outbox_attempts took
        WHERE took.event_id = $1 AND took.claim = $3 AND took.relay = $2
          AND took.outcome = 'claimed'
    )
    AND NOT EXISTS (
        SELECT 1 FROM axa_outbox_attempts ended
        WHERE ended.event_id = $1 AND ended.claim = $3
          AND ended.outcome NOT IN ('claimed', 'lost', 'abandoned')
    )
    RETURNING id";
/// Gives back events a live claim holds, charging nothing.
const RETURNED: &str = "WITH given AS (
        SELECT * FROM unnest($1::uuid[], $2::uuid[]) AS given(event_id, claim)
    ), current AS (
        SELECT o.event_id, o.claim FROM axa_outbox_owed o
        JOIN given ON given.event_id = o.event_id AND given.claim = o.claim
        WHERE o.state = 'claimed'
        FOR UPDATE OF o
    )
    INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim, reason, retry_at)
    SELECT event_id, 'returned', $3, claim, $4, now() + make_interval(secs => $5)
    FROM current";
const HEARTBEAT: &str = "UPDATE axa_outbox_leases SET heard_at = now() WHERE claim = ANY($1)";
const END_LEASE: &str = "DELETE FROM axa_outbox_leases WHERE claim = $1";
/// Ends every claim not heard from: `lost` with attempts left, and `abandoned` without.
const RECOVER: &str = "WITH stale AS (
        SELECT o.event_id, o.claim, o.charges FROM axa_outbox_owed o
        WHERE o.state = 'claimed' AND NOT EXISTS (
            SELECT 1 FROM axa_outbox_leases l
            WHERE l.claim = o.claim AND l.heard_at >= now() - make_interval(mins => $1)
        )
        FOR UPDATE OF o SKIP LOCKED
    ), ended AS (
        INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim)
        SELECT event_id, CASE WHEN charges + 1 >= $2 THEN 'abandoned' ELSE 'lost' END, $3, claim
        FROM stale
        RETURNING id, event_id, outcome, claim
    )
    SELECT ended.id, ended.event_id, ended.outcome, ended.claim, e.topic, e.kind, e.key
    FROM ended JOIN axa_outbox_events e ON e.event_id = ended.event_id
    ORDER BY ended.id";
const DROP_LEASES: &str = "DELETE FROM axa_outbox_leases l
    WHERE l.heard_at < now() - make_interval(mins => $1)
      AND NOT EXISTS (
          SELECT 1 FROM axa_outbox_owed o WHERE o.claim = l.claim AND o.state = 'claimed'
      )";
/// Reads only what is owed, and the last hour of outcomes.
const STATS: &str = "SELECT
        count(*) FILTER (WHERE o.state = 'pending' AND o.held_since IS NULL) AS pending,
        count(*) FILTER (WHERE o.state = 'claimed') AS claimed,
        count(*) FILTER (WHERE o.state = 'pending' AND o.held_since IS NOT NULL) AS held,
        count(*) FILTER (WHERE o.state = 'failed') AS failed,
        count(*) FILTER (WHERE o.state = 'failed' AND o.reason = 'tampered') AS tampered,
        EXTRACT(EPOCH FROM now() - min(e.created_at)
            FILTER (WHERE o.state <> 'failed' AND o.held_since IS NULL))::float8 AS oldest,
        EXTRACT(EPOCH FROM now() - min(o.held_since)
            FILTER (WHERE o.state = 'pending'))::float8 AS oldest_held,
        EXTRACT(EPOCH FROM now() - min(e.created_at)
            FILTER (WHERE o.state = 'failed'))::float8 AS oldest_failed,
        (SELECT count(*) FROM axa_outbox_attempts a
         WHERE a.outcome = 'delivered' AND a.at > now() - interval '1 hour') AS delivered,
        (SELECT count(*) FROM axa_outbox_attempts a
         WHERE a.outcome = 'refused' AND a.at > now() - interval '1 hour') AS refused
    FROM axa_outbox_owed o JOIN axa_outbox_events e ON e.event_id = o.event_id";

/// How long to wait before the try after `attempts`, doubling each time up to `limit`.
pub(crate) fn backoff(base: Duration, attempts: i32, limit: Duration) -> Duration {
    let doublings = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(16);
    base.saturating_mul(1 << doublings).min(limit)
}

/// How one round of a relay ended.
enum Round {
    /// It claimed or decided something.
    Worked,
    /// There was nothing to claim.
    Empty,
    /// The broker is not taking events, or no key here opens what is owed.
    Stalled,
}

/// How one publish ended.
enum Published {
    Sent,
    Failed(Failure),
    TimedOut,
}

/// What one claim took.
#[derive(Default)]
struct Taken {
    batch: Vec<Claimed>,
    /// Events refused, held, or failed in the claim itself.
    decided: usize,
    /// Sealed events no key here opens, left as they were.
    unopened: usize,
}

/// What the claim did with one event.
enum Decided {
    Deliver,
    Refused,
    Held,
}

/// Why a payload could not be read for publishing.
enum Unread {
    /// No key here opens it. Nothing is written, and the event waits.
    NoKey,
    /// It does not match what was written.
    Tampered,
}

/// A connection to the outbox, and the relay that drains it. Clones share a pool and
/// are the same relay.
#[derive(Clone)]
pub struct Outbox {
    pool: PgPool,
    options: OutboxOptions,
    relay: Uuid,
    unordered: Vec<String>,
    #[cfg(feature = "encryption")]
    keys: Option<Keys>,
}

impl fmt::Debug for Outbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outbox")
            .field("relay", &self.relay)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Outbox {
    /// Open the pool, without touching the schema.
    async fn open(uri: &str, options: OutboxOptions) -> Result<Self, sqlx::Error> {
        options.validate()?;
        let pool = PgPoolOptions::new()
            .max_connections(CONNECTIONS)
            .connect(uri)
            .await?;
        Ok(Self {
            pool,
            unordered: options.unordered(),
            options,
            relay: Uuid::new_v4(),
            #[cfg(feature = "encryption")]
            keys: None,
        })
    }

    /// Connect to outbox tables that a migration made, and change no schema, for a
    /// database role that may not. A migration runs [`SETUP_STATEMENTS`].
    ///
    /// # Errors
    ///
    /// Fails on options that cannot work, when the database cannot be reached, or when
    /// a table, an index, or a trigger is missing or turned off, so a service finds
    /// out at boot. There is no way to run without the triggers.
    pub async fn connect_existing(uri: &str, options: OutboxOptions) -> Result<Self, sqlx::Error> {
        let outbox = Self::open(uri, options).await?;
        outbox.check_schema().await?;
        Ok(outbox)
    }

    /// Connect and create the outbox tables if needed, which runs DDL at boot. Use
    /// [`Outbox::connect_existing`] where a service's role may not.
    ///
    /// # Errors
    ///
    /// Fails on options that cannot work, when the database cannot be reached, or when
    /// the tables cannot be created.
    pub async fn connect(uri: &str, options: OutboxOptions) -> Result<Self, sqlx::Error> {
        let outbox = Self::open(uri, options).await?;
        // `CREATE ... IF NOT EXISTS` is not safe against itself, so setup runs one at
        // a time. The lock ends with the transaction.
        let mut setup = outbox.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(SETUP_LOCK)
            .execute(&mut *setup)
            .await?;
        for statement in SETUP_STATEMENTS {
            sqlx::query(statement).execute(&mut *setup).await?;
        }
        setup.commit().await?;
        Ok(outbox)
    }

    /// Refuse to run on tables without their triggers, indexes, or fingerprint.
    async fn check_schema(&self) -> Result<(), sqlx::Error> {
        for probe in PROBES {
            sqlx::query(probe).execute(&self.pool).await?;
        }
        let tables: Vec<&str> = TRIGGERS.iter().map(|(table, _, _)| *table).collect();
        let names: Vec<&str> = TRIGGERS.iter().map(|(_, name, _)| *name).collect();
        let always: Vec<bool> = TRIGGERS.iter().map(|(_, _, always)| *always).collect();
        let mut missing: Vec<String> = sqlx::query_scalar(MISSING_TRIGGERS)
            .bind(tables)
            .bind(names)
            .bind(always)
            .fetch_all(&self.pool)
            .await?;
        missing.extend(
            sqlx::query_scalar::<_, String>(MISSING_INDEXES)
                .bind(INDEXES.to_vec())
                .fetch_all(&self.pool)
                .await?,
        );
        if sqlx::query_scalar::<_, bool>(MISSING_FINGERPRINT)
            .fetch_one(&self.pool)
            .await?
        {
            missing.push("axa_outbox_fingerprint".to_string());
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(sqlx::Error::Configuration(
                format!(
                    "the outbox is missing, or has turned off, {}: run SETUP_STATEMENTS",
                    missing.join(", ")
                )
                .into(),
            ))
        }
    }

    /// Open sealed payloads with `keys`, the same keys the service's [`Writer`] seals
    /// with. A sealed payload that no key here opens is left as it was, and the relay
    /// logs that it is stuck.
    ///
    /// [`Writer`]: crate::Writer
    #[cfg(feature = "encryption")]
    #[must_use]
    pub fn opening(mut self, keys: Keys) -> Self {
        self.keys = Some(keys);
        self
    }

    /// The id this relay writes on its attempts.
    #[must_use]
    pub fn relay(&self) -> Uuid {
        self.relay
    }

    /// Check the database connection.
    ///
    /// # Errors
    ///
    /// Fails when the database does not answer.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Take up to [`OutboxOptions::batch_size`] events, oldest first, for this caller
    /// alone, having first recovered what a dead relay left. Each is decided with
    /// `decide`, in the claim's transaction: a refusal is recorded there, with
    /// `record`, and a hold writes nothing. What comes back is to be delivered, and
    /// each must then be passed to [`Outbox::delivered`], [`Outbox::failed`],
    /// [`Outbox::refused`], or [`Outbox::returned`], within
    /// [`OutboxOptions::stale_minutes`], or with [`Outbox::heartbeat`] along the way.
    ///
    /// # Errors
    ///
    /// Fails on a database error, or when `record` fails for a recovered claim. A
    /// decision or record that fails for one event leaves that event as it was, to be
    /// decided again after [`OutboxOptions::hold_interval`], and is logged.
    pub async fn claim<D, R>(&self, decide: D, record: R) -> Result<Vec<Claimed>, sqlx::Error>
    where
        D: AsyncFn(&Claimed, &mut PgConnection) -> Result<Decision, sqlx::Error>,
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error>,
    {
        Ok(self.take(&decide, &record).await?.batch)
    }

    async fn take<D, R>(&self, decide: &D, record: &R) -> Result<Taken, sqlx::Error>
    where
        D: AsyncFn(&Claimed, &mut PgConnection) -> Result<Decision, sqlx::Error>,
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error>,
    {
        self.recover_with(record).await?;
        let claim = Uuid::new_v4();
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query(CANDIDATES)
            .bind(self.options.batch_size)
            .bind(&self.unordered)
            .fetch_all(&mut *transaction)
            .await?;
        let mut taken = Taken::default();
        for row in &rows {
            let charges: i32 = row.try_get("charges")?;
            let mut event = Claimed {
                id: row.try_get("id")?,
                attempts: charges.saturating_add(1),
                event_id: row.try_get("event_id")?,
                topic: row.try_get("topic")?,
                kind: row.try_get("kind")?,
                key: row.try_get("key")?,
                payload: Value::Null,
                relay: self.relay,
                claim,
            };
            match self.payload_of(&event, row)? {
                Ok(payload) => event.payload = payload,
                Err(Unread::NoKey) => {
                    taken.unopened += 1;
                    continue;
                }
                Err(Unread::Tampered) => {
                    taken.decided += 1;
                    let mut point = (&mut transaction).begin().await?;
                    match self.tampered(record, &event, &mut point).await {
                        Ok(()) => {
                            point.commit().await?;
                            tracing::error!(
                                id = event.id,
                                topic = event.topic,
                                kind = event.kind,
                                "outbox event no longer matches what was written, and is not published"
                            );
                        }
                        Err(error) => {
                            point.rollback().await?;
                            self.defer(&mut transaction, &event, &error).await?;
                        }
                    }
                    continue;
                }
            }
            let mut point = (&mut transaction).begin().await?;
            match self.decide_one(decide, record, &event, &mut point).await {
                Ok(Decided::Deliver) => {
                    point.commit().await?;
                    taken.batch.push(event);
                }
                Ok(decided) => {
                    point.commit().await?;
                    taken.decided += 1;
                    if matches!(decided, Decided::Refused) {
                        tracing::info!(
                            id = event.id,
                            topic = event.topic,
                            kind = event.kind,
                            "outbox event refused by the service's decision"
                        );
                    }
                }
                Err(error) => {
                    point.rollback().await?;
                    taken.decided += 1;
                    self.defer(&mut transaction, &event, &error).await?;
                }
            }
        }
        if !taken.batch.is_empty() {
            let event_ids: Vec<Uuid> = taken.batch.iter().map(|event| event.event_id).collect();
            sqlx::query(LEASE)
                .bind(claim)
                .bind(self.relay)
                .execute(&mut *transaction)
                .await?;
            sqlx::query(CLAIMED)
                .bind(event_ids)
                .bind(self.relay)
                .bind(claim)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(taken)
    }

    /// The payload of a candidate row, opened if it was sealed.
    fn payload_of(
        &self,
        event: &Claimed,
        row: &PgRow,
    ) -> Result<Result<Value, Unread>, sqlx::Error> {
        if !row.try_get::<bool, _>("intact")? {
            return Ok(Err(Unread::Tampered));
        }
        if let Some(payload) = row.try_get::<Option<Value>, _>("payload")? {
            return Ok(Ok(payload));
        }
        let Some(sealed) = row.try_get::<Option<Vec<u8>>, _>("sealed")? else {
            return Ok(Err(Unread::Tampered));
        };
        Ok(self.open_sealed(event.event_id, &sealed))
    }

    #[cfg(feature = "encryption")]
    fn open_sealed(&self, event_id: Uuid, sealed: &[u8]) -> Result<Value, Unread> {
        let Some(keys) = &self.keys else {
            return Err(Unread::NoKey);
        };
        match keys.open(event_id, sealed) {
            Ok(plain) => serde_json::from_slice(&plain).map_err(|_| Unread::Tampered),
            Err(Unopened::NoKey) => Err(Unread::NoKey),
            Err(Unopened::Tampered) => Err(Unread::Tampered),
        }
    }

    #[cfg(not(feature = "encryption"))]
    #[allow(clippy::unused_self, reason = "the same call as with the feature")]
    fn open_sealed(&self, _event_id: Uuid, _sealed: &[u8]) -> Result<Value, Unread> {
        Err(Unread::NoKey)
    }

    /// Fail a changed event for good, and record it, in the claim's transaction.
    async fn tampered<R>(
        &self,
        record: &R,
        event: &Claimed,
        point: &mut Transaction<'_, Postgres>,
    ) -> Result<(), sqlx::Error>
    where
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error>,
    {
        let attempt = self
            .decided(point, event, Outcome::Failed, "tampered")
            .await?;
        record(&attempt, &mut **point).await
    }

    /// Ask the service about one event, and act on its answer, in a savepoint of the
    /// claim's transaction.
    async fn decide_one<D, R>(
        &self,
        decide: &D,
        record: &R,
        event: &Claimed,
        point: &mut Transaction<'_, Postgres>,
    ) -> Result<Decided, sqlx::Error>
    where
        D: AsyncFn(&Claimed, &mut PgConnection) -> Result<Decision, sqlx::Error>,
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error>,
    {
        match decide(event, &mut **point).await? {
            Decision::Deliver => Ok(Decided::Deliver),
            Decision::Refuse(reason) => {
                let attempt = self
                    .decided(point, event, Outcome::Refused, &code(&reason))
                    .await?;
                record(&attempt, &mut **point).await?;
                Ok(Decided::Refused)
            }
            Decision::Hold => {
                sqlx::query(HOLD)
                    .bind(event.event_id)
                    .bind(self.options.hold_interval.as_secs_f64())
                    .execute(&mut **point)
                    .await?;
                Ok(Decided::Held)
            }
        }
    }

    /// Write an outcome reached in the claim, on a row the claim holds.
    async fn decided(
        &self,
        point: &mut Transaction<'_, Postgres>,
        event: &Claimed,
        outcome: Outcome,
        reason: &str,
    ) -> Result<Attempt, sqlx::Error> {
        let id: i64 = sqlx::query_scalar(DECIDED)
            .bind(event.event_id)
            .bind(outcome.as_str())
            .bind(self.relay)
            .bind(event.claim)
            .bind(reason)
            .fetch_one(&mut **point)
            .await?;
        Ok(attempt_of(event, id, outcome, Some(reason.to_string())))
    }

    /// Leave an event whose decision or record failed as it was, to be decided again
    /// after a hold's wait rather than at once.
    async fn defer(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &Claimed,
        error: &sqlx::Error,
    ) -> Result<(), sqlx::Error> {
        tracing::error!(
            id = event.id,
            topic = event.topic,
            kind = event.kind,
            %error,
            "outbox decision or record failed, and the event waits"
        );
        sqlx::query(DEFER)
            .bind(event.event_id)
            .bind(self.options.hold_interval.as_secs_f64())
            .execute(&mut **transaction)
            .await?;
        Ok(())
    }

    /// Say, in the caller's `transaction`, that the broker has `events`, returning the
    /// attempts written, for the caller's audit. Call this only once the broker has
    /// acknowledged them. A claim given up on still counts, as an attempt of its own,
    /// because the broker has the event whoever says so. A claim already ended, or not
    /// this relay's, writes nothing.
    ///
    /// # Errors
    ///
    /// Fails on a database error. The events then count as lost, and are published
    /// again.
    pub async fn delivered(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        events: &[Claimed],
    ) -> Result<Vec<Attempt>, sqlx::Error> {
        let mut written = Vec::new();
        for event in events {
            sqlx::query(LOCK_OWED)
                .bind(event.event_id)
                .execute(&mut **transaction)
                .await?;
            let id: Option<i64> = sqlx::query_scalar(DELIVERED)
                .bind(event.event_id)
                .bind(event.relay)
                .bind(event.claim)
                .fetch_optional(&mut **transaction)
                .await?;
            if let Some(id) = id {
                written.push(attempt_of(event, id, Outcome::Delivered, None));
            }
        }
        Ok(written)
    }

    /// Charge `event` an attempt, for the broker's `reason`, in the caller's
    /// `transaction`. It is tried again after a wait, as `rejected`, or with no
    /// attempts left is `failed`. Answers the attempt written, or `None` when the claim
    /// was given up on or already ended, which changes nothing.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn failed(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &Claimed,
        reason: &str,
    ) -> Result<Option<Attempt>, sqlx::Error> {
        let wait = backoff(self.options.retry_backoff, event.attempts, MAX_BACKOFF);
        self.ended(transaction, event, false, &code(reason), wait)
            .await
    }

    /// Refuse `event` for good, for `reason`, in the caller's `transaction`, as a
    /// decision made after the claim. Nothing puts it back. Answers the attempt
    /// written, or `None` when the claim was given up on or already ended.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn refused(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &Claimed,
        reason: &str,
    ) -> Result<Option<Attempt>, sqlx::Error> {
        self.ended(transaction, event, true, &code(reason), Duration::ZERO)
            .await
    }

    async fn ended(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &Claimed,
        refuse: bool,
        reason: &str,
        wait: Duration,
    ) -> Result<Option<Attempt>, sqlx::Error> {
        let row = sqlx::query(ENDED)
            .bind(event.event_id)
            .bind(self.relay)
            .bind(event.claim)
            .bind(refuse)
            .bind(self.options.max_attempts)
            .bind(reason)
            .bind(wait.as_secs_f64())
            .fetch_optional(&mut **transaction)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let outcome: String = row.try_get("outcome")?;
        let outcome = Outcome::parse(&outcome).unwrap_or(Outcome::Failed);
        Ok(Some(attempt_of(
            event,
            row.try_get("id")?,
            outcome,
            Some(reason.to_string()),
        )))
    }

    /// Give `events` back untouched, in the caller's `transaction`, charging nothing,
    /// for events not published. Answers how many went back: a claim given up on gives
    /// back nothing.
    ///
    /// # Errors
    ///
    /// Fails on a database error. The events then wait to be recovered as lost.
    pub async fn returned(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        events: &[Claimed],
    ) -> Result<u64, sqlx::Error> {
        self.give_back(transaction, events, None, Duration::ZERO)
            .await
    }

    async fn give_back(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        events: &[Claimed],
        reason: Option<&str>,
        wait: Duration,
    ) -> Result<u64, sqlx::Error> {
        if events.is_empty() {
            return Ok(0);
        }
        let event_ids: Vec<Uuid> = events.iter().map(|event| event.event_id).collect();
        let claims: Vec<Uuid> = events.iter().map(|event| event.claim).collect();
        let result = sqlx::query(RETURNED)
            .bind(event_ids)
            .bind(claims)
            .bind(self.relay)
            .bind(reason.map(code))
            .bind(wait.as_secs_f64())
            .execute(&mut **transaction)
            .await?;
        Ok(result.rows_affected())
    }

    /// Report in for the claims of `events`, so a batch can take longer than
    /// [`OutboxOptions::stale_minutes`]. [`Outbox::run`] does this by itself. Answers
    /// how many claims were heard from.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn heartbeat(&self, events: &[Claimed]) -> Result<u64, sqlx::Error> {
        let mut claims: Vec<Uuid> = events.iter().map(|event| event.claim).collect();
        claims.sort_unstable();
        claims.dedup();
        let result = sqlx::query(HEARTBEAT)
            .bind(claims)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// End every claim not heard from for [`OutboxOptions::stale_minutes`], with a
    /// `lost` attempt if the event has attempts left and an `abandoned` one if not,
    /// recording each with `record`, and editing nothing. Answers how many went back.
    /// [`Outbox::claim`] does this first.
    ///
    /// # Errors
    ///
    /// Fails on a database error, or when `record` fails, and then nothing is ended.
    pub async fn recover<R>(&self, record: R) -> Result<u64, sqlx::Error>
    where
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error>,
    {
        self.recover_with(&record).await
    }

    async fn recover_with<R>(&self, record: &R) -> Result<u64, sqlx::Error>
    where
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error>,
    {
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query(RECOVER)
            .bind(self.options.stale_minutes)
            .bind(self.options.max_attempts)
            .bind(self.relay)
            .fetch_all(&mut *transaction)
            .await?;
        let (mut lost, mut abandoned) = (0_u64, 0_u64);
        for row in &rows {
            let outcome: String = row.try_get("outcome")?;
            let outcome = Outcome::parse(&outcome).unwrap_or(Outcome::Lost);
            if outcome == Outcome::Abandoned {
                abandoned += 1;
            } else {
                lost += 1;
            }
            let attempt = Attempt {
                id: row.try_get("id")?,
                event_id: row.try_get("event_id")?,
                topic: row.try_get("topic")?,
                kind: row.try_get("kind")?,
                key: row.try_get("key")?,
                outcome,
                reason: None,
                relay: self.relay,
                claim: row.try_get("claim")?,
            };
            record(&attempt, &mut *transaction).await?;
        }
        sqlx::query(DROP_LEASES)
            .bind(self.options.stale_minutes)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        if abandoned > 0 {
            tracing::error!(
                abandoned,
                "outbox events failed for good, abandoned by relays"
            );
        }
        if lost > 0 {
            tracing::warn!(lost, "outbox events recovered from a lost relay");
        }
        Ok(lost)
    }

    /// Say what is owed, and what went out or was refused in the last hour. This reads
    /// only what is owed, and the last hour's outcomes.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn stats(&self) -> Result<Stats, sqlx::Error> {
        let row = sqlx::query(STATS).fetch_one(&self.pool).await?;
        let age = |column: &str| -> Result<Option<Duration>, sqlx::Error> {
            let seconds: Option<f64> = row.try_get(column)?;
            Ok(seconds.map(|seconds| Duration::from_secs_f64(seconds.max(0.0))))
        };
        Ok(Stats {
            pending: row.try_get("pending")?,
            claimed: row.try_get("claimed")?,
            held: row.try_get("held")?,
            failed: row.try_get("failed")?,
            tampered: row.try_get("tampered")?,
            delivered: row.try_get("delivered")?,
            refused: row.try_get("refused")?,
            oldest: age("oldest")?,
            oldest_held: age("oldest_held")?,
            oldest_failed: age("oldest_failed")?,
        })
    }

    /// Relay until `shutdown` changes to `true`.
    ///
    /// Each event is first decided with `decide`, in the claim's transaction. Those
    /// to deliver are published with `publish`, which answers `Ok` only once the broker
    /// has acknowledged the event, and a [`Failure`] otherwise. A publisher that says
    /// `Ok` without doing that, as one that only logs does, records deliveries nobody
    /// received. It runs in its own task, so a panic stops only that publish. Each
    /// outcome is then written, and `record` runs in its transaction before it commits.
    /// If `record` fails, the outcome rolls back, the event stays claimed to be
    /// recovered, and the rest of the batch goes back.
    ///
    /// On shutdown a publish in progress finishes, within
    /// [`OutboxOptions::publish_timeout`], and the rest of the batch goes back. Run
    /// more than one by connecting more than once, or from more than one process.
    ///
    /// The decision and the record are required, as there is no form of `run` without
    /// them. A service with no rule and no audit says so with [`decide::deliver_all`] and
    /// [`record::nothing`]. This does not compile:
    ///
    /// ```compile_fail,E0061
    /// # async fn example(outbox: axa_outbox::Outbox, stopped: tokio::sync::watch::Receiver<bool>) {
    /// outbox
    ///     .run(|_event| async { Ok::<(), axa_outbox::Failure>(()) }, stopped)
    ///     .await;
    /// # }
    /// ```
    ///
    /// [`decide::deliver_all`]: crate::decide::deliver_all
    /// [`record::nothing`]: crate::record::nothing
    pub async fn run<D, P, F, R>(
        self,
        decide: D,
        publish: P,
        record: R,
        mut shutdown: watch::Receiver<bool>,
    ) where
        D: AsyncFn(&Claimed, &mut PgConnection) -> Result<Decision, sqlx::Error> + Send + Sync,
        P: Fn(Claimed) -> F + Send + Sync + 'static,
        F: Future<Output = Result<(), Failure>> + Send + 'static,
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error> + Send + Sync,
    {
        let mut pauses = 0;
        while !*shutdown.borrow() {
            let wait = match self.round(&decide, &publish, &record, &shutdown).await {
                Ok(Round::Worked) => {
                    pauses = 0;
                    continue;
                }
                Ok(Round::Empty) => {
                    pauses = 0;
                    self.options.poll_interval
                }
                Ok(Round::Stalled) => {
                    pauses += 1;
                    backoff(self.options.poll_interval, pauses, MAX_PAUSE)
                }
                Err(error) => {
                    tracing::error!(%error, "outbox relay error");
                    pauses += 1;
                    backoff(self.options.poll_interval, pauses, MAX_PAUSE)
                }
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {},
                changed = shutdown.changed() => {
                    // With no sender left, nothing could ever stop this loop.
                    if changed.is_err() {
                        break;
                    }
                },
            }
        }
    }

    /// Claim a batch and publish it in turn, recording each event as it goes, so that a
    /// crash repeats as few as it can.
    async fn round<D, P, F, R>(
        &self,
        decide: &D,
        publish: &P,
        record: &R,
        shutdown: &watch::Receiver<bool>,
    ) -> Result<Round, sqlx::Error>
    where
        D: AsyncFn(&Claimed, &mut PgConnection) -> Result<Decision, sqlx::Error>,
        P: Fn(Claimed) -> F,
        F: Future<Output = Result<(), Failure>> + Send + 'static,
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error>,
    {
        let taken = self.take(decide, record).await?;
        if taken.unopened > 0 {
            tracing::error!(
                unopened = taken.unopened,
                "outbox holds sealed payloads that no key here opens"
            );
        }
        let batch = taken.batch;
        let Some(claim) = batch.first().map(|event| event.claim) else {
            return Ok(if taken.decided > 0 {
                Round::Worked
            } else if taken.unopened > 0 {
                Round::Stalled
            } else {
                Round::Empty
            });
        };
        let every = self.options.heartbeat_interval;
        let mut beat = tokio::time::interval_at(Instant::now() + every, every);
        beat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut round = Round::Worked;
        for (place, event) in batch.iter().enumerate() {
            let held = &batch[place..];
            if *shutdown.borrow() {
                self.hand_back(held, None, Duration::ZERO).await?;
                break;
            }
            let outcome = match self.attend(publish(event.clone()), claim, &mut beat).await {
                Published::Sent => self.settle(record, event, None).await,
                Published::Failed(Failure::Rejected(reason)) => {
                    self.settle(record, event, Some(&reason)).await
                }
                Published::Failed(Failure::Unavailable(reason)) => {
                    tracing::warn!(id = event.id, "outbox publisher is unavailable");
                    self.hold_back(held, &reason).await?;
                    round = Round::Stalled;
                    break;
                }
                Published::TimedOut => {
                    tracing::warn!(id = event.id, "outbox publish timed out");
                    round = Round::Stalled;
                    self.settle(record, event, Some("timed_out")).await
                }
            };
            if let Err(error) = outcome {
                // The event stays claimed, to be recovered. The rest go back now.
                if let Err(also) = self.hand_back(&held[1..], None, Duration::ZERO).await {
                    tracing::error!(error = %also, "outbox could not give back a batch");
                }
                return Err(error);
            }
            if matches!(round, Round::Stalled) {
                self.hand_back(&held[1..], None, Duration::ZERO).await?;
                break;
            }
        }
        sqlx::query(END_LEASE)
            .bind(claim)
            .execute(&self.pool)
            .await?;
        Ok(round)
    }

    /// Write one event's outcome, delivered or charged for `failure`, and record it, in
    /// one transaction.
    async fn settle<R>(
        &self,
        record: &R,
        event: &Claimed,
        failure: Option<&str>,
    ) -> Result<(), sqlx::Error>
    where
        R: AsyncFn(&Attempt, &mut PgConnection) -> Result<(), sqlx::Error>,
    {
        let mut transaction = self.pool.begin().await?;
        let attempt = match failure {
            None => self
                .delivered(&mut transaction, std::slice::from_ref(event))
                .await?
                .pop(),
            Some(reason) => self.failed(&mut transaction, event, reason).await?,
        };
        if let Some(attempt) = &attempt {
            record(attempt, &mut *transaction).await?;
        }
        transaction.commit().await?;
        let (id, topic, kind, attempts) = (event.id, &event.topic, &event.kind, event.attempts);
        match attempt.map(|attempt| attempt.outcome) {
            Some(Outcome::Failed) => {
                tracing::error!(id, topic, kind, attempts, "outbox event failed for good");
            }
            Some(Outcome::Rejected) => {
                tracing::warn!(id, topic, kind, attempts, "outbox event will be retried");
            }
            Some(_) => {}
            None => tracing::warn!(id, topic, kind, attempts, "outbox claim was given up on"),
        }
        Ok(())
    }

    /// Give `events` back in a transaction of the relay's own.
    async fn hand_back(
        &self,
        events: &[Claimed],
        reason: Option<&str>,
        wait: Duration,
    ) -> Result<(), sqlx::Error> {
        if events.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        self.give_back(&mut transaction, events, reason, wait)
            .await?;
        transaction.commit().await
    }

    /// Give `held` back after an outage, charging nothing. The first keeps the reason,
    /// and waits [`OutboxOptions::retry_backoff`], so that one event a publisher wrongly
    /// calls an outage holds up its own key and no other.
    async fn hold_back(&self, held: &[Claimed], reason: &str) -> Result<(), sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        self.give_back(
            &mut transaction,
            &held[..1],
            Some(reason),
            self.options.retry_backoff,
        )
        .await?;
        self.give_back(&mut transaction, &held[1..], None, Duration::ZERO)
            .await?;
        transaction.commit().await
    }

    /// Run one publish to its end in its own task, keeping the claim alive meanwhile,
    /// so that a panic or a hang stops only the publish.
    async fn attend<F>(&self, work: F, claim: Uuid, beat: &mut Interval) -> Published
    where
        F: Future<Output = Result<(), Failure>> + Send + 'static,
    {
        let mut job = tokio::spawn(work);
        let limit = tokio::time::sleep(self.options.publish_timeout);
        tokio::pin!(limit);
        loop {
            tokio::select! {
                finished = &mut job => {
                    return match finished {
                        Ok(Ok(())) => Published::Sent,
                        Ok(Err(failure)) => Published::Failed(failure),
                        Err(_) => Published::Failed(Failure::rejected("publisher_panicked")),
                    };
                }
                () = &mut limit => {
                    job.abort();
                    return Published::TimedOut;
                }
                _ = beat.tick() => {
                    let heard = sqlx::query(HEARTBEAT)
                        .bind(vec![claim])
                        .execute(&self.pool)
                        .await;
                    if let Err(error) = heard {
                        tracing::warn!(%error, "outbox heartbeat failed");
                    }
                }
            }
        }
    }
}

/// The attempt an outcome for `event` wrote.
fn attempt_of(event: &Claimed, id: i64, outcome: Outcome, reason: Option<String>) -> Attempt {
    Attempt {
        id,
        event_id: event.event_id,
        topic: event.topic.clone(),
        kind: event.kind.clone(),
        key: event.key.clone(),
        outcome,
        reason,
        relay: event.relay,
        claim: event.claim,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_retry_waits_twice_as_long_up_to_a_limit() {
        let base = Duration::from_secs(5);
        assert_eq!(backoff(base, 1, MAX_BACKOFF), base);
        assert_eq!(backoff(base, 2, MAX_BACKOFF), base * 2);
        assert_eq!(backoff(base, 3, MAX_BACKOFF), base * 4);
        assert_eq!(backoff(base, 1_000, MAX_BACKOFF), MAX_BACKOFF);
    }

    #[test]
    fn an_outage_waits_longer_each_time_up_to_a_minute() {
        let base = Duration::from_secs(1);
        assert_eq!(backoff(base, 1, MAX_PAUSE), base);
        assert_eq!(backoff(base, 3, MAX_PAUSE), base * 4);
        assert_eq!(backoff(base, 1_000, MAX_PAUSE), MAX_PAUSE);
    }
}
