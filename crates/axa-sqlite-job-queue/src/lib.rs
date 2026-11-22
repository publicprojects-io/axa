//! A SQLite job queue, worker pool, and recurring schedules: the SQLite twin of
//! `axa-postgres-job-queue`, with the same API and the same behaviour, for a service
//! on one machine.
//!
//! Jobs live in one `axa_queue` table. Each job loop claims the ready job with the
//! highest priority, oldest first, in a single `UPDATE ... RETURNING`. SQLite runs one
//! writer at a time, so two loops, or two processes on the same file, can never claim
//! the same row, and no broker is needed. The database is opened in WAL mode, with a
//! five-second wait for the write lock.
//!
//! Every time is kept as microseconds since the Unix epoch, in UTC, on the database's
//! clock. SQLite is a file: every process using a queue must be on the machine that
//! holds it.
//!
//! ```no_run
//! use std::time::Duration;
//! use axa_sqlite_job_queue::{NewJob, Queue, QueueOptions, Schedule};
//! use tokio::sync::watch;
//!
//! # async fn example(uri: &str) -> axa_sqlite_job_queue::Result<()> {
//! let mut queue = Queue::connect(uri, QueueOptions::default()).await?;
//! queue.handle("Report", |args| async move {
//!     let id = args["id"].as_i64().ok_or("no id")?;
//!     println!("report {id}");
//!     Ok::<(), &str>(())
//! });
//! queue.limit("Report", 1); // at most one at a time in this process
//! queue.schedule(Schedule::every(
//!     "nightly-report",
//!     Duration::from_hours(24),
//!     NewJob::new("Report", serde_json::json!({ "id": 0 })),
//! )?)?;
//!
//! let id = queue
//!     .enqueue_job(&NewJob::new("Report", serde_json::json!({ "id": 7 })).run_in(Duration::from_mins(5)))
//!     .await?;
//! let queue = std::sync::Arc::new(queue);
//! let (stop, stopped) = watch::channel(false);
//! let jobs = tokio::spawn(queue.clone().run(stopped));
//! // ... serve until told to stop, then:
//! let _ = stop.send(true);
//! let _ = jobs.await;
//! # let _ = id;
//! # Ok(())
//! # }
//! ```
//!
//! A job moves from `queued` to `running` to `completed` or `failed`, with the
//! failure text kept on the row. A queued job can be `cancelled`.
//!
//! # A person's acts
//!
//! A job row is state: it changes as the job runs, and retention deletes it. What a
//! person does to a job is a record. [`cancel()`] stops a queued job, and [`retry()`]
//! queues a failed one again or releases one from review. Each takes the caller's
//! transaction and a `by` and `reason`, and appends the act to `axa_job_acts` in the
//! same statement as the change, so the act, the change, and the service's own audit
//! row commit together or not at all. There is no form that takes a pool. An act that
//! does not apply, such as cancelling a running job, changes nothing and writes no
//! act.
//!
//! ```no_run
//! # async fn example(queue: &axa_sqlite_job_queue::Queue, id: uuid::Uuid) -> axa_sqlite_job_queue::Result<()> {
//! let mut transaction = queue.pool().begin().await?;
//! let cancelled = axa_sqlite_job_queue::cancel(&mut transaction, id, "ops:alice", "customer_asked").await?;
//! // ... write the service's own audit row on the same transaction ...
//! transaction.commit().await?;
//! # let _ = cancelled;
//! # Ok(())
//! # }
//! ```
//!
//! `by` and `reason` are codes, not content: at most [`MAX_REASON_CHARS`] characters
//! with no line breaks, and the table refuses anything else. `axa_job_acts` is only
//! ever inserted into, and its triggers refuse `UPDATE` and `DELETE`, whoever sends
//! them. SQLite has no `TRUNCATE`. An act names its job by id, with no foreign key, so
//! it outlives the job's row. [`Queue::acts`] reads a job's acts.
//!
//! # Attempts
//!
//! A job gets [`QueueOptions::max_attempts`] tries, unless it was queued with its
//! own limit. The default is one, because a retry repeats the job's work, and only
//! its author knows that is safe. Each retry waits about twice as long as the last,
//! starting from [`QueueOptions::retry_backoff`], spread at random over the upper
//! half of that wait so jobs that failed together do not all come back at once. A
//! handler that knows better, such as from a receiver's `Retry-After`, returns
//! [`RetryAfter`] as its error. A job that panics counts as failed, and its loop
//! carries on.
//!
//! # Lost jobs
//!
//! A running job reports in every [`QueueOptions::heartbeat_interval`], so a job
//! may run for as long as it needs. One not heard from for
//! [`QueueOptions::stale_minutes`] is assumed lost with its worker. If it has
//! attempts left it is queued again, and otherwise it is marked failed, so a crash
//! can never loop for ever. Each claim is identified by its attempt, so a run that
//! was given up on cannot then overwrite the result of its replacement.
//!
//! # How many at once
//!
//! A process runs [`QueueOptions::workers`] job loops, so at most that many jobs at
//! once. [`Queue::limit`] caps one job type below that, in this process. A limit is
//! per process and not across replicas: three processes with a limit of one may run
//! three. Work that must never overlap anywhere needs a lock of its own.
//!
//! # When a job runs
//!
//! [`NewJob::run_in`] and [`NewJob::run_at`] delay a job, and [`NewJob::priority`]
//! puts it ahead of others that are ready. Delay is measured on the database's
//! clock. A delayed job is claimed only once it is due, and then about as soon as a
//! loop next polls.
//!
//! # Schedules
//!
//! A [`Schedule`] queues a job every interval. Each due time becomes one ordinary
//! job, so its retries, status, limit, and recovery are the queue's. The schedule's
//! row in `axa_schedule` is locked while its job is queued, in the same
//! transaction, so across any number of replicas each due time is queued once.
//!
//! - **No drift.** The next time is counted from when the last was due, not from
//!   when it ran.
//! - **Missed times run once.** After every process was down, one job is queued,
//!   and the schedule keeps its place: the next time is still a whole number of
//!   intervals from the first.
//! - **No stacking.** When the schedule's last job is still queued or running at a
//!   due time, that time is skipped.
//! - **The first time** is [`Schedule::first_at`], or as soon as the row is made.
//!   It applies only when the row is made, so a schedule every 24 hours first due
//!   at 02:00 runs at 02:00 each day, on the database's clock in UTC.
//!
//! [`Schedule::cron`] queues a job at the times a cron pattern names, such as
//! `"0 2 * * *"` for 02:00 each day, in UTC on the database's clock. Its due times
//! are the pattern's, so the same three rules hold: each due time is queued once, times
//! missed while every process was down queue one job and the next time is the
//! pattern's next after now, and a due time is skipped while the last job is queued or
//! running. Its first time is the pattern's next after the row is made, and a changed
//! pattern takes its next time from the new pattern.
//!
//! [`Queue::run`] ticks the schedules on every poll, and a service with no job
//! loops can call [`Queue::tick_schedules`] itself. A schedule's job type must be
//! handled by some worker, or its jobs wait. A schedule removed from code leaves a
//! row that nothing reads.
//!
//! # One job at a time per key
//!
//! [`NewJob::unique`] names a key, such as `"sync:42"`, and while a job of the same
//! name, tenant, and key is queued or running, queuing another adds nothing and
//! answers the id of the one there. The first job's arguments, priority, and time
//! stand. Once it completes, fails, is cancelled, or goes to review, the key is free
//! again. The table holds to this with a unique index, so it holds across
//! replicas and for a job queued in a transaction that has not yet committed: a
//! second enqueue of the key waits for that transaction, and then either finds its
//! job or queues its own. [`retry()`] leaves a failed job alone while another of its
//! key is queued or running.
//!
//! # Queuing inside a transaction
//!
//! [`enqueue_on`] takes the caller's transaction, so a job and the record it is
//! about commit together or not at all. [`Queue::enqueue`] uses its own
//! connection, so a job queued that way can outlive a rollback.
//!
//! # Personal data
//!
//! The `args` column is plain JSON text, and finished rows are deleted after
//! [`QueueOptions::retention_days`], so a job table does not become a second copy
//! of the data. Prefer an id to a snapshot, and never put a secret in arguments.
//! [`JobInfo`] leaves the arguments out. Its error text is whatever the job's
//! error said, so it is for operators, and is not to be sent to a client. An act
//! keeps no arguments either, and nothing logs its `by` or `reason`.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use sqlx::{
    AssertSqlSafe, Row, Sqlite, SqliteConnection, SqliteExecutor, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use uuid::Uuid;

/// An instant, in UTC: every time the queue keeps or takes. Re-exported from `jiff`, so
/// a service needs no dependency of its own to name one.
pub use jiff::Timestamp;

/// What this crate's functions return.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What went wrong.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The database refused or could not be reached.
    Database(sqlx::Error),
    /// A job's arguments could not be encoded.
    Encode(serde_json::Error),
    /// A job failed, or the queue met something it cannot read.
    Job(String),
    /// Who or why, given for a person's act, or a job's unique key, is empty,
    /// longer than [`MAX_REASON_CHARS`], or has a line break.
    NotACode {
        /// Which it was: `by`, `reason`, or `unique key`.
        what: &'static str,
    },
    /// A job's tenant is not a tenant id: see [`is_tenant_id`].
    NotATenant,
    /// A schedule is a mistake in code, found at boot: a name that is empty, over 200
    /// characters, or used twice, an interval under a second, or a cron pattern that does
    /// not parse or names no time to come.
    Schedule(String),
    /// A figure was too large to compute: a count, or a time that does not fit.
    Overflow(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => error.fmt(f),
            Self::Encode(error) => error.fmt(f),
            Self::Job(text) => write!(f, "worker error: {text}"),
            Self::NotACode { what } => write!(
                f,
                "`{what}` must be 1 to {MAX_REASON_CHARS} characters with no line breaks"
            ),
            Self::Schedule(text) => write!(f, "schedule error: {text}"),
            Self::Overflow(figure) => write!(f, "{figure}: the value is too large to compute"),
            Self::NotATenant => f.write_str(
                "a job's tenant must be 1 to 63 lower-case letters, digits, and hyphens, \
                 not starting or ending with a hyphen",
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Encode(error) => Some(error),
            Self::Job(_)
            | Self::NotACode { .. }
            | Self::NotATenant
            | Self::Schedule(_)
            | Self::Overflow(_) => None,
        }
    }
}

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Encode(error)
    }
}

type JobFuture = Pin<Box<dyn Future<Output = Result<(), Failure>> + Send>>;
type Handler = Arc<dyn Fn(Option<String>, serde_json::Value) -> JobFuture + Send + Sync>;

/// What a job queued for a tenant fails with when its handler takes no tenant.
const NO_TENANT_HANDLER: &str =
    "the job has a tenant, and its handler takes none: register it with handle_with_tenant";

/// Now, as microseconds since the Unix epoch, on the database's clock. SQLite keeps
/// one `now` for a whole statement, so every use in a statement agrees.
macro_rules! now {
    () => {
        "CAST(unixepoch('subsec') * 1000000 AS INTEGER)"
    };
}

/// Times are microseconds since the Unix epoch, in UTC, so they compare exactly.
const SETUP: &str = "CREATE TABLE IF NOT EXISTS axa_queue (
    id BLOB PRIMARY KEY,
    name TEXT NOT NULL,
    args TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER,
    priority INTEGER NOT NULL DEFAULT 0,
    timeout_ms INTEGER,
    tenant TEXT CHECK (tenant IS NULL OR (length(tenant) BETWEEN 1 AND 63
        AND tenant NOT GLOB '*[^a-z0-9-]*' AND tenant NOT GLOB '-*' AND tenant NOT GLOB '*-')),
    unique_key TEXT CHECK (unique_key IS NULL OR (length(unique_key) BETWEEN 1 AND 200
        AND NOT unique_key GLOB ('*[' || char(1) || '-' || char(31) || char(127) || '-' || char(159) || ']*'))),
    run_at INTEGER NOT NULL,
    started_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT";
const SETUP_CLAIM_INDEX: &str = "CREATE INDEX IF NOT EXISTS axa_queue_claim
    ON axa_queue (priority DESC, run_at) WHERE status = 'queued'";
/// At most one job queued or running per name, tenant, and key.
const SETUP_UNIQUE_INDEX: &str = "CREATE UNIQUE INDEX IF NOT EXISTS axa_queue_unique
    ON axa_queue (name, coalesce(tenant, ''), unique_key)
    WHERE unique_key IS NOT NULL AND status IN ('queued', 'running')";
const SETUP_STALE_INDEX: &str = "CREATE INDEX IF NOT EXISTS axa_queue_stale
    ON axa_queue (status, updated_at)";
const SETUP_SCHEDULE: &str = "CREATE TABLE IF NOT EXISTS axa_schedule (
    name TEXT PRIMARY KEY,
    due_at INTEGER NOT NULL,
    last_job BLOB,
    interval_ms INTEGER,
    cron TEXT,
    seen_at INTEGER NOT NULL,
    archived_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT";
/// A person's acts on jobs. This table is a record (P-7): it is only ever inserted
/// into, and its triggers refuse every change. A job row is state, and retention
/// deletes it, so an act names its job by id with no foreign key, and keeps the job's
/// name and attempts as they were when the act was done. A code may hold no control
/// character, C0 or C1, as Rust's `char::is_control` has them.
const SETUP_ACTS: &str = "CREATE TABLE IF NOT EXISTS axa_job_acts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id BLOB NOT NULL,
    job_name TEXT NOT NULL,
    act TEXT NOT NULL CHECK (act IN ('cancelled', 'retried', 'released')),
    attempts INTEGER NOT NULL,
    by TEXT NOT NULL CHECK (length(by) BETWEEN 1 AND 200
        AND NOT by GLOB ('*[' || char(1) || '-' || char(31) || char(127) || '-' || char(159) || ']*')),
    reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 200
        AND NOT reason GLOB ('*[' || char(1) || '-' || char(31) || char(127) || '-' || char(159) || ']*')),
    at INTEGER NOT NULL
) STRICT";
const SETUP_ACTS_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS axa_job_acts_job ON axa_job_acts (job_id, id)";
/// Refuses an `UPDATE`, whoever sends it.
const SETUP_ACTS_UPDATE: &str = "CREATE TRIGGER IF NOT EXISTS axa_job_acts_refuse_update
    BEFORE UPDATE ON axa_job_acts
    BEGIN
        SELECT RAISE(ABORT, 'UPDATE on axa_job_acts is refused: acts on jobs are only ever added to');
    END";
/// Refuses a `DELETE`, whoever sends it. SQLite has no `TRUNCATE`: a `DELETE` with no
/// `WHERE` fires this too.
const SETUP_ACTS_DELETE: &str = "CREATE TRIGGER IF NOT EXISTS axa_job_acts_refuse_delete
    BEFORE DELETE ON axa_job_acts
    BEGIN
        SELECT RAISE(ABORT, 'DELETE on axa_job_acts is refused: acts on jobs are only ever added to');
    END";
/// The refusing triggers [`Queue::check`] requires.
const ACTS_TRIGGERS: [&str; 2] = ["axa_job_acts_refuse_update", "axa_job_acts_refuse_delete"];
const PRESENT_TRIGGERS: &str = "SELECT name FROM sqlite_schema
    WHERE type = 'trigger' AND tbl_name = 'axa_job_acts'";
/// A write transaction from the start, so that two processes setting up, or ticking
/// a schedule, take turns.
const IMMEDIATE: &str = "BEGIN IMMEDIATE";

const ENQUEUE: &str = concat!(
    "INSERT INTO axa_queue
    (id, name, args, max_attempts, priority, run_at, timeout_ms, tenant, created_at, updated_at)
    VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, ",
    now!(),
    " + ?7), ?8, ?9, ",
    now!(),
    ", ",
    now!(),
    ")"
);
/// Queue a job with a key, or answer the one of its name, tenant, and key already
/// queued or running. `DO UPDATE`, which changes nothing, so that `RETURNING` gives
/// the job there.
const ENQUEUE_UNIQUE: &str = concat!(
    "INSERT INTO axa_queue
    (id, name, args, max_attempts, priority, run_at, timeout_ms, tenant, unique_key,
        created_at, updated_at)
    VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, ",
    now!(),
    " + ?7), ?8, ?9, ?10, ",
    now!(),
    ", ",
    now!(),
    ")
    ON CONFLICT (name, coalesce(tenant, ''), unique_key)
        WHERE unique_key IS NOT NULL AND status IN ('queued', 'running')
    DO UPDATE SET unique_key = excluded.unique_key
    RETURNING id"
);
/// Claim the ready job with the highest priority, oldest first, among `names`, which
/// [`placeholders`] fills in. One statement, and SQLite runs one writer at a time, so
/// two loops can never claim the same row.
const CLAIM: &str = concat!(
    "UPDATE axa_queue
    SET status = 'running', started_at = ",
    now!(),
    ", updated_at = ",
    now!(),
    ", attempts = attempts + 1
    WHERE id = (
        SELECT id FROM axa_queue
        WHERE status = 'queued' AND run_at <= ",
    now!(),
    " AND name IN ({names})
        ORDER BY priority DESC, run_at
        LIMIT 1
    )
    RETURNING id, name, args, attempts, max_attempts, timeout_ms, tenant"
);
/// `attempts` identifies this run of the job, so a run that was given up on
/// finds no row here and changes nothing.
const HEARTBEAT: &str = concat!(
    "UPDATE axa_queue SET updated_at = ",
    now!(),
    " WHERE id = ?1 AND attempts = ?2 AND status = 'running'"
);
const FINISH: &str = concat!(
    "UPDATE axa_queue SET status = ?1, error = ?2, updated_at = ",
    now!(),
    " WHERE id = ?3 AND attempts = ?4 AND status = 'running'"
);
const RETRY: &str = concat!(
    "UPDATE axa_queue
    SET status = 'queued', error = ?1, started_at = NULL, updated_at = ",
    now!(),
    ", run_at = ",
    now!(),
    " + ?2 WHERE id = ?3 AND attempts = ?4 AND status = 'running'"
);
const REQUEUE_STALE: &str = concat!(
    "UPDATE axa_queue
    SET status = 'queued', started_at = NULL, updated_at = ",
    now!(),
    " WHERE status = 'running' AND updated_at < ",
    now!(),
    " - ?1 * 60000000 AND attempts < COALESCE(max_attempts, ?2)"
);
const FAIL_STALE: &str = concat!(
    "UPDATE axa_queue
    SET status = 'failed', error = 'abandoned by its worker', updated_at = ",
    now!(),
    " WHERE status = 'running' AND updated_at < ",
    now!(),
    " - ?1 * 60000000 AND attempts >= COALESCE(max_attempts, ?2)"
);
const PURGE: &str = concat!(
    "DELETE FROM axa_queue
    WHERE status IN ('completed', 'failed', 'cancelled')
      AND updated_at < ",
    now!(),
    " - ?1 * 86400000000"
);
const STATUS: &str = "SELECT id, name, status, attempts, max_attempts, priority, error,
    timeout_ms, tenant, unique_key, run_at, started_at, created_at, updated_at FROM axa_queue WHERE id = ?1";
/// A run that lost its claim stops, and its job waits for a person. Another run
/// may hold the row by now, and this stops that one too, at its next report.
/// A row already completed is left alone.
const MARK_LOST: &str = concat!(
    "UPDATE axa_queue SET status = 'needs_review', error = ?1, updated_at = ",
    now!(),
    " WHERE id = ?2 AND status IN ('queued', 'running', 'failed')"
);
/// Cancel a job if it is still queued, as the first write of the caller's
/// transaction.
const CANCEL: &str = concat!(
    "UPDATE axa_queue SET status = 'cancelled', updated_at = ",
    now!(),
    " WHERE id = ?1 AND status = 'queued' RETURNING name, attempts"
);
/// A retried job gets one more try. `attempts` keeps rising, as it identifies
/// each run, so a run given up on still cannot write over the new one. `?2` is the
/// state it must be in, `failed` or `needs_review`.
const RETRY_FAILED: &str = concat!(
    "UPDATE axa_queue
    SET status = 'queued', run_at = ",
    now!(),
    ", started_at = NULL, updated_at = ",
    now!(),
    ", max_attempts = attempts + 1
    WHERE id = ?1 AND status = ?2 AND NOT EXISTS (
        SELECT 1 FROM axa_queue other
        WHERE other.unique_key = axa_queue.unique_key AND other.name = axa_queue.name
          AND coalesce(other.tenant, '') = coalesce(axa_queue.tenant, '')
          AND other.status IN ('queued', 'running')
    )
    RETURNING name, attempts"
);
const ACT_KEEP: &str = concat!(
    "INSERT INTO axa_job_acts (job_id, job_name, act, attempts, by, reason, at)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ",
    now!(),
    ")"
);
const ACTS: &str = "SELECT id, job_id, job_name, act, attempts, by, reason, at
    FROM axa_job_acts WHERE job_id = ?1 ORDER BY id";
const COUNTS: &str = concat!(
    "SELECT
    count(*) FILTER (WHERE status = 'queued' AND run_at <= ",
    now!(),
    ") AS ready,
    count(*) FILTER (WHERE status = 'queued' AND run_at > ",
    now!(),
    ") AS waiting,
    count(*) FILTER (WHERE status = 'running') AS running,
    count(*) FILTER (WHERE status = 'completed') AS completed,
    count(*) FILTER (WHERE status = 'failed') AS failed,
    count(*) FILTER (WHERE status = 'cancelled') AS cancelled,
    count(*) FILTER (WHERE status = 'needs_review') AS needs_review,
    (",
    now!(),
    " - min(run_at) FILTER (WHERE status = 'queued' AND run_at <= ",
    now!(),
    ")) / 1e6 AS oldest
    FROM axa_queue"
);
/// Make the row, or say this schedule is still in code: restore it if it was
/// archived, and note the interval or pattern. A changed pattern takes its next time,
/// `?2`, from the new pattern. `seen_at` moves at most once a minute, so a tick does
/// not write each poll. A cron schedule's first time is `?2`, its next after now.
const SCHEDULE_CREATE: &str = concat!(
    "INSERT INTO axa_schedule (name, due_at, interval_ms, cron, seen_at, created_at, updated_at)
    VALUES (?1, COALESCE(?2, ",
    now!(),
    "), ?3, ?4, ",
    now!(),
    ", ",
    now!(),
    ", ",
    now!(),
    ")
    ON CONFLICT (name) DO UPDATE
    SET seen_at = ",
    now!(),
    ", archived_at = NULL, interval_ms = excluded.interval_ms, cron = excluded.cron,
        due_at = CASE WHEN excluded.cron IS NOT NULL AND axa_schedule.cron IS NOT excluded.cron
                 THEN excluded.due_at ELSE axa_schedule.due_at END
    WHERE axa_schedule.archived_at IS NOT NULL
       OR axa_schedule.interval_ms IS NOT excluded.interval_ms
       OR axa_schedule.cron IS NOT excluded.cron
       OR axa_schedule.seen_at < ",
    now!(),
    " - 60000000"
);
const SCHEDULE_LOCK: &str = concat!(
    "SELECT last_job, due_at <= ",
    now!(),
    " AS due, ",
    now!(),
    " AS now FROM axa_schedule WHERE name = ?1 AND archived_at IS NULL"
);
/// Archive schedules no live process has in code. A row any process still ticks
/// is seen within a minute, so one unseen for `stale_minutes` is in none, and an
/// old replica in a rolling deploy keeps its schedules until it stops. `{kept}`
/// leaves out the names this process has.
const SCHEDULE_ARCHIVE: &str = concat!(
    "UPDATE axa_schedule SET archived_at = ",
    now!(),
    ", updated_at = ",
    now!(),
    " WHERE archived_at IS NULL AND seen_at < ",
    now!(),
    " - ?1 * 60000000{kept}"
);
const SCHEDULES: &str = "SELECT s.name, s.interval_ms, s.cron, s.due_at, s.last_job, s.seen_at,
    s.archived_at, s.created_at, q.status AS last_status, q.error AS last_error
    FROM axa_schedule s LEFT JOIN axa_queue q ON q.id = s.last_job ORDER BY s.name";
const SCHEDULE_BUSY: &str = "SELECT EXISTS (SELECT 1 FROM axa_queue
    WHERE id = ?1 AND status IN ('queued', 'running')) AS busy";
/// The next due time after now, a whole number of intervals from the last, on the
/// database's clock. One expression gives no drift, one catch-up, and no stacking.
const SCHEDULE_ADVANCE: &str = concat!(
    "UPDATE axa_schedule
    SET due_at = due_at + ?1 * ((",
    now!(),
    " - due_at) / ?1 + 1),
        last_job = COALESCE(?2, last_job), updated_at = ",
    now!(),
    " WHERE name = ?3"
);
/// A cron schedule's next due time, `?1`, the pattern's next after now.
const SCHEDULE_ADVANCE_TO: &str = concat!(
    "UPDATE axa_schedule SET due_at = ?1, last_job = COALESCE(?2, last_job), updated_at = ",
    now!(),
    " WHERE name = ?3"
);

/// The statements that make the queue's tables, in order, each safe to repeat. A
/// service whose role may not change the schema runs these in a migration, and calls
/// [`Queue::connect_existing`]. They need SQLite 3.38 or later, which sqlx bundles.
pub const SETUP_STATEMENTS: [&str; 9] = [
    SETUP,
    SETUP_CLAIM_INDEX,
    SETUP_UNIQUE_INDEX,
    SETUP_STALE_INDEX,
    SETUP_SCHEDULE,
    SETUP_ACTS,
    SETUP_ACTS_INDEX,
    SETUP_ACTS_UPDATE,
    SETUP_ACTS_DELETE,
];

/// The longest `by` or `reason` kept on an act, in characters.
pub const MAX_REASON_CHARS: usize = 200;

/// The shortest gap between heartbeats, so a zero setting cannot spin.
const MIN_HEARTBEAT: Duration = Duration::from_millis(20);

/// The longest wait between attempts.
const MAX_BACKOFF: Duration = Duration::from_hours(1);

/// The longest error text kept on a row.
const MAX_ERROR: usize = 1024;

/// The most a backoff doubles: 16 doublings.
const MAX_DOUBLING_FACTOR: u32 = 1 << 16;

/// How long to wait before the attempt after `attempts`, doubling each time.
#[must_use]
pub fn backoff(base: Duration, attempts: i32) -> Duration {
    // ARI-2: fewer than one attempt has no doublings, and a wait that does not fit is the cap.
    let doublings = attempts
        .checked_sub(1)
        .and_then(|earlier| u32::try_from(earlier).ok())
        .unwrap_or(0)
        .min(16);
    let factor = 1_u32.checked_shl(doublings).unwrap_or(MAX_DOUBLING_FACTOR);
    base.checked_mul(factor)
        .unwrap_or(MAX_BACKOFF)
        .min(MAX_BACKOFF)
}

/// The shortest delay a job may choose with [`RetryAfter`].
pub const MIN_RETRY_AFTER: Duration = Duration::from_secs(1);

/// A job's error that also says when to try it again, such as a receiver's
/// `Retry-After`. Return it as a handler's error: the next attempt waits `delay`,
/// held between [`MIN_RETRY_AFTER`] and an hour, rather than the backoff. It still
/// counts as an attempt, so a job out of attempts fails as any other.
#[derive(Debug, Clone)]
pub struct RetryAfter {
    delay: Duration,
    reason: String,
}

impl RetryAfter {
    /// Try again after `delay`, keeping `reason` as the job's error.
    #[must_use]
    pub fn new(delay: Duration, reason: impl Into<String>) -> Self {
        Self {
            delay,
            reason: reason.into(),
        }
    }

    /// The delay the job asked for, before it is held to its limits.
    #[must_use]
    pub const fn delay(&self) -> Duration {
        self.delay
    }
}

impl fmt::Display for RetryAfter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for RetryAfter {}

/// How a run failed: the text kept on the row, and any delay the job chose.
struct Failure {
    text: String,
    retry_in: Option<Duration>,
}

impl Failure {
    fn from_error<E: fmt::Display + 'static>(error: E) -> Self {
        let any: &dyn std::any::Any = &error;
        // A handler's own error, or one boxed, as `axa`'s workers hand theirs on.
        let retry_in = any
            .downcast_ref::<RetryAfter>()
            .or_else(|| {
                any.downcast_ref::<Box<dyn std::error::Error + Send + Sync>>()
                    .and_then(|boxed| boxed.downcast_ref::<RetryAfter>())
            })
            .map(RetryAfter::delay);
        Self {
            text: error.to_string(),
            retry_in,
        }
    }
}

impl From<String> for Failure {
    fn from(text: String) -> Self {
        Self {
            text,
            retry_in: None,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Spread `wait` over its upper half at random, so jobs that failed together do
/// not all try again at once.
#[must_use]
pub fn jittered(wait: Duration) -> Duration {
    let random = Uuid::new_v4().as_u128();
    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "RCA-EXC-2: below one million, which an f64 holds exactly"
    )]
    let fraction = (random % 1_000_000) as f64 / 1_000_000.0;
    wait.mul_f64(0.5 + fraction / 2.0)
}

/// How long to wait before the attempt after `attempts`: the delay a job chose,
/// held between [`MIN_RETRY_AFTER`] and an hour, or else the doubling backoff
/// from `base`, jittered.
#[must_use]
pub fn retry_wait(base: Duration, attempts: i32, chosen: Option<Duration>) -> Duration {
    match chosen {
        // WRK-41: a delay a job chose is held between the shortest and the longest wait.
        Some(delay) if delay < MIN_RETRY_AFTER => MIN_RETRY_AFTER,
        Some(delay) if delay > MAX_BACKOFF => MAX_BACKOFF,
        Some(delay) => delay,
        None => jittered(backoff(base, attempts)),
    }
}

/// Cut `text` to at most [`MAX_ERROR`] bytes, on a character boundary.
fn cut(mut text: String) -> String {
    if text.len() > MAX_ERROR {
        let end = text.floor_char_boundary(MAX_ERROR);
        text.truncate(end);
    }
    text
}

/// Why a run is stopped when its times do not fit.
const TIMING: &str = "a time for the run is too large to compute";

/// When a worker stops a job it cannot report on: two heartbeats before another worker
/// may take it, and at half of `stale_minutes` at the latest. `None` when the times do
/// not fit.
fn give_up_after(stale_minutes: i32, every: Duration) -> Option<Duration> {
    let stale = Duration::from_secs(u64::try_from(stale_minutes).ok()?.checked_mul(60)?);
    let half = stale.checked_div(2)?;
    let early = every
        .checked_mul(2)
        .and_then(|twice| stale.checked_sub(twice));
    // ARI-1: when two heartbeats do not fit before the stale time, give up at half of it.
    Some(early.map_or(half, |early| early.max(half)))
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

/// `?, ?, ?` for `count` values, as SQLite has no array to bind. Each `?` takes the
/// next number after those written before it, so place these last.
fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

/// A time kept as microseconds since the Unix epoch.
fn at(micros: i64) -> Result<Timestamp> {
    Timestamp::from_microsecond(micros).map_err(|_| Error::Job("a time out of range".to_string()))
}

/// A time that may be missing, kept as microseconds since the Unix epoch.
fn at_or_none(micros: Option<i64>) -> Result<Option<Timestamp>> {
    micros.map(at).transpose()
}

fn from_millis(millis: i64) -> Duration {
    Duration::from_millis(u64::try_from(millis).unwrap_or(0))
}

/// When a new job may first run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    Now,
    At(Timestamp),
    In(Duration),
}

/// A job to queue: its type, its arguments, and when and how it runs.
#[derive(Clone)]
pub struct NewJob {
    name: String,
    args: serde_json::Value,
    max_attempts: Option<i32>,
    priority: i16,
    when: When,
    timeout: Option<Duration>,
    tenant: Option<String>,
    unique_key: Option<String>,
}

impl fmt::Debug for NewJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewJob")
            .field("name", &self.name)
            .field("max_attempts", &self.max_attempts)
            .field("priority", &self.priority)
            .field("when", &self.when)
            .field("timeout", &self.timeout)
            .field("tenant", &self.tenant)
            .field("unique_key", &self.unique_key)
            .finish_non_exhaustive()
    }
}

impl NewJob {
    /// A job for the handler named `name`, ready now, with the queue's limit on
    /// tries and priority zero.
    pub fn new(name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            args,
            max_attempts: None,
            priority: 0,
            when: When::Now,
            timeout: None,
            tenant: None,
            unique_key: None,
        }
    }

    /// Queue this job only if no job of its name, tenant, and `key` is queued or
    /// running: otherwise queuing it adds nothing and answers the id of the one
    /// there, whose arguments, priority, and time stand. The key is a code, at most
    /// [`MAX_REASON_CHARS`] characters with no line breaks, such as `"sync:42"`, and
    /// is kept on the job's row, so it holds no secret or personal data. Enqueuing
    /// refuses any other key with [`Error::NotACode`].
    #[must_use]
    pub fn unique(mut self, key: impl Into<String>) -> Self {
        self.unique_key = Some(key.into());
        self
    }

    /// Run this job for `tenant`, the id an auth crate's `Tenant` gives, which its
    /// handler receives beside the arguments, so that it can open its own tenant
    /// transaction. Only a handler registered with [`Queue::handle_with_tenant`] runs
    /// it. Enqueuing refuses an id that is not a tenant id, with
    /// [`Error::NotATenant`].
    #[must_use]
    pub fn tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = Some(tenant.into());
        self
    }

    /// Stop a run of this job past `limit`, and count it as failed with the reason.
    /// A job has no time limit unless it is given one.
    #[must_use]
    pub const fn timeout(mut self, limit: Duration) -> Self {
        self.timeout = Some(limit);
        self
    }

    /// Tries this job gets, including the first, or the queue's when `None`.
    #[must_use]
    pub const fn max_attempts(mut self, max_attempts: Option<i32>) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Run ahead of ready jobs with a lower priority. Zero by default, and may be
    /// negative.
    #[must_use]
    pub const fn priority(mut self, priority: i16) -> Self {
        self.priority = priority;
        self
    }

    /// Run no sooner than `delay` from now, on the database's clock.
    #[must_use]
    pub const fn run_in(mut self, delay: Duration) -> Self {
        self.when = When::In(delay);
        self
    }

    /// Run no sooner than `at`.
    #[must_use]
    pub const fn run_at(mut self, at: Timestamp) -> Self {
        self.when = When::At(at);
        self
    }

    /// The handler this job is for.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The tenant this job runs for, if any.
    #[must_use]
    pub fn tenant_id(&self) -> Option<&str> {
        self.tenant.as_deref()
    }

    /// The key this job is unique by, if any.
    #[must_use]
    pub fn unique_key(&self) -> Option<&str> {
        self.unique_key.as_deref()
    }
}

/// The longest tenant id, which a DNS label also allows.
pub const MAX_TENANT_ID_LEN: usize = 63;

/// Whether `id` is a tenant id: lower-case ASCII letters, digits, and hyphens, not
/// starting or ending with a hyphen, from 1 to 63 characters, as a DNS label is, and as
/// the auth crates' `Tenant` holds. The table's check holds to the same rule.
#[must_use]
pub fn is_tenant_id(id: &str) -> bool {
    (1..=MAX_TENANT_ID_LEN).contains(&id.len())
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Add `job` on `executor`, usually the caller's transaction, and return its id.
/// The job then commits or rolls back with the transaction. A job with a
/// [`NewJob::unique`] key that is already queued or running adds nothing, and the id
/// is that job's.
///
/// # Errors
///
/// Fails on a database error, with [`Error::NotATenant`] for a tenant that is not a
/// tenant id, and with [`Error::NotACode`] for a key that is not a code, before
/// anything is written.
pub async fn enqueue_on<'e, E: SqliteExecutor<'e>>(executor: E, job: &NewJob) -> Result<Uuid> {
    if job
        .tenant
        .as_deref()
        .is_some_and(|tenant| !is_tenant_id(tenant))
    {
        return Err(Error::NotATenant);
    }
    if let Some(key) = job.unique_key.as_deref() {
        check_code(key, "unique key")?;
    }
    let id = Uuid::new_v4();
    let (at, delay) = match job.when {
        When::Now => (None, 0),
        When::At(at) => (Some(at.as_microsecond()), 0),
        When::In(delay) => (None, micros(delay)),
    };
    let query = sqlx::query(if job.unique_key.is_some() {
        ENQUEUE_UNIQUE
    } else {
        ENQUEUE
    })
    .bind(id)
    .bind(&job.name)
    .bind(&job.args)
    .bind(job.max_attempts)
    .bind(job.priority)
    .bind(at)
    .bind(delay)
    .bind(job.timeout.map(millis))
    .bind(job.tenant.as_deref());
    if let Some(key) = job.unique_key.as_deref() {
        Ok(query.bind(key).fetch_one(executor).await?.try_get("id")?)
    } else {
        query.execute(executor).await?;
        Ok(id)
    }
}

/// Add every job in `jobs` on `connection`, returning their ids in order. On a
/// transaction they commit or roll back together; [`Queue::enqueue_all`] opens one.
///
/// # Errors
///
/// Fails on a database error, after which a transaction must be rolled back.
pub async fn enqueue_all_on(
    connection: &mut SqliteConnection,
    jobs: &[NewJob],
) -> Result<Vec<Uuid>> {
    let mut ids = Vec::with_capacity(jobs.len());
    for job in jobs {
        ids.push(enqueue_on(&mut *connection, job).await?);
    }
    Ok(ids)
}

/// Refuse who or why for a person's act unless it is a code.
fn check_code(text: &str, what: &'static str) -> Result<()> {
    if text.is_empty()
        || text.chars().count() > MAX_REASON_CHARS
        || text.chars().any(char::is_control)
    {
        Err(Error::NotACode { what })
    } else {
        Ok(())
    }
}

/// Cancel the job `id` if it is still queued, as an act of `by`, for `reason`, kept in
/// `axa_job_acts` in the caller's transaction, so that the caller's own audit row
/// commits with it. Answers whether it was cancelled. A job already claimed runs on:
/// cancelling cannot stop work in progress. A job in any other state, or no such job,
/// changes nothing and writes no act.
///
/// The job's row stays locked until the transaction ends, so keep it short.
///
/// # Errors
///
/// Fails on a database error, after which the transaction must be rolled back, and
/// with [`Error::NotACode`] when `by` or `reason` is empty, longer than
/// [`MAX_REASON_CHARS`], or has a line break, before anything is written.
pub async fn cancel(
    transaction: &mut Transaction<'_, Sqlite>,
    id: Uuid,
    by: &str,
    reason: &str,
) -> Result<bool> {
    act(transaction, Wanted::Cancel, id, by, reason).await
}

/// Queue the job `id` again, now, for one more try, if it failed or waits for review,
/// as an act of `by`, for `reason`, kept in `axa_job_acts` in the caller's
/// transaction. The act is [`ActKind::Retried`] for a failed job and
/// [`ActKind::Released`] for one in review. Answers whether it was queued again. A job
/// in any other state, or no such job, changes nothing and writes no act. Its last
/// error stays until it next runs.
///
/// # Errors
///
/// Fails on a database error, after which the transaction must be rolled back, and
/// with [`Error::NotACode`] when `by` or `reason` is empty, longer than
/// [`MAX_REASON_CHARS`], or has a line break, before anything is written.
pub async fn retry(
    transaction: &mut Transaction<'_, Sqlite>,
    id: Uuid,
    by: &str,
    reason: &str,
) -> Result<bool> {
    act(transaction, Wanted::Retry, id, by, reason).await
}

/// Which act a person asked for.
#[derive(Clone, Copy)]
enum Wanted {
    Cancel,
    Retry,
}

/// When the act applies to the job's state, change the job and keep the act, all in
/// the caller's transaction. The change is the transaction's first write, so it
/// holds SQLite's write lock from there until the transaction ends.
async fn act(
    transaction: &mut Transaction<'_, Sqlite>,
    wanted: Wanted,
    id: Uuid,
    by: &str,
    reason: &str,
) -> Result<bool> {
    check_code(by, "by")?;
    check_code(reason, "reason")?;
    let tries: &[(&'static str, Option<&str>, &str)] = match wanted {
        Wanted::Cancel => &[(CANCEL, None, "cancelled")],
        Wanted::Retry => &[
            (RETRY_FAILED, Some("failed"), "retried"),
            (RETRY_FAILED, Some("needs_review"), "released"),
        ],
    };
    for (statement, from, act) in tries {
        let mut query = sqlx::query(*statement).bind(id);
        if let Some(from) = from {
            query = query.bind(*from);
        }
        let Some(row) = query.fetch_optional(&mut **transaction).await? else {
            continue;
        };
        let name: String = row.try_get("name")?;
        let attempts: i32 = row.try_get("attempts")?;
        sqlx::query(ACT_KEEP)
            .bind(id)
            .bind(name)
            .bind(*act)
            .bind(attempts)
            .bind(by)
            .bind(reason)
            .execute(&mut **transaction)
            .await?;
        return Ok(true);
    }
    Ok(false)
}

/// What a person did to a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActKind {
    /// Cancelled it while it was queued.
    Cancelled,
    /// Queued it again after it failed.
    Retried,
    /// Released it from review, to run again.
    Released,
}

impl ActKind {
    fn parse(text: &str) -> Result<Self> {
        match text {
            "cancelled" => Ok(Self::Cancelled),
            "retried" => Ok(Self::Retried),
            "released" => Ok(Self::Released),
            _ => Err(Error::Job("an act with an unknown kind".to_string())),
        }
    }
}

/// One act of a person on a job, as `axa_job_acts` keeps it. It outlives the job's
/// row, which retention deletes.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Act {
    /// The act's row. It rises with each act.
    pub id: i64,
    /// The job it was done to.
    pub job_id: Uuid,
    /// The handler that job is for.
    pub job_name: String,
    /// What was done.
    pub kind: ActKind,
    /// The job's runs so far when it was done.
    pub attempts: i32,
    /// Who did it, as the service named them.
    pub by: String,
    /// Why, as a short code.
    pub reason: String,
    /// When, on the database's clock.
    pub at: Timestamp,
}

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Waiting to be claimed, now or at its time.
    Queued,
    /// Claimed by a job loop.
    Running,
    /// Finished.
    Completed,
    /// Out of tries, or given up on.
    Failed,
    /// Cancelled before it ran.
    Cancelled,
    /// Its run lost its claim and was stopped, so it may have run in part or
    /// twice. It is never retried by itself: a person looks, and calls
    /// [`retry()`] to release it.
    NeedsReview,
}

impl JobState {
    fn parse(text: &str) -> Result<Self> {
        match text {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "needs_review" => Ok(Self::NeedsReview),
            _ => Err(Error::Job("a job row with an unknown status".to_string())),
        }
    }
}

/// What the queue knows of one job. Its arguments are left out.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct JobInfo {
    /// The job's id.
    pub id: Uuid,
    /// The handler it is for.
    pub name: String,
    /// Where it is.
    pub state: JobState,
    /// Runs so far, including one in progress.
    pub attempts: i32,
    /// Its own limit on tries, or `None` for the queue's.
    pub max_attempts: Option<i32>,
    /// Its priority.
    pub priority: i16,
    /// Its time limit, if it was given one.
    pub timeout: Option<Duration>,
    /// The tenant it runs for, if any.
    pub tenant: Option<String>,
    /// The key it is unique by while queued or running, if any.
    pub unique_key: Option<String>,
    /// The text of its last failure. For operators only: it is whatever the job's
    /// error said.
    pub error: Option<String>,
    /// When it may run next, or last could.
    pub run_at: Timestamp,
    /// When its latest run started.
    pub started_at: Option<Timestamp>,
    /// When it was queued.
    pub created_at: Timestamp,
    /// When its row last changed.
    pub updated_at: Timestamp,
}

/// How many jobs are in each state, for metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[non_exhaustive]
pub struct Counts {
    /// Queued and due.
    pub ready: i64,
    /// Queued for a later time.
    pub waiting: i64,
    /// Claimed and running.
    pub running: i64,
    /// Finished, and not yet deleted by retention.
    pub completed: i64,
    /// Failed, and not yet deleted by retention.
    pub failed: i64,
    /// Cancelled, and not yet deleted by retention.
    pub cancelled: i64,
    /// Stopped after losing their claim, waiting for a person. Never deleted by
    /// retention.
    pub needs_review: i64,
    /// Seconds the oldest ready job has waited, or zero. A rising value means jobs
    /// are not being run.
    pub oldest_ready_seconds: f64,
}

/// A job queued every interval, or at the times a cron pattern names. See
/// [Schedules](crate#schedules).
#[derive(Clone)]
pub struct Schedule {
    name: String,
    times: Times,
    first: Option<Timestamp>,
    job: NewJob,
}

/// When a schedule is due.
#[derive(Clone)]
enum Times {
    Every(Duration),
    /// The parsed pattern, and the pattern as written, which `croner` would rewrite.
    Cron(Box<croner::Cron>, String),
}

impl Times {
    fn interval(&self) -> Option<Duration> {
        match self {
            Self::Every(interval) => Some(*interval),
            Self::Cron(..) => None,
        }
    }

    fn pattern(&self) -> Option<&str> {
        match self {
            Self::Every(_) => None,
            Self::Cron(_, pattern) => Some(pattern),
        }
    }
}

/// The time after `now` a cron pattern names, in UTC.
fn next_after(cron: &croner::Cron, now: Timestamp) -> Result<Timestamp> {
    cron.find_next_occurrence(&InUtc(now), false)
        .map(|next| next.0)
        .map_err(|error| Error::Job(format!("the cron pattern has no next time: {error}")))
}

/// An instant as `croner` reads it, on a wall clock in UTC, which has no daylight
/// saving, so every wall clock time is one instant. `croner`'s own `jiff` support is for
/// an older `jiff`, so the crate gives it this one time zone itself.
#[derive(Clone, Copy)]
struct InUtc(Timestamp);

impl croner::CronDateTime for InUtc {
    fn to_civil(&self) -> croner::CivilDateTime {
        let time = jiff::tz::TimeZone::UTC.to_datetime(self.0);
        croner::CivilDateTime::new(
            croner::CivilDate::from_parts_unchecked(
                i32::from(time.year()),
                time.month().unsigned_abs().into(),
                time.day().unsigned_abs().into(),
            ),
            croner::CivilTime::from_parts_unchecked(
                time.hour().unsigned_abs().into(),
                time.minute().unsigned_abs().into(),
                time.second().unsigned_abs().into(),
            ),
        )
    }

    fn civil_weekday(&self) -> croner::Weekday {
        let day = jiff::tz::TimeZone::UTC
            .to_datetime(self.0)
            .weekday()
            .to_sunday_zero_offset();
        croner::Weekday::from_days_from_sunday(day.unsigned_abs().into())
    }

    fn resolve_civil(
        &self,
        civil: croner::CivilDateTime,
    ) -> Result<croner::Resolution<Self>, croner::errors::CronError> {
        let part =
            |value: u32| i8::try_from(value).map_err(|_| croner::errors::CronError::InvalidDate);
        let year =
            i16::try_from(civil.year()).map_err(|_| croner::errors::CronError::InvalidDate)?;
        let time = jiff::civil::DateTime::new(
            year,
            part(civil.month())?,
            part(civil.day())?,
            part(civil.hour())?,
            part(civil.minute())?,
            part(civil.second())?,
            0,
        )
        .and_then(|time| jiff::tz::TimeZone::UTC.to_timestamp(time))
        .map_err(|_| croner::errors::CronError::InvalidDate)?;
        Ok(croner::Resolution::Single(Self(time)))
    }

    fn checked_add_seconds(&self, seconds: i64) -> Option<Self> {
        self.0
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .ok()
            .map(Self)
    }

    fn cmp_instant(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl fmt::Debug for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schedule")
            .field("name", &self.name)
            .field("interval", &self.times.interval())
            .field("cron", &self.times.pattern())
            .field("first", &self.first)
            .field("job", &self.job)
            .finish()
    }
}

impl Schedule {
    /// Queue `job` every `interval`, under the schedule name `name`. The name keys
    /// its row, so keep it stable across releases. A delay set on `job` is ignored.
    ///
    /// # Errors
    ///
    /// [`Error::Schedule`] when the name is empty or over 200 characters, or the
    /// interval is under a second: these are mistakes in code, found at boot.
    pub fn every(name: impl Into<String>, interval: Duration, job: NewJob) -> Result<Self> {
        let name = checked_name(name.into())?;
        if interval < Duration::from_secs(1) {
            return Err(Error::Schedule(format!(
                "the schedule {name} needs an interval of at least a second"
            )));
        }
        Ok(Self {
            name,
            times: Times::Every(interval),
            first: None,
            job: NewJob {
                when: When::Now,
                ..job
            },
        })
    }

    /// Queue `job` at each time the cron `pattern` names, in UTC on the database's
    /// clock, under the schedule name `name`. The pattern has five fields, minute to
    /// day of week, or six with seconds first, as `croner` reads them: `"0 2 * * *"` is
    /// 02:00 each day, and `"0 9 * * MON-FRI"` 09:00 each weekday. A delay set on `job`
    /// is ignored. The first time is the pattern's next after the row is made.
    ///
    /// # Errors
    ///
    /// [`Error::Schedule`] when the name is empty or over 200 characters, or the
    /// pattern does not parse or names no time to come: these are mistakes in code,
    /// found at boot.
    pub fn cron(name: impl Into<String>, pattern: &str, job: NewJob) -> Result<Self> {
        let name = checked_name(name.into())?;
        let cron: croner::Cron = pattern.parse().map_err(|error| {
            Error::Schedule(format!(
                "the schedule {name} has a bad cron pattern: {error}"
            ))
        })?;
        if next_after(&cron, Timestamp::now()).is_err() {
            return Err(Error::Schedule(format!(
                "the cron pattern of the schedule {name} names no time to come"
            )));
        }
        Ok(Self {
            name,
            times: Times::Cron(Box::new(cron), pattern.to_string()),
            first: None,
            job: NewJob {
                when: When::Now,
                ..job
            },
        })
    }

    /// The first due time, used only when the schedule's row is made. With an
    /// interval of 24 hours and a first time at 02:00 UTC, the job runs at 02:00
    /// each day. Without one, the first is as soon as the row is made. A cron
    /// schedule's first time is its pattern's, and it ignores this.
    #[must_use]
    pub const fn first_at(mut self, first: Timestamp) -> Self {
        self.first = Some(first);
        self
    }

    /// The schedule's interval, or `None` for a cron schedule.
    #[must_use]
    pub fn interval(&self) -> Option<Duration> {
        self.times.interval()
    }

    /// The schedule's cron pattern, or `None` for one every interval.
    #[must_use]
    pub fn cron_pattern(&self) -> Option<&str> {
        self.times.pattern()
    }

    /// The schedule's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Refuse a schedule name that is empty or over 200 characters.
fn checked_name(name: String) -> Result<String> {
    if name.is_empty() || name.len() > 200 {
        return Err(Error::Schedule(
            "a schedule name must be 1 to 200 characters".to_string(),
        ));
    }
    Ok(name)
}

/// What the table holds of one schedule.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ScheduleInfo {
    /// Its name.
    pub name: String,
    /// Its interval, as the code last had it, or `None` for a cron schedule.
    pub interval: Option<Duration>,
    /// Its cron pattern, as the code last had it, or `None` for one every interval.
    pub cron: Option<String>,
    /// When it is next due.
    pub due_at: Timestamp,
    /// The last job it queued.
    pub last_job: Option<Uuid>,
    /// That job's state, unless retention deleted it.
    pub last_state: Option<JobState>,
    /// That job's last failure text. For operators only.
    pub last_error: Option<String>,
    /// When a live process last had it in code, to within a minute.
    pub seen_at: Timestamp,
    /// When it was archived, as no live process had it in code. An archived
    /// schedule queues nothing, and a schedule of that name in code restores it.
    pub archived_at: Option<Timestamp>,
    /// When its row was made.
    pub created_at: Timestamp,
}

/// Queue tuning.
#[derive(Debug, Clone)]
pub struct QueueOptions {
    /// Concurrent job loops, so the most jobs this process runs at once.
    pub workers: usize,
    /// Sleep between polls of an empty queue, and between schedule ticks.
    pub poll_interval: Duration,
    /// A running job not heard from for this many minutes is assumed lost with
    /// its worker.
    pub stale_minutes: i32,
    /// How often a running job reports in. Keep it well under `stale_minutes`.
    pub heartbeat_interval: Duration,
    /// Tries a job gets, including the first, unless it has its own limit. One
    /// means no retries.
    pub max_attempts: i32,
    /// The wait before the first retry.
    pub retry_backoff: Duration,
    /// Days to keep finished and cancelled rows.
    pub retention_days: i32,
    /// How often to recover stale jobs and delete old rows.
    pub maintenance_interval: Duration,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            workers: 2,
            poll_interval: Duration::from_secs(1),
            stale_minutes: 60,
            heartbeat_interval: Duration::from_mins(1),
            max_attempts: 1,
            retry_backoff: Duration::from_secs(30),
            retention_days: 7,
            maintenance_interval: Duration::from_mins(5),
        }
    }
}

/// A SQLite-backed job queue.
pub struct Queue {
    pool: SqlitePool,
    handlers: HashMap<String, Handler>,
    limits: HashMap<String, Arc<Semaphore>>,
    schedules: Vec<Schedule>,
    options: QueueOptions,
}

impl fmt::Debug for Queue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queue")
            .field("handlers", &self.handlers.len())
            .field("limits", &self.limits.len())
            .field("schedules", &self.schedules.len())
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// A claimed job, as `run_one` holds it.
struct Claimed {
    id: Uuid,
    name: String,
    args: serde_json::Value,
    attempts: i32,
    limit: i32,
    timeout: Option<Duration>,
    tenant: Option<String>,
}

/// How a run ended.
enum Outcome {
    /// The job answered, or failed, timed out, or panicked, with this text.
    Done(Result<(), Failure>),
    /// The run lost its claim and was stopped, for this reason.
    Lost(String),
}

impl Queue {
    /// A queue on `pool`, checking and changing nothing. Call [`Queue::check`] at
    /// boot. Give the pool at least `workers + 2` connections.
    #[must_use]
    pub fn from_pool(pool: SqlitePool, options: QueueOptions) -> Self {
        Self {
            pool,
            handlers: HashMap::new(),
            limits: HashMap::new(),
            schedules: Vec::new(),
            options,
        }
    }

    /// Open a pool sized for the job loops, without touching the schema.
    async fn open(uri: &str, options: QueueOptions) -> Result<Self> {
        let loops = u32::try_from(options.workers).unwrap_or(u32::MAX);
        let connect = uri
            .parse::<SqliteConnectOptions>()?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(loops.checked_add(2).unwrap_or(u32::MAX)) // ARI-5
            .connect_with(connect)
            .await?;
        Ok(Self::from_pool(pool, options))
    }

    /// Connect to tables that a migration made, and change no schema, for a database
    /// role that may not. A migration runs [`SETUP_STATEMENTS`].
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be reached, a table is missing or out of date,
    /// or a trigger of `axa_job_acts` is missing or not set to fire always, so a
    /// service finds out at boot rather than at its first job. There is no way to run
    /// without the triggers.
    pub async fn connect_existing(uri: &str, options: QueueOptions) -> Result<Self> {
        let queue = Self::open(uri, options).await?;
        queue.check().await?;
        Ok(queue)
    }

    /// Connect and create the tables if needed, which runs DDL at boot. Use
    /// [`Queue::connect_existing`] where a service's role may not.
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be reached or the tables not created.
    pub async fn connect(uri: &str, options: QueueOptions) -> Result<Self> {
        let queue = Self::open(uri, options).await?;
        // Two services booting together run setup one at a time: a write
        // transaction from the start holds the file's write lock.
        let mut setup = queue.pool.begin_with(IMMEDIATE).await?;
        run_setup(&mut setup).await?;
        setup.commit().await?;
        Ok(queue)
    }

    /// Check that the tables exist and are up to date, and that the triggers of
    /// `axa_job_acts` are there and set to fire always.
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be reached, a table is missing or out of date,
    /// or a trigger is missing or turned off.
    pub async fn check(&self) -> Result<()> {
        sqlx::query(
            "SELECT attempts, max_attempts, priority, timeout_ms, tenant, unique_key FROM axa_queue LIMIT 0",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "SELECT name, due_at, last_job, interval_ms, cron, seen_at, archived_at FROM axa_schedule LIMIT 0",
        )
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "SELECT id, job_id, job_name, act, attempts, by, reason, at FROM axa_job_acts LIMIT 0",
        )
        .execute(&self.pool)
        .await?;
        let present: Vec<String> = sqlx::query_scalar(PRESENT_TRIGGERS)
            .fetch_all(&self.pool)
            .await?;
        let mut missing: Vec<&str> = ACTS_TRIGGERS
            .into_iter()
            .filter(|name| !present.iter().any(|have| have == name))
            .collect();
        let unique_index: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema
                WHERE type = 'index' AND name = 'axa_queue_unique')",
        )
        .fetch_one(&self.pool)
        .await?;
        if !unique_index {
            missing.push("axa_queue_unique");
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::Database(sqlx::Error::Configuration(
                format!(
                    "the job queue is missing {}: run SETUP_STATEMENTS",
                    missing.join(", ")
                )
                .into(),
            )))
        }
    }

    /// The pool, for a service's own queries or [`enqueue_on`] outside a
    /// transaction.
    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The options this queue runs with.
    #[must_use]
    pub const fn options(&self) -> &QueueOptions {
        &self.options
    }

    /// Run `handler` for jobs named `name`. Its error text is kept on the job's
    /// row. Registering a name again replaces its handler.
    ///
    /// A job queued for a tenant, with [`NewJob::tenant`], is failed rather than run
    /// here, as this handler would run it with no tenant: register a handler that
    /// takes one with [`Queue::handle_with_tenant`].
    pub fn handle<F, Fut, E>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<(), E>> + Send + 'static,
        E: fmt::Display + 'static,
    {
        let handler = Arc::new(handler);
        let run: Handler = Arc::new(move |tenant, args| {
            if tenant.is_some() {
                return Box::pin(std::future::ready(Err(NO_TENANT_HANDLER
                    .to_string()
                    .into())));
            }
            let job = handler(args);
            Box::pin(async move { job.await.map_err(Failure::from_error) })
        });
        self.handlers.insert(name.into(), run);
    }

    /// Run `handler` for jobs named `name`, handing it the job's tenant, or `None` for
    /// a job queued with none, beside its arguments, so that it can open its own
    /// tenant transaction. Claiming stays fair across jobs, not tenants. Registering a
    /// name again replaces its handler.
    pub fn handle_with_tenant<F, Fut, E>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(Option<String>, serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<(), E>> + Send + 'static,
        E: fmt::Display + 'static,
    {
        let handler = Arc::new(handler);
        let run: Handler = Arc::new(move |tenant, args| {
            let job = handler(tenant, args);
            Box::pin(async move { job.await.map_err(Failure::from_error) })
        });
        self.handlers.insert(name.into(), run);
    }

    /// Run at most `jobs` jobs named `name` at once in this process, and at least
    /// one. Other jobs keep the remaining loops busy.
    pub fn limit(&mut self, name: impl Into<String>, jobs: usize) {
        self.limits
            .insert(name.into(), Arc::new(Semaphore::new(jobs.max(1))));
    }

    /// Queue a job on every due time of `schedule`.
    ///
    /// # Errors
    ///
    /// [`Error::Schedule`] when a schedule of that name was added already.
    pub fn schedule(&mut self, schedule: Schedule) -> Result<()> {
        if self.schedules.iter().any(|s| s.name == schedule.name) {
            return Err(Error::Schedule(format!(
                "the schedule {} was added twice",
                schedule.name
            )));
        }
        self.schedules.push(schedule);
        Ok(())
    }

    /// Add a job and return its id.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn enqueue(&self, name: &str, args: serde_json::Value) -> Result<Uuid> {
        enqueue_on(&self.pool, &NewJob::new(name, args)).await
    }

    /// Add a job with its own limit on tries, or the queue's when `None`.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn enqueue_limited(
        &self,
        name: &str,
        args: serde_json::Value,
        max_attempts: Option<i32>,
    ) -> Result<Uuid> {
        enqueue_on(
            &self.pool,
            &NewJob::new(name, args).max_attempts(max_attempts),
        )
        .await
    }

    /// Add `job` and return its id.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn enqueue_job(&self, job: &NewJob) -> Result<Uuid> {
        enqueue_on(&self.pool, job).await
    }

    /// Add every job in `jobs` in one transaction, so all are queued or none.
    ///
    /// # Errors
    ///
    /// Fails on a database error, and then none was queued.
    pub async fn enqueue_all(&self, jobs: &[NewJob]) -> Result<Vec<Uuid>> {
        let mut transaction = self.pool.begin().await?;
        let ids = enqueue_all_on(&mut transaction, jobs).await?;
        transaction.commit().await?;
        Ok(ids)
    }

    /// What the queue knows of the job `id`, or `None` when there is no such job,
    /// as after retention deleted it.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn job(&self, id: Uuid) -> Result<Option<JobInfo>> {
        let Some(row) = sqlx::query(STATUS)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(JobInfo {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            state: JobState::parse(row.try_get::<&str, _>("status")?)?,
            attempts: row.try_get("attempts")?,
            max_attempts: row.try_get("max_attempts")?,
            priority: row.try_get("priority")?,
            timeout: row
                .try_get::<Option<i64>, _>("timeout_ms")?
                .map(from_millis),
            tenant: row.try_get("tenant")?,
            unique_key: row.try_get("unique_key")?,
            error: row.try_get("error")?,
            run_at: at(row.try_get("run_at")?)?,
            started_at: at_or_none(row.try_get("started_at")?)?,
            created_at: at(row.try_get("created_at")?)?,
            updated_at: at(row.try_get("updated_at")?)?,
        }))
    }

    /// Every act a person did to the job `id`, oldest first, for a service's own
    /// review screens. Acts are kept after retention deletes the job's row. A person
    /// acts with [`cancel()`] and [`retry()`], in the caller's transaction.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn acts(&self, id: Uuid) -> Result<Vec<Act>> {
        let rows = sqlx::query(ACTS).bind(id).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                Ok(Act {
                    id: row.try_get("id")?,
                    job_id: row.try_get("job_id")?,
                    job_name: row.try_get("job_name")?,
                    kind: ActKind::parse(row.try_get::<&str, _>("act")?)?,
                    attempts: row.try_get("attempts")?,
                    by: row.try_get("by")?,
                    reason: row.try_get("reason")?,
                    at: at(row.try_get("at")?)?,
                })
            })
            .collect()
    }

    /// How many jobs are in each state.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn counts(&self) -> Result<Counts> {
        let row = sqlx::query(COUNTS).fetch_one(&self.pool).await?;
        Ok(Counts {
            ready: row.try_get("ready")?,
            waiting: row.try_get("waiting")?,
            running: row.try_get("running")?,
            completed: row.try_get("completed")?,
            failed: row.try_get("failed")?,
            cancelled: row.try_get("cancelled")?,
            needs_review: row.try_get("needs_review")?,
            oldest_ready_seconds: row.try_get::<Option<f64>, _>("oldest")?.unwrap_or(0.0),
        })
    }

    /// Check the database connection.
    ///
    /// # Errors
    ///
    /// Fails when the database does not answer.
    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Recover jobs abandoned by a worker that died: queue again those with
    /// attempts left and fail the rest, returning how many were queued again.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn requeue_stale(&self) -> Result<u64> {
        sqlx::query(FAIL_STALE)
            .bind(self.options.stale_minutes)
            .bind(self.options.max_attempts)
            .execute(&self.pool)
            .await?;
        let result = sqlx::query(REQUEUE_STALE)
            .bind(self.options.stale_minutes)
            .bind(self.options.max_attempts)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete finished and cancelled rows older than the retention period,
    /// returning how many went.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn purge_finished(&self) -> Result<u64> {
        let result = sqlx::query(PURGE)
            .bind(self.options.retention_days)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Queue a job for each schedule that is due, returning how many were queued.
    /// [`Queue::run`] calls this on every poll. Any number of processes may call it
    /// at once: each due time is queued once.
    ///
    /// # Errors
    ///
    /// Fails on a database error, which leaves that schedule due for the next tick.
    pub async fn tick_schedules(&self) -> Result<usize> {
        let mut queued: usize = 0;
        for schedule in &self.schedules {
            if self.tick(schedule).await? {
                queued = queued
                    .checked_add(1)
                    .ok_or(Error::Overflow("schedules queued"))?;
            }
        }
        Ok(queued)
    }

    /// One schedule's tick, returning whether it queued a job.
    async fn tick(&self, schedule: &Schedule) -> Result<bool> {
        // Outside the transaction: an upsert locks the row it meets, and replicas
        // would then wait on each other instead of skipping.
        let first = match &schedule.times {
            Times::Every(_) => schedule.first,
            Times::Cron(cron, _) => {
                let now: i64 = sqlx::query_scalar(concat!("SELECT ", now!()))
                    .fetch_one(&self.pool)
                    .await?;
                Some(next_after(cron, at(now)?)?)
            }
        };
        sqlx::query(SCHEDULE_CREATE)
            .bind(&schedule.name)
            .bind(first.map(Timestamp::as_microsecond))
            .bind(schedule.times.interval().map(millis))
            .bind(schedule.times.pattern())
            .execute(&self.pool)
            .await?;
        let mut transaction = self.begin().await?;
        // Another process holding the row is queuing this time already.
        let Some(row) = sqlx::query(SCHEDULE_LOCK)
            .bind(&schedule.name)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Ok(false);
        };
        if row.try_get::<i64, _>("due")? == 0 {
            transaction.commit().await?;
            return Ok(false);
        }
        let busy = match row.try_get::<Option<Uuid>, _>("last_job")? {
            Some(last) => {
                sqlx::query_scalar::<_, i64>(SCHEDULE_BUSY)
                    .bind(last)
                    .fetch_one(&mut *transaction)
                    .await?
                    != 0
            }
            None => false,
        };
        let job = if busy {
            tracing::info!(schedule = %schedule.name, "schedule skipped, its last job has not finished");
            None
        } else {
            Some(enqueue_on(&mut *transaction, &schedule.job).await?)
        };
        match &schedule.times {
            Times::Every(interval) => {
                sqlx::query(SCHEDULE_ADVANCE)
                    .bind(micros(*interval))
                    .bind(job)
                    .bind(&schedule.name)
                    .execute(&mut *transaction)
                    .await?;
            }
            Times::Cron(cron, _) => {
                let next = next_after(cron, at(row.try_get("now")?)?)?;
                sqlx::query(SCHEDULE_ADVANCE_TO)
                    .bind(next.as_microsecond())
                    .bind(job)
                    .bind(&schedule.name)
                    .execute(&mut *transaction)
                    .await?;
            }
        }
        transaction.commit().await?;
        if job.is_some() {
            tracing::debug!(schedule = %schedule.name, "schedule queued its job");
        }
        Ok(job.is_some())
    }

    /// Archive the schedules that no live process has in code, returning how many
    /// were archived. Rows are kept, for audit. Housekeeping runs this.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn archive_schedules(&self) -> Result<u64> {
        let kept = if self.schedules.is_empty() {
            String::new()
        } else {
            format!(" AND name NOT IN ({})", placeholders(self.schedules.len()))
        };
        // Only placeholders are added to the statement, never a value.
        let statement = SCHEDULE_ARCHIVE.replace("{kept}", &kept);
        let mut query = sqlx::query(AssertSqlSafe(statement)).bind(self.options.stale_minutes);
        for schedule in &self.schedules {
            query = query.bind(&schedule.name);
        }
        Ok(query.execute(&self.pool).await?.rows_affected())
    }

    /// Every schedule the table holds, archived or not, with its last job's state.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn schedules(&self) -> Result<Vec<ScheduleInfo>> {
        let rows = sqlx::query(SCHEDULES).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                Ok(ScheduleInfo {
                    name: row.try_get("name")?,
                    interval: row
                        .try_get::<Option<i64>, _>("interval_ms")?
                        .map(from_millis),
                    cron: row.try_get("cron")?,
                    due_at: at(row.try_get("due_at")?)?,
                    last_job: row.try_get("last_job")?,
                    last_state: row
                        .try_get::<Option<&str>, _>("last_status")?
                        .map(JobState::parse)
                        .transpose()?,
                    last_error: row.try_get("last_error")?,
                    seen_at: at(row.try_get("seen_at")?)?,
                    archived_at: at_or_none(row.try_get("archived_at")?)?,
                    created_at: at(row.try_get("created_at")?)?,
                })
            })
            .collect()
    }

    /// One round of housekeeping. Failures are logged, because the next round
    /// can try again.
    async fn maintain(&self) {
        match self.archive_schedules().await {
            Ok(0) => {}
            Ok(archived) => tracing::info!(archived, "archived schedules no longer in code"),
            Err(error) => tracing::error!(%error, "could not archive schedules"),
        }
        if let Err(error) = self.requeue_stale().await {
            tracing::error!(%error, "could not recover stale jobs");
        }
        if let Err(error) = self.purge_finished().await {
            tracing::error!(%error, "could not delete old jobs");
        }
    }

    /// Run `job` to its end in its own task, reporting in as it goes, so a panic
    /// stops only the job. The job is stopped when its time limit passes, when its
    /// claim is found taken, or when this future is dropped.
    async fn supervise(&self, job: JobFuture, claim: &Claimed) -> Outcome {
        let mut job = AbortOnDrop(tokio::spawn(job));
        let every = self.options.heartbeat_interval.max(MIN_HEARTBEAT);
        let Some(give_up) = give_up_after(self.options.stale_minutes, every) else {
            return Outcome::Lost(TIMING.to_string());
        };
        let mut last_heard = tokio::time::Instant::now();
        let Some(mut give_up_at) = last_heard.checked_add(give_up) else {
            return Outcome::Lost(TIMING.to_string());
        };
        // A heartbeat is given at most `every` to answer, and runs beside the job, so
        // a database that hangs holds up neither the job's time limit nor giving up.
        let beat = || {
            tokio::time::timeout(
                every,
                sqlx::query(HEARTBEAT)
                    .bind(claim.id)
                    .bind(claim.attempts)
                    .execute(&self.pool),
            )
        };
        let mut beating = None;
        let Some(mut next_beat) = tokio::time::Instant::now().checked_add(every) else {
            return Outcome::Lost(TIMING.to_string());
        };
        let mut failing = false;
        let limit = match claim.timeout {
            Some(timeout) => match tokio::time::Instant::now().checked_add(timeout) {
                Some(at) => Some(at),
                None => return Outcome::Lost(TIMING.to_string()),
            },
            None => None,
        };
        let deadline = async {
            match limit {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                finished = &mut job.0 => {
                    return Outcome::Done(finished.unwrap_or_else(|_| {
                        Err(Error::Job("the job panicked".to_string()).to_string().into())
                    }));
                }
                () = &mut deadline => {
                    let limit = claim.timeout.unwrap_or(Duration::ZERO);
                    return Outcome::Done(Err(
                        Error::Job(format!("timed out after {}s", limit.as_secs_f64())).to_string().into(),
                    ));
                }
                () = tokio::time::sleep_until(give_up_at), if failing => {
                    return Outcome::Lost(format!(
                        "it could not report in for {}s, near the {} minutes after which another worker may take it",
                        give_up.as_secs(),
                        self.options.stale_minutes
                    ));
                }
                () = tokio::time::sleep_until(next_beat), if beating.is_none() => {
                    beating = Some(Box::pin(beat()));
                }
                answer = async {
                    match beating.as_mut() {
                        Some(beat) => beat.await,
                        None => std::future::pending().await,
                    }
                }, if beating.is_some() => {
                    beating = None;
                    let Some(next) = tokio::time::Instant::now().checked_add(every) else {
                        return Outcome::Lost(TIMING.to_string());
                    };
                    next_beat = next;
                    match answer {
                        Ok(Ok(done)) if done.rows_affected() == 1 => {
                            last_heard = tokio::time::Instant::now();
                            let Some(at) = last_heard.checked_add(give_up) else {
                                return Outcome::Lost(TIMING.to_string());
                            };
                            give_up_at = at;
                            failing = false;
                        }
                        Ok(Ok(_)) => return Outcome::Lost("another worker or recovery took its claim".to_string()),
                        Ok(Err(error)) => {
                            tracing::warn!(id = %claim.id, %error, "job heartbeat failed");
                            failing = true;
                        }
                        Err(_) => {
                            tracing::warn!(id = %claim.id, "job heartbeat did not answer in time");
                            failing = true;
                        }
                    }
                }
            }
        }
    }

    /// Mark a run's job for review, trying until the database answers, as the
    /// run must not be taken for a crash and retried.
    async fn mark_lost(&self, claim: &Claimed, reason: &str) {
        let text = Error::Job(format!("stopped: {reason}")).to_string();
        let every = self.options.heartbeat_interval.max(MIN_HEARTBEAT);
        loop {
            match sqlx::query(MARK_LOST)
                .bind(&text)
                .bind(claim.id)
                .execute(&self.pool)
                .await
            {
                Ok(_) => return,
                Err(error) => {
                    tracing::error!(id = %claim.id, %error, "could not mark a lost job for review");
                    tokio::time::sleep(every).await;
                }
            }
        }
    }

    /// The job names this loop may claim now, holding a place under each limit
    /// that has one free. The places not used are given back when dropped.
    fn claimable(&self) -> (Vec<String>, HashMap<String, OwnedSemaphorePermit>) {
        let mut places = HashMap::new();
        let mut names = Vec::new();
        for name in self.handlers.keys() {
            match self.limits.get(name) {
                None => names.push(name.clone()),
                Some(limit) => {
                    if let Ok(place) = limit.clone().try_acquire_owned() {
                        places.insert(name.clone(), place);
                        names.push(name.clone());
                    }
                }
            }
        }
        (names, places)
    }

    /// A write transaction on the pool, for the queue's own work.
    async fn begin(&self) -> Result<Transaction<'static, Sqlite>> {
        Ok(self.pool.begin_with(IMMEDIATE).await?)
    }

    async fn claim(&self, names: &[String]) -> Result<Option<Claimed>> {
        // Only placeholders are added to the statement, never a value.
        let claim = CLAIM.replace("{names}", &placeholders(names.len()));
        let mut query = sqlx::query(AssertSqlSafe(claim));
        for name in names {
            query = query.bind(name);
        }
        let Some(row) = query.fetch_optional(&self.pool).await? else {
            return Ok(None);
        };
        Ok(Some(Claimed {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            args: row.try_get("args")?,
            attempts: row.try_get("attempts")?,
            limit: row
                .try_get::<Option<i32>, _>("max_attempts")?
                .unwrap_or(self.options.max_attempts),
            timeout: row
                .try_get::<Option<i64>, _>("timeout_ms")?
                .map(from_millis),
            tenant: row.try_get("tenant")?,
        }))
    }

    /// Claim and run one job, returning whether there was one. A job's own
    /// failure is recorded on its row, not returned.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn run_one(&self) -> Result<bool> {
        let (names, mut places) = self.claimable();
        if names.is_empty() {
            return Ok(false);
        }
        let Some(job) = self.claim(&names).await? else {
            return Ok(false);
        };
        // Keep only the place this job takes.
        let _place = places.remove(&job.name);
        drop(places);
        let outcome = match self.handlers.get(&job.name) {
            Some(handler) => {
                let run = handler(job.tenant.clone(), job.args.clone());
                self.supervise(run, &job).await
            }
            None => Outcome::Done(Err(Error::Job(format!("no handler for job {}", job.name))
                .to_string()
                .into())),
        };
        let (id, name, attempts) = (job.id, job.name.as_str(), job.attempts);
        let finished = match outcome {
            Outcome::Lost(reason) => {
                tracing::error!(job = %name, %id, attempts, %reason, "job stopped, it lost its claim");
                self.mark_lost(&job, &reason).await;
                return Ok(true);
            }
            Outcome::Done(Ok(())) => {
                sqlx::query(FINISH)
                    .bind("completed")
                    .bind(None::<String>)
                    .bind(id)
                    .bind(attempts)
                    .execute(&self.pool)
                    .await?
            }
            Outcome::Done(Err(error)) if attempts < job.limit => {
                tracing::warn!(job = %name, %id, attempts, %error, "job failed, will retry");
                let wait = retry_wait(self.options.retry_backoff, attempts, error.retry_in);
                sqlx::query(RETRY)
                    .bind(cut(error.text))
                    .bind(micros(wait))
                    .bind(id)
                    .bind(attempts)
                    .execute(&self.pool)
                    .await?
            }
            Outcome::Done(Err(error)) => {
                tracing::error!(job = %name, %id, attempts, %error, "job failed");
                sqlx::query(FINISH)
                    .bind("failed")
                    .bind(cut(error.text))
                    .bind(id)
                    .bind(attempts)
                    .execute(&self.pool)
                    .await?
            }
        };
        if finished.rows_affected() == 0 {
            // It ran to its end, but its claim had been taken, so the job may be
            // running again elsewhere.
            tracing::error!(job = %name, %id, "job finished after losing its claim");
            self.mark_lost(
                &job,
                "it finished after another worker or recovery took its claim",
            )
            .await;
        }
        Ok(true)
    }

    /// Run job loops, housekeeping, and schedules until `shutdown` changes to
    /// `true`. Jobs in progress finish first, so a caller that must stop by a
    /// time limit drops this future at it, which aborts those jobs and leaves them
    /// for stale recovery. Housekeeping runs straight away, to recover anything a
    /// previous process left behind, and then on a timer.
    pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>) {
        let mut loops = Vec::new();
        loops.push(AbortOnDrop(tokio::spawn(every_until(
            shutdown.clone(),
            self.options.maintenance_interval,
            {
                let queue = self.clone();
                move || {
                    let queue = queue.clone();
                    async move { queue.maintain().await }
                }
            },
        ))));
        if !self.schedules.is_empty() {
            let queue = self.clone();
            loops.push(AbortOnDrop(tokio::spawn(every_until(
                shutdown.clone(),
                self.options.poll_interval,
                move || {
                    let queue = queue.clone();
                    async move {
                        if let Err(error) = queue.tick_schedules().await {
                            tracing::error!(%error, "could not queue scheduled jobs");
                        }
                    }
                },
            ))));
        }
        for _ in 0..self.options.workers.max(1) {
            let queue = self.clone();
            let mut shutdown = shutdown.clone();
            loops.push(AbortOnDrop(tokio::spawn(async move {
                while !*shutdown.borrow() {
                    let worked = queue.run_one().await.unwrap_or_else(|error| {
                        tracing::error!(%error, "job queue error");
                        false
                    });
                    if !worked {
                        tokio::select! {
                            () = tokio::time::sleep(queue.options.poll_interval) => {},
                            _ = shutdown.changed() => {},
                        }
                    }
                }
            })));
        }
        for job_loop in &mut loops {
            let _ = (&mut job_loop.0).await;
        }
    }

    /// The names of the job types this queue handles.
    #[must_use]
    pub fn handled(&self) -> HashSet<&str> {
        self.handlers.keys().map(String::as_str).collect()
    }
}

/// Run each of [`SETUP_STATEMENTS`] as plain SQL on `connection`.
async fn run_setup(connection: &mut SqliteConnection) -> Result<()> {
    for statement in SETUP_STATEMENTS {
        sqlx::raw_sql(statement).execute(&mut *connection).await?;
    }
    Ok(())
}

/// Run `round` now and then every `gap`, until `shutdown` is `true`.
async fn every_until<F, Fut>(mut shutdown: watch::Receiver<bool>, gap: Duration, round: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = ()>,
{
    while !*shutdown.borrow() {
        round().await;
        tokio::select! {
            () = tokio::time::sleep(gap) => {},
            _ = shutdown.changed() => {},
        }
    }
}

/// A task that is aborted when its handle is dropped, so that dropping
/// [`Queue::run`] stops its loops and their jobs instead of leaving them running.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
#[allow(clippy::string_slice, reason = "RCA-SCOPE-4: tests may slice strings")]
mod tests {
    use super::*;

    #[test]
    fn the_time_to_give_up_is_two_heartbeats_early_and_at_half_at_the_latest() {
        let beat = Duration::from_secs(30);
        assert_eq!(give_up_after(60, beat), Some(Duration::from_mins(59)));
        // Two heartbeats of the stale time or more: half of it.
        assert_eq!(
            give_up_after(1, Duration::from_secs(40)),
            Some(Duration::from_secs(30))
        );
        // A heartbeat too long to double: half of it too.
        assert_eq!(
            give_up_after(60, Duration::MAX),
            Some(Duration::from_mins(30))
        );
        // A stale time that does not fit, or is negative, has no time.
        assert_eq!(give_up_after(-1, beat), None);
        assert!(give_up_after(i32::MAX, Duration::ZERO).is_some());
    }

    #[test]
    fn a_figure_too_large_to_compute_is_named_in_its_error() {
        let error = Error::Overflow("schedules queued");
        assert_eq!(
            error.to_string(),
            "schedules queued: the value is too large to compute"
        );
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn a_stored_word_that_is_not_a_known_state_or_act_is_refused() {
        assert!(JobState::parse("cancelled").is_ok());
        assert!(ActKind::parse("released").is_ok());
        let error = JobState::parse("lost").unwrap_err().to_string();
        assert!(error.contains("unknown status"), "{error}");
        let error = ActKind::parse("lost").unwrap_err().to_string();
        assert!(error.contains("unknown kind"), "{error}");
    }

    #[test]
    fn errors_say_what_is_wrong_with_a_tenant() {
        let error = Error::NotATenant.to_string();
        assert!(error.contains("lower-case letters"), "{error}");
    }

    #[test]
    fn each_retry_waits_twice_as_long_up_to_a_limit() {
        let base = Duration::from_secs(30);
        assert_eq!(backoff(base, 1), base);
        assert_eq!(backoff(base, 2), base * 2);
        assert_eq!(backoff(base, 3), base * 4);
        assert_eq!(backoff(base, 1_000), MAX_BACKOFF);
    }

    #[test]
    fn a_retry_waits_a_jittered_backoff_or_the_delay_the_job_chose() {
        let base = Duration::from_secs(30);
        for _ in 0..200 {
            let wait = retry_wait(base, 2, None);
            assert!(wait >= base && wait <= base * 2, "{wait:?}");
        }
        let waits: HashSet<_> = (0..20).map(|_| retry_wait(base, 1, None)).collect();
        assert!(waits.len() > 1, "the waits are spread");
        assert_eq!(retry_wait(Duration::ZERO, 3, None), Duration::ZERO);
        let chosen = Some(Duration::from_secs(90));
        assert_eq!(retry_wait(base, 1, chosen), Duration::from_secs(90));
        assert_eq!(retry_wait(base, 1, Some(Duration::ZERO)), MIN_RETRY_AFTER);
        assert_eq!(
            retry_wait(base, 1, Some(Duration::from_hours(9))),
            MAX_BACKOFF
        );
    }

    #[test]
    fn a_retry_after_error_carries_its_delay_and_others_none() {
        let chosen = Failure::from_error(RetryAfter::new(Duration::from_secs(7), "busy"));
        assert_eq!(
            (chosen.text.as_str(), chosen.retry_in),
            ("busy", Some(Duration::from_secs(7)))
        );
        let boxed: Box<dyn std::error::Error + Send + Sync> =
            Box::new(RetryAfter::new(Duration::from_secs(8), "boxed"));
        assert_eq!(
            Failure::from_error(boxed).retry_in,
            Some(Duration::from_secs(8))
        );
        let plain = Failure::from_error("broken".to_string());
        assert_eq!((plain.text.as_str(), plain.retry_in), ("broken", None));
    }

    #[test]
    fn the_debug_text_leaves_out_arguments() {
        let job = NewJob::new("Report", serde_json::json!({ "nhs": "943 476 5919" }));
        let schedule = Schedule::every("nightly", Duration::from_hours(1), job.clone()).unwrap();
        let cron = Schedule::cron("at-two", "0 2 * * *", job.clone()).unwrap();
        for text in [
            format!("{job:?}"),
            format!("{schedule:?}"),
            format!("{cron:?}"),
        ] {
            assert!(text.contains("Report"), "{text}");
            assert!(!text.contains("943"), "{text}");
        }
    }

    #[test]
    fn a_by_or_reason_that_is_not_a_code_is_refused() {
        assert!(check_code("ops:alice", "by").is_ok());
        assert!(check_code(&"é".repeat(MAX_REASON_CHARS), "reason").is_ok());
        for bad in [
            String::new(),
            "é".repeat(MAX_REASON_CHARS + 1),
            "two\nlines".to_string(),
            "a\ttab".to_string(),
        ] {
            let error = check_code(&bad, "reason").unwrap_err();
            assert!(matches!(error, Error::NotACode { what: "reason" }));
            assert!(!error.to_string().contains("lines"), "the text is left out");
        }
    }

    #[test]
    fn a_long_error_is_cut_on_a_character_boundary() {
        // One byte first, so byte MAX_ERROR falls inside an `é`, and the cut steps back.
        let text = format!("a{}", "é".repeat(MAX_ERROR));
        let kept = cut(text);
        assert_eq!(kept.len(), MAX_ERROR - 1);
        assert!(kept[1..].chars().all(|c| c == 'é'));
        assert_eq!(cut("short".to_string()), "short");
    }

    #[test]
    fn a_schedule_under_a_second_is_refused_at_boot() {
        let error = Schedule::every(
            "fast",
            Duration::from_millis(10),
            NewJob::new("J", serde_json::Value::Null),
        )
        .unwrap_err();
        assert!(matches!(&error, Error::Schedule(_)), "{error:?}");
        assert!(error.to_string().contains("at least a second"), "{error}");
    }

    #[test]
    fn a_bad_cron_pattern_is_refused_at_boot() {
        let error = Schedule::cron(
            "bad",
            "61 * * * *",
            NewJob::new("J", serde_json::Value::Null),
        )
        .unwrap_err();
        assert!(matches!(&error, Error::Schedule(_)), "{error:?}");
        assert!(error.to_string().contains("bad cron pattern"), "{error}");
    }

    #[test]
    fn a_cron_schedule_names_its_next_times_in_utc() {
        let schedule = Schedule::cron(
            "weekdays",
            "0 9 * * MON-FRI",
            NewJob::new("J", serde_json::Value::Null),
        )
        .unwrap();
        assert_eq!(schedule.cron_pattern(), Some("0 9 * * MON-FRI"));
        assert_eq!(schedule.interval(), None);
        let Times::Cron(cron, _) = &schedule.times else {
            panic!("a cron schedule")
        };
        // Friday 2026-11-13 09:00 UTC is due, so the next after it is Monday's.
        let friday: Timestamp = "2026-11-13T09:00:00Z".parse().unwrap();
        let monday: Timestamp = "2026-11-16T09:00:00Z".parse().unwrap();
        assert_eq!(next_after(cron, friday).unwrap(), monday);
        assert_eq!(
            Schedule::every(
                "e",
                Duration::from_secs(5),
                NewJob::new("J", serde_json::Value::Null)
            )
            .unwrap()
            .interval(),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn a_cron_schedule_counts_in_utc_across_a_month_end_and_a_clock_change() {
        let next = |pattern: &str, after: &str| {
            let cron: croner::Cron = pattern.parse().unwrap();
            next_after(&cron, after.parse().unwrap())
                .unwrap()
                .to_string()
        };
        // The 31st skips a month of 30 days, and the year end is an ordinary midnight.
        assert_eq!(
            next("0 0 31 * *", "2026-11-15T00:00:00Z"),
            "2026-12-31T00:00:00Z"
        );
        assert_eq!(
            next("30 23 * * *", "2026-12-31T23:30:00Z"),
            "2027-01-01T23:30:00Z"
        );
        assert_eq!(
            next("0 12 29 2 *", "2026-03-01T00:00:00Z"),
            "2028-02-29T12:00:00Z"
        );
        // Clocks in Europe go forward at 01:00 UTC on 2026-03-29 and back at 01:00 UTC
        // on 2026-10-25, and in the US back on 2026-11-01. UTC has no such change, so
        // 01:30 comes once each day, 24 hours apart.
        assert_eq!(
            next("30 1 * * *", "2026-03-28T01:30:00Z"),
            "2026-03-29T01:30:00Z"
        );
        assert_eq!(
            next("30 1 * * *", "2026-10-25T01:30:00Z"),
            "2026-10-26T01:30:00Z"
        );
        assert_eq!(
            next("0 6 * * *", "2026-11-01T06:00:00Z"),
            "2026-11-02T06:00:00Z"
        );
        // A time part way through a second goes on to the next whole one.
        assert_eq!(
            next("* * * * * *", "2026-11-15T00:00:00.5Z"),
            "2026-11-15T00:00:01Z"
        );
    }

    #[test]
    fn a_schedule_with_no_name_is_refused_at_boot() {
        let error = Schedule::every(
            "",
            Duration::from_secs(1),
            NewJob::new("J", serde_json::Value::Null),
        )
        .unwrap_err();
        assert!(matches!(&error, Error::Schedule(_)), "{error:?}");
        assert!(error.to_string().contains("1 to 200 characters"), "{error}");
    }

    #[tokio::test]
    async fn a_schedule_added_twice_is_refused_at_boot() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let mut queue = Queue::from_pool(pool, QueueOptions::default());
        let job = NewJob::new("J", serde_json::Value::Null);
        queue
            .schedule(Schedule::every("a", Duration::from_secs(5), job.clone()).unwrap())
            .unwrap();
        let error = queue
            .schedule(Schedule::every("a", Duration::from_secs(9), job).unwrap())
            .unwrap_err();
        assert!(error.to_string().contains("added twice"), "{error}");
    }

    #[test]
    fn a_schedule_ignores_a_delay_on_its_job() {
        let job = NewJob::new("J", serde_json::Value::Null).run_in(Duration::from_hours(1));
        let schedule = Schedule::every("s", Duration::from_secs(5), job).unwrap();
        assert_eq!(schedule.job.when, When::Now);
    }

    #[test]
    fn an_error_keeps_what_caused_it_and_only_a_database_or_encoding_error_has_a_source() {
        use std::error::Error as _;
        let encoding = serde_json::from_str::<u8>("x").unwrap_err();
        let text = encoding.to_string();
        let error = Error::from(encoding);
        assert!(matches!(error, Error::Encode(_)));
        assert_eq!(error.to_string(), text);
        assert_eq!(error.source().map(ToString::to_string), Some(text));
        let database = Error::Database(sqlx::Error::RowNotFound);
        assert!(database.source().is_some());
        for none in [
            Error::Job("it broke".into()),
            Error::NotACode { what: "by" },
            Error::NotATenant,
        ] {
            assert!(none.source().is_none(), "{none}");
        }
    }

    #[test]
    fn a_cron_pattern_that_names_no_time_to_come_is_refused_at_boot() {
        // The 30th of February.
        let error = Schedule::cron(
            "never",
            "0 0 30 2 *",
            NewJob::new("J", serde_json::Value::Null),
        )
        .unwrap_err();
        assert!(matches!(&error, Error::Schedule(_)), "{error:?}");
        assert!(
            error.to_string().contains("names no time to come"),
            "{error}"
        );
    }

    #[test]
    fn a_job_and_a_schedule_say_their_names_and_a_failure_its_text() {
        let job = NewJob::new("SendMail", serde_json::Value::Null);
        assert_eq!(job.name(), "SendMail");
        let schedule = Schedule::every("hourly", Duration::from_hours(1), job).unwrap();
        assert_eq!(schedule.name(), "hourly");
        assert_eq!(
            Failure::from("the receiver is busy".to_string()).to_string(),
            "the receiver is busy"
        );
    }

    #[tokio::test]
    async fn a_queue_shows_how_much_it_holds_and_its_options_and_never_its_pool() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite:///axa-no-such-folder/secret-password.sqlite3")
            .unwrap();
        let options = QueueOptions {
            workers: 3,
            ..QueueOptions::default()
        };
        let mut queue = Queue::from_pool(pool, options);
        queue.handle("A", |_: serde_json::Value| async { Ok::<(), String>(()) });
        queue.limit("A", 1);
        queue
            .schedule(
                Schedule::every(
                    "hourly",
                    Duration::from_hours(1),
                    NewJob::new("A", serde_json::Value::Null),
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(queue.options().workers, 3);
        let shown = format!("{queue:?}");
        assert!(
            shown.starts_with("Queue { handlers: 1, limits: 1, schedules: 1, options: "),
            "{shown}"
        );
        assert!(!shown.contains("secret-password"), "{shown}");
    }
}
