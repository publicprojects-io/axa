//! A transactional outbox and relay on Postgres.
//!
//! A service that changes a row and then tells a broker can crash between the two,
//! and the event is gone. Here the event is a row in `axa_outbox`, written with
//! [`write()`] in the same transaction as the change, so the two commit or roll back
//! together. A relay then publishes each row and marks it sent.
//!
//! ```no_run
//! use axa_outbox::{Event, Failure, Outbox, OutboxOptions, write};
//! use tokio::sync::watch;
//!
//! # async fn example(pool: sqlx::PgPool, uri: &str) -> Result<(), sqlx::Error> {
//! // In a handler. The event is part of the transaction.
//! let mut transaction = pool.begin().await?;
//! sqlx::query("UPDATE people SET name = 'Ada' WHERE id = 7")
//!     .execute(&mut *transaction)
//!     .await?;
//! let payload = serde_json::json!({ "id": 7 });
//! write(&mut transaction, &Event::new("person", "updated", "7", payload)).await?;
//! transaction.commit().await?;
//!
//! // In `main`, beside the server. The broker client stays in the service.
//! let outbox = Outbox::connect(uri, OutboxOptions::default()).await?;
//! let (stop, stopped) = watch::channel(false);
//! let relay = tokio::spawn(outbox.run(
//!     |event| async move {
//!         // Answer `Ok` only once the broker has acknowledged the event.
//!         publish_to_my_broker(&event.topic, &event.key, &event.payload)
//!             .await
//!             .map_err(Failure::unavailable)
//!     },
//!     stopped,
//! ));
//! // ... serve until told to stop, then:
//! let _ = stop.send(true);
//! let _ = relay.await;
//! # Ok(())
//! # }
//! # async fn publish_to_my_broker(
//! #     _topic: &str,
//! #     _key: &str,
//! #     _payload: &serde_json::Value,
//! # ) -> Result<(), std::io::Error> {
//! #     Ok(())
//! # }
//! ```
//!
//! # Delivery
//!
//! Delivery is at least once. Nothing committed is lost, and a row is marked sent
//! only after the publisher answers `Ok`. Several relays can run at once, and in the
//! normal case each row is published once, because each claim locks its rows with
//! `FOR UPDATE SKIP LOCKED` and marks them claimed before any is published.
//!
//! Duplicates remain, and a consumer must dedupe on [`Claimed::event_id`]:
//!
//! - A relay that dies after the broker took an event and before the row is marked
//!   sent. That row is published again.
//! - A relay that stalls for longer than [`OutboxOptions::stale_minutes`]. Its rows
//!   are claimed again while it may still be publishing them.
//! - A publish that runs out of [`OutboxOptions::publish_timeout`], or an error
//!   returned after the broker had in fact taken the event.
//! - A row that [`Outbox::retry_failed`] puts back.
//!
//! # Order
//!
//! Events with one topic and key go out in the order of [`Claimed::id`]: a row is
//! claimed only when no earlier row with its topic and key is pending or claimed.
//! So a row waiting for a retry holds back its own key and no other, and a claim
//! holds at most one row for each key. Different keys have no order between them.
//!
//! This holds when transactions on one key write in the order they commit, which
//! they do if each changes the same row before writing its event, as that row's lock
//! then makes the second wait. It does not hold across a row that ended as `failed`:
//! later rows for that key go ahead, leaving a gap, because holding them for ever is
//! worse. A failed row put back afterwards arrives late. [`Claimed::id`] rises with
//! each event for a key, so a consumer can refuse anything older than it has seen.
//!
//! # Failures
//!
//! A publisher says which kind of failure it had, because they want opposite things:
//!
//! - [`Failure::Unavailable`]: the broker is down. No row is at fault, so none is
//!   charged an attempt. The rest of the batch goes back, and the relay waits longer
//!   each time. An outage only backs the table up, however long it lasts. The row it
//!   was trying waits [`OutboxOptions::retry_backoff`], so the next round tries another,
//!   and a refusal called an outage by mistake holds up its own key and no other.
//! - [`Failure::Rejected`]: this row was refused. It is tried again after
//!   [`OutboxOptions::retry_backoff`], doubling each time, and after
//!   [`OutboxOptions::max_attempts`] it is left as `failed` and the relay moves on.
//!
//! A publisher that panics has rejected that row. One that runs past
//! [`OutboxOptions::publish_timeout`] is charged an attempt, and the round stops as
//! in an outage, as a hang can be either.
//!
//! A `failed` row is kept and never swept, with its error on the row, as it is still
//! owed. [`Outbox::stats`] counts it and [`Outbox::retry_failed`] puts it back.
//!
//! # A relay that died
//!
//! A relay keeps its claim alive, by reporting in every
//! [`OutboxOptions::heartbeat_interval`]. A claim not heard from for
//! [`OutboxOptions::stale_minutes`] comes back as `pending`, or ends as `failed` if
//! it has no attempts left, so a row that kills its relay cannot loop for ever. This
//! happens at the start of every [`Outbox::claim`], so anything that claims recovers
//! what was lost, with nothing else to run. Every row of a batch lost that way is
//! charged an attempt, including those not yet tried.
//!
//! # Running a relay
//!
//! Nothing relays by itself. [`Outbox::run`] is the loop, with the publisher passed
//! in as a closure, so the broker client stays in the service, and `main.rs` starts
//! it and stops it like any other task. There is no trait to implement and nothing
//! here calls into a service unasked.
//!
//! A service that needs more runs its own loop on [`Outbox::claim`], [`Outbox::sent`],
//! [`Outbox::failed`], and [`Outbox::release`]. A broker client that batches needs
//! this, as it can send a whole batch, flush once, and then mark it sent.
//!
//! # Personal data
//!
//! A payload is stored as plain JSONB, so what it holds about a person is a second
//! copy of it. Keep [`OutboxOptions::retention_days`] short, prefer an id to a snapshot
//! where consumers can look the record up, and never put a secret in one. No log line here holds a payload, a key, or a publisher's error text, and
//! the `Debug` text of [`Event`] and [`Claimed`] leaves out the payload and the key.
//! Erasing a person means deleting their rows, which this crate does not know how to
//! find: `DELETE FROM axa_outbox WHERE key = $1` where the key is their id.
//!
//! # The table
//!
//! [`Outbox::connect`] creates `axa_outbox`. Where a service's role may not change
//! the schema, a migration runs [`SETUP_STATEMENTS`] and the service calls
//! [`Outbox::connect_existing`].

use std::{
    fmt::{self, Display},
    future::Future,
    slice,
    time::Duration,
};

use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};
use tokio::{
    sync::watch,
    time::{Instant, Interval, MissedTickBehavior},
};
use uuid::Uuid;

const SETUP: &str = "CREATE TABLE IF NOT EXISTS axa_outbox (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_id UUID NOT NULL UNIQUE,
    topic TEXT NOT NULL,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    payload JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
)";
/// What a claim walks, oldest first.
const SETUP_READY: &str =
    "CREATE INDEX IF NOT EXISTS axa_outbox_ready ON axa_outbox (id) WHERE status = 'pending'";
/// What is still owed: finds an earlier row for a key, and keeps [`Outbox::stats`]
/// off the sent rows.
const SETUP_OWED: &str = "CREATE INDEX IF NOT EXISTS axa_outbox_owed
    ON axa_outbox (topic, key, id) WHERE status <> 'sent'";
/// What recovery looks at, on every claim.
const SETUP_CLAIMED: &str = "CREATE INDEX IF NOT EXISTS axa_outbox_claimed
    ON axa_outbox (updated_at) WHERE status = 'claimed'";
/// What the sweep looks at.
const SETUP_SENT: &str =
    "CREATE INDEX IF NOT EXISTS axa_outbox_sent ON axa_outbox (sent_at) WHERE status = 'sent'";
/// Advisory lock key for table setup: `axa_o` as bytes.
const SETUP_LOCK: i64 = 0x0061_7861_5F6F;

/// One text for both drivers, so that `SeaORM` can bind plain strings and need none of
/// its optional value types.
const WRITE: &str = "INSERT INTO axa_outbox (event_id, topic, kind, key, payload)
    VALUES ($1::uuid, $2, $3, $4, $5::jsonb)";
/// `FOR UPDATE SKIP LOCKED` is what stops two relays taking one row: the outer
/// `UPDATE` does not look at `status` again. An earlier row that is pending or
/// claimed holds a row back whichever the other relay has seen, and `failed` does not.
const CLAIM: &str = "WITH ready AS (
        SELECT candidate.id FROM axa_outbox candidate
        WHERE candidate.status = 'pending' AND candidate.run_at <= now()
          AND NOT EXISTS (
              SELECT 1 FROM axa_outbox earlier
              WHERE earlier.topic = candidate.topic AND earlier.key = candidate.key
                AND earlier.id < candidate.id
                AND earlier.status <> 'sent' AND earlier.status <> 'failed'
          )
        ORDER BY candidate.id
        LIMIT $1
        FOR UPDATE OF candidate SKIP LOCKED
    )
    UPDATE axa_outbox claimed
    SET status = 'claimed', attempts = claimed.attempts + 1, updated_at = now()
    FROM ready WHERE claimed.id = ready.id
    RETURNING claimed.id, claimed.attempts, claimed.event_id, claimed.topic, claimed.kind,
              claimed.key, claimed.payload";
const HEARTBEAT: &str =
    "UPDATE axa_outbox SET updated_at = now() WHERE id = ANY($1) AND status = 'claimed'";
/// Not fenced on `attempts`, unlike the rest. The broker having the event is a fact
/// whoever reports it, and believing a late report spares a duplicate.
const SENT: &str = "UPDATE axa_outbox
    SET status = 'sent', sent_at = now(), error = NULL, updated_at = now()
    WHERE id = ANY($1) AND status <> 'sent'";
/// `attempts` identifies this claim of the row, so a relay given up on changes nothing.
const FAILED: &str = "UPDATE axa_outbox
    SET status = CASE WHEN attempts >= $3 THEN 'failed' ELSE 'pending' END,
        error = $4, run_at = now() + make_interval(secs => $5), updated_at = now()
    WHERE id = $1 AND attempts = $2 AND status = 'claimed'
    RETURNING status";
/// Takes back the attempt the claim counted, so that nothing is charged.
const RELEASE: &str = "UPDATE axa_outbox outbox
    SET status = 'pending', attempts = outbox.attempts - 1, updated_at = now()
    FROM unnest($1::bigint[], $2::integer[]) AS held(id, attempts)
    WHERE outbox.id = held.id AND outbox.attempts = held.attempts
      AND outbox.status = 'claimed'";
/// Puts one row back as [`RELEASE`] does, and makes it wait, so that the next round tries
/// another row. If that one goes out then the broker is up and this row was the trouble.
const HOLD_BACK: &str = "UPDATE axa_outbox
    SET status = 'pending', attempts = attempts - 1, error = $3,
        run_at = now() + make_interval(secs => $4), updated_at = now()
    WHERE id = $1 AND attempts = $2 AND status = 'claimed'";
const REQUEUE_STALE: &str = "UPDATE axa_outbox
    SET status = 'pending', updated_at = now()
    WHERE status = 'claimed' AND updated_at < now() - make_interval(mins => $1)
      AND attempts < $2";
const FAIL_STALE: &str = "UPDATE axa_outbox
    SET status = 'failed', error = 'abandoned by its relay', updated_at = now()
    WHERE status = 'claimed' AND updated_at < now() - make_interval(mins => $1)
      AND attempts >= $2";
const PURGE: &str = "DELETE FROM axa_outbox
    WHERE status = 'sent' AND sent_at < now() - make_interval(days => $1)";
const RETRY_FAILED: &str = "UPDATE axa_outbox
    SET status = 'pending', attempts = 0, error = NULL, run_at = now(), updated_at = now()
    WHERE status = 'failed'";
const STATS: &str = "SELECT
        count(*) FILTER (WHERE status = 'pending') AS pending,
        count(*) FILTER (WHERE status = 'claimed') AS claimed,
        count(*) FILTER (WHERE status = 'failed') AS failed,
        EXTRACT(EPOCH FROM now() - min(created_at) FILTER (WHERE status <> 'failed'))::float8
            AS oldest
    FROM axa_outbox WHERE status <> 'sent'";

/// The statements that make the outbox table, in order, each safe to repeat. A service
/// whose role may not change the schema runs these in a migration, and calls
/// [`Outbox::connect_existing`].
pub const SETUP_STATEMENTS: [&str; 5] = [SETUP, SETUP_READY, SETUP_OWED, SETUP_CLAIMED, SETUP_SENT];

/// Connections one [`Outbox`] holds: it works in sequence, and a heartbeat can
/// overlap an acknowledgement. Each `connect` gets its own, so a relay cannot take
/// connections from requests.
const CONNECTIONS: u32 = 2;

/// The longest wait before a row is tried again.
const MAX_BACKOFF: Duration = Duration::from_hours(1);

/// The longest wait while the broker is down, which is also how long a relay can
/// lag once it is back.
const MAX_PAUSE: Duration = Duration::from_mins(1);

/// The most error text kept on a row.
const MAX_ERROR_CHARS: usize = 1024;

/// How long to wait before the try after `attempts`, doubling each time up to `limit`.
fn backoff(base: Duration, attempts: i32, limit: Duration) -> Duration {
    let doublings = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(16);
    base.saturating_mul(1 << doublings).min(limit)
}

/// An event to write.
#[derive(Clone, PartialEq)]
pub struct Event {
    /// What a consumer dedupes on. [`Event::new`] makes one, and a service whose
    /// envelope already has an id sets it here. A repeat fails the write.
    pub event_id: Uuid,
    /// What the event is about, which usually chooses the broker's topic.
    pub topic: String,
    /// What happened, such as `created`.
    pub kind: String,
    /// What keeps events in order, usually the record's public id. Events with one
    /// topic and key go out in order, and a broker's partition key is usually this.
    pub key: String,
    /// The event itself. Stored as JSONB, so it comes back with its keys reordered.
    pub payload: Value,
}

impl Event {
    /// An event with a new id.
    #[must_use]
    pub fn new(
        topic: impl Into<String>,
        kind: impl Into<String>,
        key: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            topic: topic.into(),
            kind: kind.into(),
            key: key.into(),
            payload,
        }
    }
}

impl fmt::Debug for Event {
    /// Leaves out the payload and the key, which can name a person.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event")
            .field("event_id", &self.event_id)
            .field("topic", &self.topic)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Write `event` on the caller's transaction, so that it commits or rolls back with
/// the change it describes. It takes a transaction and not a pool, as an event written
/// outside the transaction is the bug this crate exists to prevent.
///
/// # Errors
///
/// Fails on a database error, which includes a repeated `event_id`.
pub async fn write(
    transaction: &mut Transaction<'_, Postgres>,
    event: &Event,
) -> Result<(), sqlx::Error> {
    sqlx::query(WRITE)
        .bind(event.event_id)
        .bind(&event.topic)
        .bind(&event.kind)
        .bind(&event.key)
        .bind(&event.payload)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// [`write()`], for a transaction of `SeaORM`'s.
///
/// # Errors
///
/// Fails on a database error, which includes a repeated `event_id`.
#[cfg(feature = "sea-orm")]
pub async fn write_seaorm(
    transaction: &sea_orm::DatabaseTransaction,
    event: &Event,
) -> Result<(), sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let values = [
        event.event_id.to_string().into(),
        event.topic.clone().into(),
        event.kind.clone().into(),
        event.key.clone().into(),
        event.payload.to_string().into(),
    ];
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            WRITE,
            values,
        ))
        .await?;
    Ok(())
}

/// A row a relay has claimed.
#[derive(Clone, PartialEq)]
pub struct Claimed {
    /// The row. It rises with each event for a key, with gaps, so a consumer can
    /// refuse an event older than one it has applied.
    pub id: i64,
    /// Tries so far, including this one. It also identifies this claim of the row.
    pub attempts: i32,
    /// What a consumer dedupes on.
    pub event_id: Uuid,
    /// What the event is about.
    pub topic: String,
    /// What happened.
    pub kind: String,
    /// What keeps events in order.
    pub key: String,
    /// The event itself.
    pub payload: Value,
}

impl fmt::Debug for Claimed {
    /// Leaves out the payload and the key, which can name a person.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Claimed")
            .field("id", &self.id)
            .field("attempts", &self.attempts)
            .field("event_id", &self.event_id)
            .field("topic", &self.topic)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Why a publisher could not publish an event. The text goes on the row, and never
/// in the log, as a broker's error can quote what it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The broker cannot take anything now. No row is charged an attempt, the rest of
    /// the batch goes back, and the relay waits before trying again.
    Unavailable(String),
    /// This event was refused. It is charged an attempt, and the relay moves on to
    /// the next row.
    Rejected(String),
}

impl Failure {
    /// The broker cannot take anything now.
    pub fn unavailable(error: impl Display) -> Self {
        Self::Unavailable(error.to_string())
    }

    /// This event was refused.
    pub fn rejected(error: impl Display) -> Self {
        Self::Rejected(error.to_string())
    }
}

/// Outbox tuning.
#[derive(Debug, Clone)]
pub struct OutboxOptions {
    /// Rows one claim holds. A relay publishes them in turn.
    pub batch_size: i64,
    /// Sleep between polls of an empty outbox.
    pub poll_interval: Duration,
    /// A claim not heard from for this many minutes is assumed lost with its relay.
    pub stale_minutes: i32,
    /// How often a relay reports in. At most half of `stale_minutes`.
    pub heartbeat_interval: Duration,
    /// The longest one publish may take. A broker client can hang, and that must not
    /// hold a relay.
    pub publish_timeout: Duration,
    /// Tries a row gets, including the first, before it is left as `failed`.
    pub max_attempts: i32,
    /// The wait before the first retry of a row.
    pub retry_backoff: Duration,
    /// Days to keep sent rows. Zero sweeps them at the next round.
    pub retention_days: i32,
    /// How often to sweep sent rows.
    pub maintenance_interval: Duration,
}

impl Default for OutboxOptions {
    fn default() -> Self {
        Self {
            batch_size: 100,
            poll_interval: Duration::from_secs(1),
            stale_minutes: 5,
            heartbeat_interval: Duration::from_secs(30),
            publish_timeout: Duration::from_secs(30),
            max_attempts: 10,
            retry_backoff: Duration::from_secs(5),
            retention_days: 7,
            maintenance_interval: Duration::from_mins(5),
        }
    }
}

impl OutboxOptions {
    /// Refuse settings that cannot work, rather than start a relay that repeats
    /// everything, spins, or sweeps what it should keep.
    fn validate(&self) -> Result<(), sqlx::Error> {
        let stale = Duration::from_mins(u64::try_from(self.stale_minutes).unwrap_or(0));
        let problem = if self.batch_size < 1 {
            Some("batch_size must be at least 1")
        } else if self.stale_minutes < 1 {
            Some("stale_minutes must be at least 1")
        } else if self.heartbeat_interval.is_zero() || self.heartbeat_interval * 2 > stale {
            Some("heartbeat_interval must be above zero and at most half of stale_minutes")
        } else if self.publish_timeout.is_zero() {
            Some("publish_timeout must be above zero")
        } else if self.poll_interval.is_zero() {
            Some("poll_interval must be above zero")
        } else if self.max_attempts < 1 {
            Some("max_attempts must be at least 1")
        } else if self.retention_days < 0 {
            Some("retention_days must not be negative")
        } else {
            None
        };
        match problem {
            Some(problem) => Err(sqlx::Error::Configuration(problem.into())),
            None => Ok(()),
        }
    }
}

/// What an outbox owes, for a metric or an alert. A count that grows, or an age that
/// keeps rising, means rows are not going out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Rows waiting for a relay.
    pub pending: i64,
    /// Rows a relay is working on.
    pub claimed: i64,
    /// Rows that ran out of attempts, which no relay will touch until
    /// [`Outbox::retry_failed`].
    pub failed: i64,
    /// How long the oldest pending or claimed row has waited.
    pub oldest: Option<Duration>,
}

/// How one round of a relay ended.
enum Round {
    /// It published what it claimed, or as much as it could.
    Worked,
    /// There was nothing to claim.
    Empty,
    /// The broker is not taking events.
    Stalled,
}

/// How one publish ended.
enum Published {
    Sent,
    Failed(Failure),
    TimedOut,
}

/// A connection to `axa_outbox`, and the relay that drains it. Clones share a pool.
#[derive(Clone)]
pub struct Outbox {
    pool: PgPool,
    options: OutboxOptions,
}

impl fmt::Debug for Outbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outbox")
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
        Ok(Self { pool, options })
    }

    /// Connect to an outbox table that a migration made, and change no schema, for a
    /// database role that may not. A migration runs [`SETUP_STATEMENTS`].
    ///
    /// # Errors
    ///
    /// Fails on options that cannot work, when the database cannot be reached, or when
    /// the table is missing or out of date, so a service finds out at boot.
    pub async fn connect_existing(uri: &str, options: OutboxOptions) -> Result<Self, sqlx::Error> {
        let outbox = Self::open(uri, options).await?;
        sqlx::query(
            "SELECT id, event_id, topic, kind, key, payload, status, attempts, error, run_at,
                    sent_at, created_at, updated_at
             FROM axa_outbox LIMIT 0",
        )
        .execute(&outbox.pool)
        .await?;
        Ok(outbox)
    }

    /// Connect and create the outbox table if needed, which runs DDL at boot. Use
    /// [`Outbox::connect_existing`] where a service's role may not.
    ///
    /// # Errors
    ///
    /// Fails on options that cannot work, when the database cannot be reached, or when
    /// the table cannot be created.
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

    /// Check the database connection.
    ///
    /// # Errors
    ///
    /// Fails when the database does not answer.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Take up to [`OutboxOptions::batch_size`] rows, oldest first, for this caller
    /// alone, having first recovered what a dead relay left. Each must then be passed
    /// to [`Outbox::sent`], [`Outbox::failed`], or [`Outbox::release`], and within
    /// [`OutboxOptions::stale_minutes`], or with [`Outbox::heartbeat`] along the way.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn claim(&self) -> Result<Vec<Claimed>, sqlx::Error> {
        self.requeue_stale().await?;
        let rows = sqlx::query(CLAIM)
            .bind(self.options.batch_size)
            .fetch_all(&self.pool)
            .await?;
        let mut events = rows
            .iter()
            .map(|row| {
                Ok(Claimed {
                    id: row.try_get("id")?,
                    attempts: row.try_get("attempts")?,
                    event_id: row.try_get("event_id")?,
                    topic: row.try_get("topic")?,
                    kind: row.try_get("kind")?,
                    key: row.try_get("key")?,
                    payload: row.try_get("payload")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        // `RETURNING` promises no order.
        events.sort_unstable_by_key(|event| event.id);
        Ok(events)
    }

    /// Say that the broker has `events`, returning how many rows changed. Call this
    /// only once it has acknowledged them, as a row marked sent is never published
    /// again. A row whose claim was given up on is marked too, as it is still true.
    ///
    /// # Errors
    ///
    /// Fails on a database error. The rows then count as lost, and are published again.
    pub async fn sent(&self, events: &[Claimed]) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(SENT)
            .bind(ids(events))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Charge `event` the attempt its claim counted, keeping `error` on the row. It
    /// is tried again after a wait, or left as `failed` with no attempts left. A claim
    /// that was given up on changes nothing.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn failed(&self, event: &Claimed, error: &str) -> Result<(), sqlx::Error> {
        let wait = backoff(self.options.retry_backoff, event.attempts, MAX_BACKOFF);
        let status = sqlx::query_scalar::<_, String>(FAILED)
            .bind(event.id)
            .bind(event.attempts)
            .bind(self.options.max_attempts)
            .bind(error.chars().take(MAX_ERROR_CHARS).collect::<String>())
            .bind(wait.as_secs_f64())
            .fetch_optional(&self.pool)
            .await?;
        let (id, topic, kind, attempts) = (event.id, &event.topic, &event.kind, event.attempts);
        match status.as_deref() {
            Some("failed") => {
                tracing::error!(id, topic, kind, attempts, "outbox event failed for good");
            }
            Some(_) => tracing::warn!(id, topic, kind, attempts, "outbox event will be retried"),
            None => tracing::warn!(id, topic, kind, attempts, "outbox claim was given up on"),
        }
        Ok(())
    }

    /// Put `events` back untouched, charging no attempt, for rows not published.
    ///
    /// # Errors
    ///
    /// Fails on a database error. The rows then wait to be recovered as lost.
    pub async fn release(&self, events: &[Claimed]) -> Result<u64, sqlx::Error> {
        if events.is_empty() {
            return Ok(0);
        }
        let attempts: Vec<i32> = events.iter().map(|event| event.attempts).collect();
        let result = sqlx::query(RELEASE)
            .bind(ids(events))
            .bind(attempts)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Report in for `events`, so a batch can take longer than
    /// [`OutboxOptions::stale_minutes`]. [`Outbox::run`] does this by itself.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn heartbeat(&self, events: &[Claimed]) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(HEARTBEAT)
            .bind(ids(events))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Recover claims not heard from: put back those with attempts left and fail the
    /// rest, returning how many went back. [`Outbox::claim`] does this first.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn requeue_stale(&self) -> Result<u64, sqlx::Error> {
        let abandoned = sqlx::query(FAIL_STALE)
            .bind(self.options.stale_minutes)
            .bind(self.options.max_attempts)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if abandoned > 0 {
            tracing::error!(
                abandoned,
                "outbox events failed for good, abandoned by relays"
            );
        }
        let requeued = sqlx::query(REQUEUE_STALE)
            .bind(self.options.stale_minutes)
            .bind(self.options.max_attempts)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if requeued > 0 {
            tracing::warn!(requeued, "outbox events recovered from a lost relay");
        }
        Ok(requeued)
    }

    /// Delete sent rows older than [`OutboxOptions::retention_days`], returning how
    /// many went. Rows still owed are never deleted, `failed` ones included.
    /// [`Outbox::run`] does this on a timer.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn purge_sent(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(PURGE)
            .bind(self.options.retention_days)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Put every `failed` row back with its attempts cleared, returning how many. Do
    /// this once the cause is fixed. Later rows for a key may already have gone out,
    /// so these can arrive out of order.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn retry_failed(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(RETRY_FAILED).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    /// Say what is owed. This reads only rows not yet sent.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn stats(&self) -> Result<Stats, sqlx::Error> {
        let row = sqlx::query(STATS).fetch_one(&self.pool).await?;
        let oldest: Option<f64> = row.try_get("oldest")?;
        Ok(Stats {
            pending: row.try_get("pending")?,
            claimed: row.try_get("claimed")?,
            failed: row.try_get("failed")?,
            oldest: oldest.map(|seconds| Duration::from_secs_f64(seconds.max(0.0))),
        })
    }

    /// Relay until `shutdown` changes to `true`, publishing each event with `publish`.
    ///
    /// `publish` answers `Ok` only once the broker has acknowledged the event, and a
    /// [`Failure`] otherwise. A publisher that says `Ok` without doing that, as one
    /// that only logs does, marks rows sent that nobody received. It runs in its own
    /// task, so a panic stops only that publish.
    ///
    /// On shutdown a publish in progress finishes, within
    /// [`OutboxOptions::publish_timeout`], and the rest of the batch goes back. Run
    /// more than one by calling this on clones, or from more than one process.
    pub async fn run<P, F>(self, publish: P, mut shutdown: watch::Receiver<bool>)
    where
        P: Fn(Claimed) -> F + Send + Sync + 'static,
        F: Future<Output = Result<(), Failure>> + Send + 'static,
    {
        let mut swept: Option<Instant> = None;
        let mut pauses = 0;
        while !*shutdown.borrow() {
            if swept.is_none_or(|at| at.elapsed() >= self.options.maintenance_interval) {
                swept = Some(Instant::now());
                if let Err(error) = self.purge_sent().await {
                    tracing::error!(%error, "could not delete old outbox rows");
                }
            }
            let wait = match self.round(&publish, &shutdown).await {
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

    /// Claim a batch and publish it in turn, acknowledging each row as it goes, so
    /// that a crash repeats as few as it can.
    async fn round<P, F>(
        &self,
        publish: &P,
        shutdown: &watch::Receiver<bool>,
    ) -> Result<Round, sqlx::Error>
    where
        P: Fn(Claimed) -> F,
        F: Future<Output = Result<(), Failure>> + Send + 'static,
    {
        let batch = self.claim().await?;
        if batch.is_empty() {
            return Ok(Round::Empty);
        }
        let every = self.options.heartbeat_interval;
        let mut beat = tokio::time::interval_at(Instant::now() + every, every);
        beat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        for (place, event) in batch.iter().enumerate() {
            if *shutdown.borrow() {
                self.release(&batch[place..]).await?;
                return Ok(Round::Worked);
            }
            let held = &batch[place..];
            match self.attend(publish(event.clone()), held, &mut beat).await {
                Published::Sent => {
                    self.sent(slice::from_ref(event)).await?;
                }
                Published::Failed(Failure::Rejected(error)) => {
                    self.failed(event, &error).await?;
                }
                Published::Failed(Failure::Unavailable(error)) => {
                    tracing::warn!(id = event.id, "outbox publisher is unavailable");
                    self.hold_back(held, &error).await?;
                    return Ok(Round::Stalled);
                }
                Published::TimedOut => {
                    tracing::warn!(id = event.id, "outbox publish timed out");
                    self.failed(event, "the publish timed out").await?;
                    self.release(&held[1..]).await?;
                    return Ok(Round::Stalled);
                }
            }
        }
        Ok(Round::Worked)
    }

    /// Put `held` back after an outage, charging nothing. The first keeps the error, and
    /// waits [`OutboxOptions::retry_backoff`], so that one row a publisher wrongly calls
    /// an outage holds up its own key and no other.
    async fn hold_back(&self, held: &[Claimed], error: &str) -> Result<(), sqlx::Error> {
        let (first, rest) = (&held[0], &held[1..]);
        sqlx::query(HOLD_BACK)
            .bind(first.id)
            .bind(first.attempts)
            .bind(error.chars().take(MAX_ERROR_CHARS).collect::<String>())
            .bind(self.options.retry_backoff.as_secs_f64())
            .execute(&self.pool)
            .await?;
        self.release(rest).await?;
        Ok(())
    }

    /// Run one publish to its end in its own task, keeping `held` alive meanwhile, so
    /// that a panic or a hang stops only the publish.
    async fn attend<F>(&self, work: F, held: &[Claimed], beat: &mut Interval) -> Published
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
                        Err(_) => Published::Failed(Failure::rejected("the publisher panicked")),
                    };
                }
                () = &mut limit => {
                    job.abort();
                    return Published::TimedOut;
                }
                _ = beat.tick() => {
                    if let Err(error) = self.heartbeat(held).await {
                        tracing::warn!(%error, "outbox heartbeat failed");
                    }
                }
            }
        }
    }
}

/// The ids of `events`.
fn ids(events: &[Claimed]) -> Vec<i64> {
    events.iter().map(|event| event.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fine() -> OutboxOptions {
        OutboxOptions::default()
    }

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

    #[test]
    fn the_default_options_are_valid() {
        assert!(fine().validate().is_ok());
    }

    #[test]
    fn options_that_cannot_work_are_refused() {
        let bad = [
            OutboxOptions {
                batch_size: 0,
                ..fine()
            },
            OutboxOptions {
                stale_minutes: 0,
                ..fine()
            },
            OutboxOptions {
                heartbeat_interval: Duration::ZERO,
                ..fine()
            },
            OutboxOptions {
                heartbeat_interval: Duration::from_mins(3),
                ..fine()
            },
            OutboxOptions {
                publish_timeout: Duration::ZERO,
                ..fine()
            },
            OutboxOptions {
                poll_interval: Duration::ZERO,
                ..fine()
            },
            OutboxOptions {
                max_attempts: 0,
                ..fine()
            },
            OutboxOptions {
                retention_days: -1,
                ..fine()
            },
        ];
        for options in bad {
            assert!(options.validate().is_err(), "{options:?}");
        }
    }

    #[tokio::test]
    async fn refused_options_stop_a_connect_before_it_reaches_the_network() {
        let options = OutboxOptions {
            batch_size: 0,
            ..fine()
        };
        let refused = Outbox::connect("postgres://nobody@127.0.0.1:1/none", options)
            .await
            .unwrap_err();
        assert!(matches!(refused, sqlx::Error::Configuration(_)));
    }

    #[test]
    fn debug_text_leaves_out_the_payload_and_the_key() {
        let payload = serde_json::json!({ "name": "Ada Lovelace" });
        let event = Event::new("person", "updated", "ada@example.com", payload.clone());
        let claimed = Claimed {
            id: 1,
            attempts: 1,
            event_id: event.event_id,
            topic: event.topic.clone(),
            kind: event.kind.clone(),
            key: event.key.clone(),
            payload,
        };
        for text in [format!("{event:?}"), format!("{claimed:?}")] {
            assert!(!text.contains("Ada"), "{text}");
            assert!(!text.contains("ada@example.com"), "{text}");
            assert!(text.contains("updated"), "{text}");
        }
    }

    #[test]
    fn each_event_gets_its_own_id() {
        let one = Event::new("t", "k", "1", Value::Null);
        let two = Event::new("t", "k", "1", Value::Null);
        assert_ne!(one.event_id, two.event_id);
    }

    #[test]
    fn this_crate_depends_on_no_broker_client() {
        let manifest = include_str!("../Cargo.toml");
        for broker in ["fluvio", "rdkafka", "kafka", "lapin", "nats", "pulsar"] {
            assert!(!manifest.contains(broker), "{broker}");
        }
    }
}
