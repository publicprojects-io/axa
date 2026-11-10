//! A transactional outbox and relay on Postgres, that keeps a record.
//!
//! A service that changes a row and then tells a broker can crash between the two,
//! and the event is gone. Here the event is a row, written with [`write()`] in the same
//! transaction as the change, so the two commit or roll back together. A relay then
//! decides whether each event may go, publishes it, and records what happened.
//!
//! ```no_run
//! use axa_transactional_outbox::{Event, Failure, Outbox, OutboxOptions, decide, record, write};
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
//!     // Whether each event may go, in the claim's transaction.
//!     decide::deliver_all,
//!     // Answer `Ok` only once the broker has acknowledged the event.
//!     |event| async move {
//!         publish_to_my_broker(&event.topic, &event.key, &event.payload)
//!             .await
//!             .map_err(|_| Failure::unavailable("broker_unreachable"))
//!     },
//!     // The service's audit row, in each outcome's transaction.
//!     record::nothing,
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
//! # A record, not state
//!
//! An event and everything that happens to it are evidence of what a service disclosed
//! and what it refused, so they are only ever added to. `axa_outbox_events`,
//! `axa_outbox_attempts`, and `axa_outbox_payloads` have triggers that refuse `UPDATE`,
//! `DELETE`, and `TRUNCATE`, whoever sends them, and nothing is swept. The one change
//! allowed is [`erase()`], which blanks a payload once it has recorded who erased it and
//! why.
//!
//! An event's state is its latest attempt: `claimed`, `delivered`, `refused`,
//! `rejected` (to be tried again), `failed`, `lost`, `abandoned`, `returned`,
//! `released`, `erased`, or `imported`. Two small tables are working state, and do
//! change: `axa_outbox_owed`, which the database keeps in step with the attempts so that
//! finding work never reads what was delivered, and `axa_outbox_leases`, where a live
//! relay reports in. Their guards keep them honest: `axa_outbox_owed` changes only as
//! the record does, apart from a hold, and [`REBUILD_OWED_STATEMENTS`] make it again from
//! the record. A lease cannot be dated ahead, so changing one can at worst bring a
//! recovery early, and a recovery is recorded.
//!
//! # Deciding, publishing, recording
//!
//! [`Outbox::run`] takes three closures, and all three are required:
//!
//! - **decide**, run before the publisher in the claim's transaction, with the claimed
//!   rows locked. It answers [`Decision::Deliver`], [`Decision::Refuse`], or
//!   [`Decision::Hold`]. A refusal is final: the event is never claimed again, and
//!   [`release()`] refuses it. A hold writes nothing, charges nothing, and leaves the
//!   event to be decided again after [`OutboxOptions::hold_interval`], for as long as
//!   the service holds it. A service with no rule passes [`decide::deliver_all`].
//! - **publish**, as a broker client would, answering `Ok` only once the broker has
//!   acknowledged the event.
//! - **record**, run after each outcome is written, in that outcome's transaction,
//!   before it commits, so that the service's audit row and the outcome commit together
//!   or not at all. A service with nothing to audit passes [`record::nothing`].
//!
//! A closure that fails changes nothing: the decision or the outcome rolls back with it.
//!
//! # Delivery
//!
//! Delivery is at least once. Nothing committed is lost, and an event is `delivered`
//! only after the publisher answers `Ok`. Several relays can run at once, and in the
//! normal case each event is published once, because each claim locks its rows with
//! `FOR UPDATE SKIP LOCKED`.
//!
//! Duplicates remain, and a consumer must dedupe on [`Claimed::event_id`], which
//! [`inbox::seen`] does:
//!
//! - A relay that dies after the broker took an event and before it recorded so.
//! - A relay that stalls for longer than [`OutboxOptions::stale_minutes`]. Its events
//!   are claimed again while it may still be publishing them. If it then succeeds, its
//!   success is kept as an attempt of its own, so the duplicate is on record.
//! - A publish that runs out of [`OutboxOptions::publish_timeout`], or an error
//!   returned after the broker had in fact taken the event.
//! - An event that a person puts back with [`release()`].
//!
//! Every attempt names its relay and its claim, and an outcome must match the claim it
//! ends, so a relay given up on cannot write over the relay that took over.
//!
//! # Order
//!
//! Events with one topic and key go out in the order of [`Claimed::id`]: an event is
//! claimed only when no earlier event with its topic and key is still owed. So an event
//! waiting for a retry, or held, holds back its own key and no other. A `failed` event
//! frees its key, leaving a gap. A topic given [`Order::None`] in
//! [`OutboxOptions::topic_order`] has no order, so one stuck key holds back nothing.
//!
//! This holds when transactions on one key write in the order they commit, which they
//! do if each changes the same row before writing its event, as that row's lock then
//! makes the second wait.
//!
//! # Failures
//!
//! A publisher says which kind of failure it had, with a short code, because they want
//! opposite things:
//!
//! - [`Failure::Unavailable`]: the broker is down. No event is charged an attempt. The
//!   rest of the batch goes back, and the relay waits longer each time. The event it
//!   was trying waits [`OutboxOptions::retry_backoff`], so the next round tries another.
//! - [`Failure::Rejected`]: this event was refused by the broker. It is tried again
//!   after [`OutboxOptions::retry_backoff`], doubling each time, and after
//!   [`OutboxOptions::max_attempts`] it is `failed`, until a person calls [`release()`].
//!
//! A publisher that panics has rejected that event. One that runs past
//! [`OutboxOptions::publish_timeout`] is charged an attempt, and the round stops as in
//! an outage. A reason is a code of at most 200 characters with no line breaks, such
//! as `broker_rejected`. A longer one is kept as `reason_too_long`, and one with a line
//! break as `reason_not_a_code`. Free error text is never stored and never logged.
//!
//! # A changed row
//!
//! [`write()`] stores a SHA-256 fingerprint of the event's id, topic, kind, key, and
//! payload. Every claim checks it before the decision, and an event that no longer
//! matches is `failed` with the reason `tampered`, is never published, and is counted
//! in [`Stats::tampered`]. The triggers refuse a change from any role, including one
//! with `session_replication_role` set to `replica`, so this catches a change made by
//! someone who turned the triggers off.
//!
//! # A relay that died
//!
//! A relay reports in every [`OutboxOptions::heartbeat_interval`]. A claim not heard
//! from for [`OutboxOptions::stale_minutes`] is ended with a `lost` attempt and goes
//! back, charged an attempt, or with an `abandoned` one if it has no attempts left, so
//! an event that kills its relay cannot loop for ever. This happens at the start of
//! every [`Outbox::claim`], with nothing else to run.
//!
//! # Running a relay
//!
//! Nothing relays by itself. [`Outbox::run`] is the loop, and `main.rs` starts it and
//! stops it like any other task. There is no trait to implement and nothing here calls
//! into a service unasked.
//!
//! A service that needs more runs its own loop on [`Outbox::claim`], then
//! [`Outbox::delivered`], [`Outbox::failed`], [`Outbox::refused`], or
//! [`Outbox::returned`]. Each of those takes the service's transaction, so that its
//! audit row commits with the outcome, and there is no form that takes a pool.
//!
//! # Personal data
//!
//! Prefer an id to a snapshot where consumers can look the record up: a payload about
//! a person is a second copy of them, kept as long as the record is.
//!
//! - [`WriteOptions::max_payload_bytes`], 64 KiB by default, refuses a larger payload,
//!   so the caller's transaction fails rather than storing a large copy.
//! - [`WriteOptions::deny`] names fields that must never be in a payload, at any depth.
//!   It is empty by default.
//! - With the `encryption` feature, a [`Writer`] given keys seals each payload with
//!   XChaCha20-Poly1305, bound to its event, and a relay given the same keys opens it.
//! - [`erase()`] blanks one event's payload and records who and why. An erased event is
//!   never published. Its fingerprint stays, as the record of what was there, and on a
//!   plain payload it can confirm a guess of what was erased. A sealed payload's
//!   fingerprint covers only the ciphertext.
//!
//! No log line here holds a payload, a key, or a reason, and the `Debug` text of
//! [`Event`], [`Claimed`], and [`Attempt`] leaves out the payload and the key.
//!
//! # The tables
//!
//! [`Outbox::connect`] creates them. Where a service's role may not change the schema,
//! a migration runs [`SETUP_STATEMENTS`] and the service calls
//! [`Outbox::connect_existing`], which refuses to start unless every table, index, and
//! trigger is there. A service moving from 0.1.0 runs [`IMPORT_STATEMENTS`] too.

use std::{collections::HashMap, fmt, time::Duration};

use serde_json::Value;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

pub mod inbox;
mod relay;
mod schema;
#[cfg(feature = "encryption")]
mod seal;

pub use relay::Outbox;
pub use schema::{IMPORT_STATEMENTS, REBUILD_OWED_STATEMENTS, SETUP_STATEMENTS};
#[cfg(feature = "encryption")]
pub use seal::{Key, KeyTooShort, Keys, MIN_KEY_LEN};

/// The largest payload [`WriteOptions::default`] allows, in bytes of JSON.
pub const DEFAULT_MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// The longest reason or name kept on an attempt, in characters.
pub const MAX_REASON_CHARS: usize = 200;

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

/// What [`Writer`] refuses to write. It reaches the caller as `sqlx::Error::Encode`,
/// so the caller's transaction fails, and it can be had back with `downcast_ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The payload's JSON is larger than [`WriteOptions::max_payload_bytes`].
    TooLarge {
        /// Its size, in bytes.
        bytes: usize,
        /// The limit, in bytes.
        limit: usize,
    },
    /// The payload has a field named in [`WriteOptions::deny`].
    Denied {
        /// The name, as the deny list has it.
        field: String,
    },
    /// Who or why, given to [`release()`] or [`erase()`], is empty, longer than
    /// [`MAX_REASON_CHARS`], or has a line break.
    NotACode {
        /// Which it was: `by` or `reason`.
        what: &'static str,
    },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => write!(
                f,
                "the outbox payload is {bytes} bytes, over the limit of {limit}"
            ),
            Self::Denied { field } => {
                write!(f, "the outbox payload has the denied field `{field}`")
            }
            Self::NotACode { what } => write!(
                f,
                "`{what}` must be 1 to {MAX_REASON_CHARS} characters with no line breaks"
            ),
        }
    }
}

impl std::error::Error for Refused {}

impl From<Refused> for sqlx::Error {
    fn from(refused: Refused) -> Self {
        Self::Encode(Box::new(refused))
    }
}

/// What [`Writer`] checks before it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOptions {
    /// The largest payload, in bytes of JSON. A larger one fails the write.
    pub max_payload_bytes: usize,
    /// Field names a payload must not have, as an object key at any depth, such as
    /// `nhs_number`. Empty by default.
    pub deny: Vec<String>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            max_payload_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
            deny: Vec::new(),
        }
    }
}

/// Writes events, checking each against its [`WriteOptions`] and, with the
/// `encryption` feature and keys, sealing its payload. Build one at boot and share it.
/// [`write()`] is a writer with the default options.
#[derive(Debug, Clone, Default)]
pub struct Writer {
    options: WriteOptions,
    #[cfg(feature = "encryption")]
    keys: Option<Keys>,
}

/// A payload ready to bind: plain JSON, or sealed bytes as hex, so that one statement
/// serves both drivers.
struct Stored {
    plain: Option<String>,
    sealed: Option<String>,
}

/// One text for both drivers, so that `SeaORM` can bind plain strings. The fingerprint
/// is made by the database from what it stores.
const WRITE: &str = "WITH event AS (
        INSERT INTO axa_outbox_events (event_id, topic, kind, key, fingerprint)
        VALUES ($1::uuid, $2, $3, $4, axa_outbox_fingerprint(
            $1::uuid, $2, $3, $4, $5::jsonb, decode($6, 'hex')))
        RETURNING event_id
    )
    INSERT INTO axa_outbox_payloads (event_id, payload, sealed)
    SELECT event_id, $5::jsonb, decode($6, 'hex') FROM event";

impl Writer {
    /// A writer that checks `options`.
    ///
    /// # Errors
    ///
    /// `sqlx::Error::Configuration` when `max_payload_bytes` is zero or a denied name
    /// is empty.
    pub fn new(options: WriteOptions) -> Result<Self, sqlx::Error> {
        if options.max_payload_bytes == 0 {
            return Err(sqlx::Error::Configuration(
                "max_payload_bytes must be at least 1".into(),
            ));
        }
        if options.deny.iter().any(String::is_empty) {
            return Err(sqlx::Error::Configuration(
                "a denied field name must not be empty".into(),
            ));
        }
        Ok(Self {
            options,
            #[cfg(feature = "encryption")]
            keys: None,
        })
    }

    /// Seal every payload with `keys`, so that it is stored encrypted. The relay needs
    /// the same keys, through [`Outbox::opening`].
    #[cfg(feature = "encryption")]
    #[must_use]
    pub fn sealing(mut self, keys: Keys) -> Self {
        self.keys = Some(keys);
        self
    }

    /// Check `event` and make what is stored.
    fn prepare(&self, event: &Event) -> Result<Stored, sqlx::Error> {
        let json = serde_json::to_string(&event.payload)
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
        if json.len() > self.options.max_payload_bytes {
            return Err(Refused::TooLarge {
                bytes: json.len(),
                limit: self.options.max_payload_bytes,
            }
            .into());
        }
        if let Some(field) = denied(&event.payload, &self.options.deny) {
            return Err(Refused::Denied {
                field: field.to_string(),
            }
            .into());
        }
        #[cfg(feature = "encryption")]
        if let Some(keys) = &self.keys {
            let sealed = keys.seal(event.event_id, json.as_bytes())?;
            return Ok(Stored {
                plain: None,
                sealed: Some(hex(&sealed)),
            });
        }
        Ok(Stored {
            plain: Some(json),
            sealed: None,
        })
    }

    /// Write `event` on the caller's transaction, so that it commits or rolls back
    /// with the change it describes. It takes a transaction and not a pool, as an event
    /// written outside the transaction is the bug this crate exists to prevent.
    ///
    /// # Errors
    ///
    /// Fails on a database error, which includes a repeated `event_id`, and with
    /// [`Refused`] inside `sqlx::Error::Encode` for a payload too large or with a
    /// denied field. Either way the caller's transaction should not commit.
    pub async fn write(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &Event,
    ) -> Result<(), sqlx::Error> {
        let stored = self.prepare(event)?;
        sqlx::query(WRITE)
            .bind(event.event_id.to_string())
            .bind(&event.topic)
            .bind(&event.kind)
            .bind(&event.key)
            .bind(stored.plain)
            .bind(stored.sealed)
            .execute(&mut **transaction)
            .await?;
        Ok(())
    }

    /// [`Writer::write`], for a transaction of `SeaORM`'s.
    ///
    /// # Errors
    ///
    /// As [`Writer::write`], with a refusal as `DbErr::Custom`.
    #[cfg(feature = "seaorm")]
    pub async fn write_seaorm(
        &self,
        transaction: &sea_orm::DatabaseTransaction,
        event: &Event,
    ) -> Result<(), sea_orm::DbErr> {
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
        let stored = self
            .prepare(event)
            .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
        let values = [
            event.event_id.to_string().into(),
            event.topic.clone().into(),
            event.kind.clone().into(),
            event.key.clone().into(),
            stored.plain.into(),
            stored.sealed.into(),
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
}

/// Write `event` on the caller's transaction, with the default [`WriteOptions`]: a
/// payload of at most 64 KiB, no denied fields, and no sealing. A service with options
/// of its own builds a [`Writer`].
///
/// # Errors
///
/// As [`Writer::write`].
pub async fn write(
    transaction: &mut Transaction<'_, Postgres>,
    event: &Event,
) -> Result<(), sqlx::Error> {
    Writer::default().write(transaction, event).await
}

/// [`write()`], for a transaction of `SeaORM`'s.
///
/// # Errors
///
/// As [`Writer::write_seaorm`].
#[cfg(feature = "seaorm")]
pub async fn write_seaorm(
    transaction: &sea_orm::DatabaseTransaction,
    event: &Event,
) -> Result<(), sea_orm::DbErr> {
    Writer::default().write_seaorm(transaction, event).await
}

/// The first name in `deny` that `value` has as an object key, at any depth.
fn denied<'a>(value: &Value, deny: &'a [String]) -> Option<&'a str> {
    if deny.is_empty() {
        return None;
    }
    match value {
        Value::Object(fields) => fields.iter().find_map(|(name, inner)| {
            deny.iter()
                .find(|denied| *denied == name)
                .map(String::as_str)
                .or_else(|| denied(inner, deny))
        }),
        Value::Array(items) => items.iter().find_map(|inner| denied(inner, deny)),
        _ => None,
    }
}

#[cfg(feature = "encryption")]
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// A reason as kept: itself if it is a code, and otherwise a code saying why not.
fn code(reason: &str) -> String {
    if reason.chars().count() > MAX_REASON_CHARS {
        tracing::warn!("outbox reason was too long, and is kept as reason_too_long");
        "reason_too_long".to_string()
    } else if reason.chars().any(char::is_control) {
        tracing::warn!("outbox reason had a line break, and is kept as reason_not_a_code");
        "reason_not_a_code".to_string()
    } else {
        reason.to_string()
    }
}

/// Refuse who or why for a person's act unless it is a code.
fn check_code(text: &str, what: &'static str) -> Result<(), Refused> {
    if text.is_empty()
        || text.chars().count() > MAX_REASON_CHARS
        || text.chars().any(char::is_control)
    {
        Err(Refused::NotACode { what })
    } else {
        Ok(())
    }
}

/// An event a relay has claimed.
#[derive(Clone, PartialEq)]
pub struct Claimed {
    /// The event's row. It rises with each event for a key, with gaps, so a consumer
    /// can refuse an event older than one it has applied.
    pub id: i64,
    /// Tries so far, including this one, since the event was written or released.
    pub attempts: i32,
    /// What a consumer dedupes on.
    pub event_id: Uuid,
    /// What the event is about.
    pub topic: String,
    /// What happened.
    pub kind: String,
    /// What keeps events in order.
    pub key: String,
    /// The event itself, opened if it was sealed.
    pub payload: Value,
    /// The relay that claimed it.
    pub relay: Uuid,
    /// This claim, which every outcome for it must name.
    pub claim: Uuid,
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
            .field("claim", &self.claim)
            .finish_non_exhaustive()
    }
}

/// What a service's decision says about one event, before it is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Publish it.
    Deliver,
    /// Never publish it. Final: nothing puts it back. The reason is a code, such as
    /// `no_consent`.
    Refuse(String),
    /// Not now. Nothing is written or charged, and it is decided again after
    /// [`OutboxOptions::hold_interval`]. It holds back its own key and no other.
    Hold,
}

impl Decision {
    /// Never publish it, for `reason`.
    pub fn refuse(reason: impl Into<String>) -> Self {
        Self::Refuse(reason.into())
    }
}

/// What an attempt recorded, as a record closure is told it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The broker has it.
    Delivered,
    /// The service's decision refused it, for good.
    Refused,
    /// The broker refused it, and it will be tried again.
    Rejected,
    /// It will not be tried again until a person releases it: out of attempts, or
    /// `tampered`.
    Failed,
    /// Its relay went quiet, and it goes back, charged an attempt.
    Lost,
    /// Its relay went quiet with no attempts left, and it is failed.
    Abandoned,
}

impl Outcome {
    /// The outcome as the attempts table has it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Refused => "refused",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::Lost => "lost",
            Self::Abandoned => "abandoned",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "delivered" => Self::Delivered,
            "refused" => Self::Refused,
            "rejected" => Self::Rejected,
            "failed" => Self::Failed,
            "lost" => Self::Lost,
            "abandoned" => Self::Abandoned,
            _ => return None,
        })
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One outcome as written, for a service's audit row.
#[derive(Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The attempt's row.
    pub id: i64,
    /// The event's id.
    pub event_id: Uuid,
    /// What the event is about.
    pub topic: String,
    /// What happened.
    pub kind: String,
    /// What keeps events in order.
    pub key: String,
    /// What this attempt recorded.
    pub outcome: Outcome,
    /// Why, as a code, where there is one.
    pub reason: Option<String>,
    /// The relay that wrote it.
    pub relay: Uuid,
    /// The claim it ends.
    pub claim: Uuid,
}

impl fmt::Debug for Attempt {
    /// Leaves out the key, which can name a person.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attempt")
            .field("id", &self.id)
            .field("event_id", &self.event_id)
            .field("topic", &self.topic)
            .field("kind", &self.kind)
            .field("outcome", &self.outcome)
            .field("reason", &self.reason)
            .field("claim", &self.claim)
            .finish_non_exhaustive()
    }
}

/// Decisions for a service with no rule of its own.
pub mod decide {
    use super::{Claimed, Decision, PgConnection};

    /// Deliver every event. Passing this says, in the service's own code, that it
    /// has no policy to apply before an event goes.
    ///
    /// # Errors
    ///
    /// None.
    #[allow(clippy::unused_async, reason = "the relay awaits every decision")]
    pub async fn deliver_all(
        _event: &Claimed,
        _transaction: &mut PgConnection,
    ) -> Result<Decision, sqlx::Error> {
        Ok(Decision::Deliver)
    }
}

/// Records for a service with nothing to audit.
pub mod record {
    use super::{Attempt, PgConnection};

    /// Record nothing. Passing this says, in the service's own code, that it keeps no
    /// audit of what it disclosed or refused.
    ///
    /// # Errors
    ///
    /// None.
    #[allow(clippy::unused_async, reason = "the relay awaits every record")]
    pub async fn nothing(
        _attempt: &Attempt,
        _transaction: &mut PgConnection,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }
}

/// Why a publisher could not publish an event, as a short code that goes on the
/// attempt. Free error text belongs in the service's own log, never here, as a
/// broker's error can quote what it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The broker cannot take anything now. No event is charged an attempt, the rest
    /// of the batch goes back, and the relay waits before trying again.
    Unavailable(String),
    /// This event was refused by the broker. It is charged an attempt, and the relay
    /// moves on to the next.
    Rejected(String),
}

impl Failure {
    /// The broker cannot take anything now, for `reason`, such as `broker_down`.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable(reason.into())
    }

    /// This event was refused, for `reason`, such as `broker_rejected`.
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected(reason.into())
    }
}

/// Whether a topic's events keep their order per key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// Events with one topic and key go out in order, so one stuck event holds back
    /// its key.
    #[default]
    PerKey,
    /// No order, so a stuck event holds back nothing.
    None,
}

/// Outbox tuning.
#[derive(Debug, Clone)]
pub struct OutboxOptions {
    /// Events one claim holds. A relay publishes them in turn.
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
    /// Tries an event gets, including the first, before it is `failed`.
    pub max_attempts: i32,
    /// The wait before the first retry of an event.
    pub retry_backoff: Duration,
    /// How long a held event waits before it is decided again. At most a day, which is
    /// as far ahead as `axa_outbox_owed` lets a hold be set.
    pub hold_interval: Duration,
    /// Topics whose order is not [`Order::PerKey`]. Any topic not named keeps its order.
    pub topic_order: HashMap<String, Order>,
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
            hold_interval: Duration::from_secs(10),
            topic_order: HashMap::new(),
        }
    }
}

impl OutboxOptions {
    /// Refuse settings that cannot work, rather than start a relay that repeats
    /// everything or spins.
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
        } else if self.hold_interval.is_zero() || self.hold_interval > Duration::from_hours(24) {
            Some("hold_interval must be above zero and at most a day")
        } else {
            None
        };
        match problem {
            Some(problem) => Err(sqlx::Error::Configuration(problem.into())),
            None => Ok(()),
        }
    }

    /// The topics with no order.
    fn unordered(&self) -> Vec<String> {
        let mut topics: Vec<String> = self
            .topic_order
            .iter()
            .filter(|(_, order)| **order == Order::None)
            .map(|(topic, _)| topic.clone())
            .collect();
        topics.sort();
        topics
    }
}

/// What an outbox owes, and what it did lately, for a metric or an alert. A count that
/// grows, or an age that keeps rising, means events are not going out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Events waiting for a relay, not held.
    pub pending: i64,
    /// Events a relay is working on.
    pub claimed: i64,
    /// Events the service's decision is holding.
    pub held: i64,
    /// Events no relay will touch until a person calls [`release()`], `tampered` ones
    /// included.
    pub failed: i64,
    /// Failed events whose fingerprint no longer matched.
    pub tampered: i64,
    /// Events delivered in the last hour.
    pub delivered: i64,
    /// Events refused in the last hour.
    pub refused: i64,
    /// How long the oldest pending or claimed event has waited.
    pub oldest: Option<Duration>,
    /// How long the longest hold has lasted.
    pub oldest_held: Option<Duration>,
    /// How long the oldest failed event has waited since it was written.
    pub oldest_failed: Option<Duration>,
}

const RELEASE: &str = "WITH failed AS (
        SELECT event_id FROM axa_outbox_owed
        WHERE event_id = $1 AND state = 'failed'
        FOR UPDATE
    )
    INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim, reason, by)
    SELECT event_id, 'released', $2, $3, $4, $5 FROM failed";

/// Put a `failed` event back to be tried again with its attempts cleared, as an act of
/// `by`, for `reason`, kept as a `released` attempt in the caller's transaction, so
/// that the caller's audit row commits with it. Answers whether it went back. Nothing
/// else can be released: a refused, delivered, or erased event is final, and one still
/// owed needs nothing. Later events for its key may already have gone out, so it can
/// arrive out of order.
///
/// # Errors
///
/// Fails on a database error, and with [`Refused::NotACode`] when `by` or `reason` is
/// empty, longer than [`MAX_REASON_CHARS`], or has a line break.
pub async fn release(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    by: &str,
    reason: &str,
) -> Result<bool, sqlx::Error> {
    check_code(by, "by")?;
    check_code(reason, "reason")?;
    let released = sqlx::query(RELEASE)
        .bind(event_id)
        .bind(Uuid::nil())
        .bind(Uuid::new_v4())
        .bind(reason)
        .bind(by)
        .execute(&mut **transaction)
        .await?
        .rows_affected();
    Ok(released > 0)
}

/// Blank the payload of `event_id`, as an act of `by`, for `reason`, in the caller's
/// transaction. It appends an `erased` attempt, then sets the payload to null, which
/// the triggers allow only after that attempt, changing nothing else. An erased event
/// is never published, and is no longer owed. Answers whether there was a payload to
/// erase.
///
/// # Errors
///
/// Fails on a database error, and with [`Refused::NotACode`] when `by` or `reason` is
/// empty, longer than [`MAX_REASON_CHARS`], or has a line break.
pub async fn erase(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    by: &str,
    reason: &str,
) -> Result<bool, sqlx::Error> {
    check_code(by, "by")?;
    check_code(reason, "reason")?;
    let held = sqlx::query(
        "SELECT 1 FROM axa_outbox_payloads
         WHERE event_id = $1 AND (payload IS NOT NULL OR sealed IS NOT NULL)
         FOR UPDATE",
    )
    .bind(event_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if held.is_none() {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim, reason, by)
         VALUES ($1, 'erased', $2, $3, $4, $5)",
    )
    .bind(event_id)
    .bind(Uuid::nil())
    .bind(Uuid::new_v4())
    .bind(reason)
    .bind(by)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("UPDATE axa_outbox_payloads SET payload = NULL, sealed = NULL WHERE event_id = $1")
        .bind(event_id)
        .execute(&mut **transaction)
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fine() -> OutboxOptions {
        OutboxOptions::default()
    }

    #[test]
    fn the_default_options_are_valid() {
        assert!(fine().validate().is_ok());
        assert!(Writer::new(WriteOptions::default()).is_ok());
        assert_eq!(WriteOptions::default().max_payload_bytes, 64 * 1024);
        assert_eq!(WriteOptions::default().deny, Vec::<String>::new());
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
                hold_interval: Duration::ZERO,
                ..fine()
            },
            OutboxOptions {
                hold_interval: Duration::from_hours(25),
                ..fine()
            },
        ];
        for options in bad {
            assert!(options.validate().is_err(), "{options:?}");
        }
        assert!(
            Writer::new(WriteOptions {
                max_payload_bytes: 0,
                ..WriteOptions::default()
            })
            .is_err()
        );
        assert!(
            Writer::new(WriteOptions {
                deny: vec![String::new()],
                ..WriteOptions::default()
            })
            .is_err()
        );
    }

    #[test]
    fn debug_text_leaves_out_the_payload_and_the_key() {
        let payload = json!({ "name": "Ada Lovelace" });
        let event = Event::new("person", "updated", "ada@example.com", payload.clone());
        let claimed = Claimed {
            id: 1,
            attempts: 1,
            event_id: event.event_id,
            topic: event.topic.clone(),
            kind: event.kind.clone(),
            key: event.key.clone(),
            payload,
            relay: Uuid::new_v4(),
            claim: Uuid::new_v4(),
        };
        let attempt = Attempt {
            id: 1,
            event_id: event.event_id,
            topic: event.topic.clone(),
            kind: event.kind.clone(),
            key: event.key.clone(),
            outcome: Outcome::Delivered,
            reason: None,
            relay: Uuid::new_v4(),
            claim: Uuid::new_v4(),
        };
        for text in [
            format!("{event:?}"),
            format!("{claimed:?}"),
            format!("{attempt:?}"),
        ] {
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

    #[test]
    fn a_reason_is_a_short_code() {
        assert_eq!(code("broker_rejected"), "broker_rejected");
        assert_eq!(code(&"é".repeat(200)), "é".repeat(200));
        assert_eq!(code(&"é".repeat(201)), "reason_too_long");
        assert_eq!(code("two\nlines"), "reason_not_a_code");
        assert_eq!(code("a\rb"), "reason_not_a_code");
        assert!(check_code("ops@example.com", "by").is_ok());
        assert!(check_code("", "by").is_err());
        assert!(check_code("a\nb", "reason").is_err());
        assert!(check_code(&"x".repeat(201), "reason").is_err());
    }

    #[test]
    fn a_denied_field_is_found_at_any_depth() {
        let deny = vec!["nhs_number".to_string(), "password".to_string()];
        assert_eq!(denied(&json!({ "id": 7 }), &deny), None);
        assert_eq!(
            denied(&json!({ "nhs_number": "1" }), &deny),
            Some("nhs_number")
        );
        assert_eq!(
            denied(&json!({ "a": [{ "b": { "password": "x" } }] }), &deny),
            Some("password")
        );
        // A value is not a field name.
        assert_eq!(denied(&json!({ "note": "password" }), &deny), None);
        assert_eq!(denied(&json!(["password"]), &deny), None);
        assert_eq!(denied(&json!({ "password": 1 }), &[]), None);
    }

    #[test]
    fn a_large_payload_or_a_denied_field_is_refused_before_any_write() {
        let writer = Writer::new(WriteOptions {
            max_payload_bytes: 16,
            deny: vec!["secret".to_string()],
        })
        .unwrap();
        let large = Event::new("t", "k", "1", json!({ "text": "x".repeat(20) }));
        let refused = writer.prepare(&large).err().unwrap();
        let sqlx::Error::Encode(inner) = refused else {
            panic!("{refused:?}")
        };
        assert!(matches!(
            inner.downcast_ref::<Refused>(),
            Some(Refused::TooLarge { limit: 16, .. })
        ));
        let secret = Event::new("t", "k", "1", json!({ "secret": 1 }));
        let refused = writer.prepare(&secret).err().unwrap();
        assert!(refused.to_string().contains("secret"), "{refused}");
        assert!(
            writer
                .prepare(&Event::new("t", "k", "1", json!({})))
                .is_ok()
        );
        // Exactly at the limit is allowed.
        let at = Event::new("t", "k", "1", json!("x".repeat(14)));
        assert!(writer.prepare(&at).is_ok());
    }

    #[test]
    fn the_unordered_topics_are_those_given_none() {
        let mut options = fine();
        options.topic_order.insert("audit".into(), Order::None);
        options.topic_order.insert("person".into(), Order::PerKey);
        assert_eq!(options.unordered(), ["audit"]);
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
    fn an_outcome_reads_back_as_itself() {
        for outcome in [
            Outcome::Delivered,
            Outcome::Refused,
            Outcome::Rejected,
            Outcome::Failed,
            Outcome::Lost,
            Outcome::Abandoned,
        ] {
            assert_eq!(Outcome::parse(outcome.as_str()), Some(outcome));
        }
        assert_eq!(Outcome::parse("claimed"), None);
    }
}
