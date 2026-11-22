//! A Postgres job queue, worker pool, and recurring schedules.
//!
//! Jobs live in one `axa_queue` table. Each job loop claims the ready job with the
//! highest priority, oldest first, in a single statement using `FOR UPDATE SKIP
//! LOCKED`, so two loops can never claim the same row and no broker is needed.
//!
//! A job queued is announced with `NOTIFY axa_queue` when its transaction commits,
//! and [`Queue::run`] listens on one connection of its pool, so an idle loop starts
//! a job at once rather than at its next poll. Polling every
//! [`QueueOptions::poll_interval`] stays, for a job whose time comes, and for any
//! notice lost while the listener reconnects.
//!
//! ```no_run
//! use std::time::Duration;
//! use axa_postgres_job_queue::{NewJob, Queue, QueueOptions, Schedule};
//! use tokio::sync::watch;
//!
//! # async fn example(uri: &str) -> axa_postgres_job_queue::Result<()> {
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
//! # async fn example(queue: &axa_postgres_job_queue::Queue, id: uuid::Uuid) -> axa_postgres_job_queue::Result<()> {
//! let mut transaction = queue.pool().begin().await?;
//! let cancelled = axa_postgres_job_queue::cancel(&mut transaction, id, "ops:alice", "customer_asked").await?;
//! // ... write the service's own audit row on the same transaction ...
//! transaction.commit().await?;
//! # let _ = cancelled;
//! # Ok(())
//! # }
//! ```
//!
//! `by` and `reason` are codes, not content: at most [`MAX_REASON_CHARS`] characters
//! with no line breaks, and the table refuses anything else. `axa_job_acts` is only
//! ever inserted into, and its triggers refuse `UPDATE`, `DELETE`, and `TRUNCATE`,
//! whoever sends them. An act names its job by id, with no foreign key, so it outlives
//! the job's row. [`Queue::acts`] reads a job's acts.
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
//! The `args` column is plain JSONB, and finished rows are deleted after
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
    PgConnection, PgExecutor, PgPool, Postgres, Row, Transaction,
    postgres::{PgPoolOptions, PgRow},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};
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
            Self::Job(_) | Self::NotACode { .. } | Self::NotATenant | Self::Schedule(_) => None,
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

const SETUP: &str = "CREATE TABLE IF NOT EXISTS axa_queue (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    args JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    error TEXT,
    run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
)";
const SETUP_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS axa_queue_ready ON axa_queue (run_at) WHERE status = 'queued'";
/// Tables created by Axa 0.1.0 have no `attempts` column.
const SETUP_ATTEMPTS: &str =
    "ALTER TABLE axa_queue ADD COLUMN IF NOT EXISTS attempts INTEGER NOT NULL DEFAULT 0";
/// A job's own limit on tries, when it has one.
const SETUP_MAX_ATTEMPTS: &str =
    "ALTER TABLE axa_queue ADD COLUMN IF NOT EXISTS max_attempts INTEGER";
/// Higher runs first. Tables created before axa-postgres-job-queue have none.
const SETUP_PRIORITY: &str =
    "ALTER TABLE axa_queue ADD COLUMN IF NOT EXISTS priority SMALLINT NOT NULL DEFAULT 0";
/// A job's own time limit, when it has one.
const SETUP_TIMEOUT: &str = "ALTER TABLE axa_queue ADD COLUMN IF NOT EXISTS timeout_ms BIGINT";
/// The tenant a job runs for, when it has one, as an auth crate's `Tenant` names it.
const SETUP_TENANT: &str = "ALTER TABLE axa_queue ADD COLUMN IF NOT EXISTS tenant TEXT
    CONSTRAINT axa_queue_tenant_id CHECK (tenant ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$')";
/// The key a job is unique by while it is queued or running, when it has one.
const SETUP_UNIQUE_KEY: &str = "ALTER TABLE axa_queue ADD COLUMN IF NOT EXISTS unique_key TEXT
    CONSTRAINT axa_queue_unique_key_code
    CHECK (length(unique_key) BETWEEN 1 AND 200 AND unique_key !~ '[[:cntrl:]]')";
/// At most one job queued or running per name, tenant, and key.
const SETUP_UNIQUE_INDEX: &str = "CREATE UNIQUE INDEX IF NOT EXISTS axa_queue_unique
    ON axa_queue (name, (COALESCE(tenant, '')), unique_key)
    WHERE unique_key IS NOT NULL AND status IN ('queued', 'running')";
const SETUP_CLAIM_INDEX: &str = "CREATE INDEX IF NOT EXISTS axa_queue_claim
    ON axa_queue (priority DESC, run_at) WHERE status = 'queued'";
const SETUP_SCHEDULE: &str = "CREATE TABLE IF NOT EXISTS axa_schedule (
    name TEXT PRIMARY KEY,
    due_at TIMESTAMPTZ NOT NULL,
    last_job UUID,
    interval_ms BIGINT,
    seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
)";
/// A person's acts on jobs. This table is a record (P-7): it is only ever inserted
/// into, and its triggers refuse every change. A job row is state, and retention
/// deletes it, so an act names its job by id with no foreign key, and keeps the job's
/// name and attempts as they were when the act was done.
/// A schedule's cron pattern, when it has one instead of an interval.
const SETUP_SCHEDULE_CRON: &str = "ALTER TABLE axa_schedule ADD COLUMN IF NOT EXISTS cron TEXT";
const SETUP_ACTS: &str = "CREATE TABLE IF NOT EXISTS axa_job_acts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job_id UUID NOT NULL,
    job_name TEXT NOT NULL,
    act TEXT NOT NULL CHECK (act IN ('cancelled', 'retried', 'released')),
    attempts INTEGER NOT NULL,
    by TEXT NOT NULL CHECK (length(by) BETWEEN 1 AND 200 AND by !~ '[[:cntrl:]]'),
    reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 200 AND reason !~ '[[:cntrl:]]'),
    at TIMESTAMPTZ NOT NULL DEFAULT now()
)";
const SETUP_ACTS_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS axa_job_acts_job ON axa_job_acts (job_id, id)";
/// Refuses whatever fired it, whoever sent it.
const SETUP_ACTS_REFUSE: &str = "CREATE OR REPLACE FUNCTION axa_job_acts_refuse() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% on % is refused: acts on jobs are only ever added to',
        TG_OP, TG_TABLE_NAME;
END
$$";
const SETUP_ACTS_ROWS: &str = "CREATE OR REPLACE TRIGGER axa_job_acts_refuse
    BEFORE UPDATE OR DELETE ON axa_job_acts
    FOR EACH ROW EXECUTE FUNCTION axa_job_acts_refuse()";
const SETUP_ACTS_TRUNCATE: &str = "CREATE OR REPLACE TRIGGER axa_job_acts_refuse_truncate
    BEFORE TRUNCATE ON axa_job_acts
    FOR EACH STATEMENT EXECUTE FUNCTION axa_job_acts_refuse()";
/// `ENABLE ALWAYS` keeps the refusals on when `session_replication_role` is
/// `replica`, which otherwise turns triggers off.
const SETUP_ACTS_ALWAYS: &str = "ALTER TABLE axa_job_acts
    ENABLE ALWAYS TRIGGER axa_job_acts_refuse,
    ENABLE ALWAYS TRIGGER axa_job_acts_refuse_truncate";
/// The channel a queued job is announced on.
pub const NOTIFY_CHANNEL: &str = "axa_queue";
/// Announces new jobs once per statement, delivered when the transaction commits.
const SETUP_NOTIFY_FUNCTION: &str = "CREATE OR REPLACE FUNCTION axa_queue_notify() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('axa_queue', '');
    RETURN NULL;
END
$$";
const SETUP_NOTIFY: &str = "CREATE OR REPLACE TRIGGER axa_queue_notify
    AFTER INSERT ON axa_queue
    FOR EACH STATEMENT EXECUTE FUNCTION axa_queue_notify()";
/// The refusing triggers [`Queue::check`] requires, each set to fire always.
const ACTS_TRIGGERS: [&str; 2] = ["axa_job_acts_refuse", "axa_job_acts_refuse_truncate"];
const MISSING_TRIGGERS: &str = "SELECT want FROM unnest($1::text[]) AS want
    WHERE NOT EXISTS (
        SELECT 1 FROM pg_trigger t
        WHERE t.tgrelid = to_regclass('axa_job_acts') AND t.tgname = want
          AND t.tgenabled::text = 'A'
    )";
/// What a job queued for a tenant fails with when its handler takes no tenant.
const NO_TENANT_HANDLER: &str =
    "the job has a tenant, and its handler takes none: register it with handle_with_tenant";

/// Advisory lock key for table setup: `axa_q` as bytes.
const SETUP_LOCK: i64 = 0x0061_7861_5F71;

const ENQUEUE: &str = "INSERT INTO axa_queue
    (id, name, args, max_attempts, priority, run_at, timeout_ms, tenant)
    VALUES ($1, $2, $3, $4, $5, COALESCE($6, now() + make_interval(secs => $7)), $8, $9)";
/// Queue a job with a key, or find the one of its name, tenant, and key already
/// queued or running. `DO UPDATE`, which changes nothing, rather than `DO NOTHING`,
/// because it returns the job there even when it committed after this statement
/// began, as a job queued in another transaction may have. `inserted` tells which.
const ENQUEUE_UNIQUE: &str = "INSERT INTO axa_queue
    (id, name, args, max_attempts, priority, run_at, timeout_ms, tenant, unique_key)
    VALUES ($1, $2, $3, $4, $5, COALESCE($6, now() + make_interval(secs => $7)), $8, $9, $10)
    ON CONFLICT (name, (COALESCE(tenant, '')), unique_key)
        WHERE unique_key IS NOT NULL AND status IN ('queued', 'running')
    DO UPDATE SET unique_key = EXCLUDED.unique_key
    RETURNING id";
const CLAIM: &str = "UPDATE axa_queue
    SET status = 'running', started_at = now(), updated_at = now(), attempts = attempts + 1
    WHERE id = (
        SELECT id FROM axa_queue
        WHERE status = 'queued' AND run_at <= now() AND name = ANY($1)
        ORDER BY priority DESC, run_at
        FOR UPDATE SKIP LOCKED
        LIMIT 1
    )
    RETURNING id, name, args, attempts, max_attempts, timeout_ms, tenant";
/// `attempts` identifies this run of the job, so a run that was given up on
/// finds no row here and changes nothing.
const HEARTBEAT: &str = "UPDATE axa_queue SET updated_at = now()
    WHERE id = $1 AND attempts = $2 AND status = 'running'";
const FINISH: &str = "UPDATE axa_queue SET status = $3, error = $4, updated_at = now()
    WHERE id = $1 AND attempts = $2 AND status = 'running'";
const RETRY: &str = "UPDATE axa_queue
    SET status = 'queued', error = $3, started_at = NULL, updated_at = now(),
        run_at = now() + make_interval(secs => $4)
    WHERE id = $1 AND attempts = $2 AND status = 'running'";
const REQUEUE_STALE: &str = "UPDATE axa_queue
    SET status = 'queued', started_at = NULL, updated_at = now()
    WHERE status = 'running' AND updated_at < now() - make_interval(mins => $1)
      AND attempts < COALESCE(max_attempts, $2)";
const FAIL_STALE: &str = "UPDATE axa_queue
    SET status = 'failed', error = 'abandoned by its worker', updated_at = now()
    WHERE status = 'running' AND updated_at < now() - make_interval(mins => $1)
      AND attempts >= COALESCE(max_attempts, $2)";
const PURGE: &str = "DELETE FROM axa_queue
    WHERE status IN ('completed', 'failed', 'cancelled')
      AND updated_at < now() - make_interval(days => $1)";
const STATUS: &str = "SELECT id, name, status, attempts, max_attempts, priority, error,
    timeout_ms, tenant, unique_key, run_at, started_at, created_at, updated_at FROM axa_queue WHERE id = $1";
/// A run that lost its claim stops, and its job waits for a person. Another run
/// may hold the row by now, and this stops that one too, at its next report.
/// A row already completed is left alone.
const MARK_LOST: &str =
    "UPDATE axa_queue SET status = 'needs_review', error = $2, updated_at = now()
    WHERE id = $1 AND status IN ('queued', 'running', 'failed')";
/// Cancel a queued job and keep the act, in one statement. A job in any other state
/// changes nothing and writes no act.
const CANCEL: &str = "WITH changed AS (
        UPDATE axa_queue SET status = 'cancelled', updated_at = now()
        WHERE id = $1 AND status = 'queued'
        RETURNING id, name, attempts
    )
    INSERT INTO axa_job_acts (job_id, job_name, act, attempts, by, reason)
    SELECT id, name, 'cancelled', attempts, $2, $3 FROM changed";
/// A retried job gets one more try. `attempts` keeps rising, as it identifies
/// each run, so a run given up on still cannot write over the new one. The act is
/// `released` for a job in review and `retried` for a failed one, and is kept in the
/// same statement.
const RETRY_FAILED: &str = "WITH target AS (
        SELECT id, status FROM axa_queue
        WHERE id = $1 AND status IN ('failed', 'needs_review')
          AND NOT EXISTS (
              SELECT 1 FROM axa_queue other
              WHERE other.unique_key = axa_queue.unique_key
                AND other.name = axa_queue.name
                AND COALESCE(other.tenant, '') = COALESCE(axa_queue.tenant, '')
                AND other.status IN ('queued', 'running')
          )
        FOR UPDATE
    ), changed AS (
        UPDATE axa_queue q
        SET status = 'queued', run_at = now(), started_at = NULL, updated_at = now(),
            max_attempts = q.attempts + 1
        FROM target WHERE q.id = target.id
        RETURNING q.id, q.name, q.attempts, target.status AS was
    )
    INSERT INTO axa_job_acts (job_id, job_name, act, attempts, by, reason)
    SELECT id, name, CASE was WHEN 'needs_review' THEN 'released' ELSE 'retried' END,
        attempts, $2, $3
    FROM changed";
const ACTS: &str = "SELECT id, job_id, job_name, act, attempts, by, reason, at
    FROM axa_job_acts WHERE job_id = $1 ORDER BY id";
const COUNTS: &str = "SELECT
    count(*) FILTER (WHERE status = 'queued' AND run_at <= now()) AS ready,
    count(*) FILTER (WHERE status = 'queued' AND run_at > now()) AS waiting,
    count(*) FILTER (WHERE status = 'running') AS running,
    count(*) FILTER (WHERE status = 'completed') AS completed,
    count(*) FILTER (WHERE status = 'failed') AS failed,
    count(*) FILTER (WHERE status = 'cancelled') AS cancelled,
    count(*) FILTER (WHERE status = 'needs_review') AS needs_review,
    EXTRACT(EPOCH FROM now() - min(run_at) FILTER (WHERE status = 'queued' AND run_at <= now()))::float8 AS oldest
    FROM axa_queue";
/// Make the row, or say this schedule is still in code: restore it if it was
/// archived, and note the interval or pattern. A changed pattern takes its next time,
/// `$5`, from the new pattern. `seen_at` moves at most once a minute, so a tick does
/// not write each poll. A cron schedule's first time is `$2`, its next after now.
const SCHEDULE_CREATE: &str = "INSERT INTO axa_schedule (name, due_at, interval_ms, cron)
    VALUES ($1, COALESCE($2, now()), $3, $4)
    ON CONFLICT (name) DO UPDATE
    SET seen_at = now(), archived_at = NULL, interval_ms = EXCLUDED.interval_ms,
        cron = EXCLUDED.cron,
        due_at = CASE WHEN EXCLUDED.cron IS NOT NULL
                       AND axa_schedule.cron IS DISTINCT FROM EXCLUDED.cron
                  THEN EXCLUDED.due_at ELSE axa_schedule.due_at END
    WHERE axa_schedule.archived_at IS NOT NULL
       OR axa_schedule.interval_ms IS DISTINCT FROM EXCLUDED.interval_ms
       OR axa_schedule.cron IS DISTINCT FROM EXCLUDED.cron
       OR axa_schedule.seen_at < now() - interval '1 minute'";
const SCHEDULE_LOCK: &str =
    "SELECT last_job, due_at <= now() AS due, now() AS now FROM axa_schedule
    WHERE name = $1 AND archived_at IS NULL FOR UPDATE SKIP LOCKED";
/// Archive schedules no live process has in code. A row any process still ticks
/// is seen within a minute, so one unseen for `stale_minutes` is in none, and an
/// old replica in a rolling deploy keeps its schedules until it stops.
const SCHEDULE_ARCHIVE: &str = "UPDATE axa_schedule SET archived_at = now(), updated_at = now()
    WHERE archived_at IS NULL AND name <> ALL($1)
      AND seen_at < now() - make_interval(mins => $2)";
const SCHEDULES: &str = "SELECT s.name, s.interval_ms, s.cron, s.due_at, s.last_job, s.seen_at,
    s.archived_at, s.created_at, q.status AS last_status, q.error AS last_error
    FROM axa_schedule s LEFT JOIN axa_queue q ON q.id = s.last_job ORDER BY s.name";
const SCHEDULE_BUSY: &str = "SELECT EXISTS (SELECT 1 FROM axa_queue
    WHERE id = $1 AND status IN ('queued', 'running'))";
/// The next due time after now, a whole number of intervals from the last, on the
/// database's clock. One expression gives no drift, one catch-up, and no stacking.
const SCHEDULE_ADVANCE: &str = "UPDATE axa_schedule
    SET due_at = due_at + make_interval(secs => $2)
            * (floor(EXTRACT(EPOCH FROM now() - due_at)::float8 / $2) + 1),
        last_job = COALESCE($3, last_job), updated_at = now()
    WHERE name = $1";
/// A cron schedule's next due time, `$2`, the pattern's next after now.
const SCHEDULE_ADVANCE_TO: &str = "UPDATE axa_schedule
    SET due_at = $2, last_job = COALESCE($3, last_job), updated_at = now()
    WHERE name = $1";

/// The statements that make the queue's tables, in order, each safe to repeat. A
/// service whose role may not change the schema runs these in a migration, and calls
/// [`Queue::connect_existing`]. They need Postgres 14 or later.
pub const SETUP_STATEMENTS: [&str; 20] = [
    SETUP,
    SETUP_ATTEMPTS,
    SETUP_MAX_ATTEMPTS,
    SETUP_INDEX,
    SETUP_PRIORITY,
    SETUP_TIMEOUT,
    SETUP_TENANT,
    SETUP_UNIQUE_KEY,
    SETUP_UNIQUE_INDEX,
    SETUP_CLAIM_INDEX,
    SETUP_SCHEDULE,
    SETUP_SCHEDULE_CRON,
    SETUP_ACTS,
    SETUP_ACTS_INDEX,
    SETUP_ACTS_REFUSE,
    SETUP_ACTS_ROWS,
    SETUP_ACTS_TRUNCATE,
    SETUP_ACTS_ALWAYS,
    SETUP_NOTIFY_FUNCTION,
    SETUP_NOTIFY,
];

/// The longest `by` or `reason` kept on an act, in characters.
pub const MAX_REASON_CHARS: usize = 200;

/// The shortest gap between heartbeats, so a zero setting cannot spin.
const MIN_HEARTBEAT: Duration = Duration::from_millis(20);

/// The longest wait between attempts.
const MAX_BACKOFF: Duration = Duration::from_hours(1);

/// The longest error text kept on a row.
const MAX_ERROR: usize = 1024;

/// How long to wait before the attempt after `attempts`, doubling each time.
#[must_use]
pub fn backoff(base: Duration, attempts: i32) -> Duration {
    let doublings = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(16);
    base.saturating_mul(1 << doublings).min(MAX_BACKOFF)
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
        let any = &error as &dyn std::any::Any;
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
    #[allow(clippy::cast_precision_loss)]
    let fraction = (random % 1_000_000) as f64 / 1_000_000.0;
    wait.mul_f64(0.5 + fraction / 2.0)
}

/// How long to wait before the attempt after `attempts`: the delay a job chose,
/// held between [`MIN_RETRY_AFTER`] and an hour, or else the doubling backoff
/// from `base`, jittered.
#[must_use]
pub fn retry_wait(base: Duration, attempts: i32, chosen: Option<Duration>) -> Duration {
    match chosen {
        Some(delay) => delay.clamp(MIN_RETRY_AFTER, MAX_BACKOFF),
        None => jittered(backoff(base, attempts)),
    }
}

/// Cut `text` to at most [`MAX_ERROR`] bytes, on a character boundary.
fn cut(mut text: String) -> String {
    if text.len() > MAX_ERROR {
        let mut end = MAX_ERROR;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
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
pub async fn enqueue_on<'e, E: PgExecutor<'e>>(executor: E, job: &NewJob) -> Result<Uuid> {
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
        When::Now => (None, 0.0),
        When::At(at) => (Some(at), 0.0),
        When::In(delay) => (None, delay.as_secs_f64()),
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
    .bind(at.map(jiff_sqlx::Timestamp::from))
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
pub async fn enqueue_all_on(connection: &mut PgConnection, jobs: &[NewJob]) -> Result<Vec<Uuid>> {
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
/// Fails on a database error, and with [`Error::NotACode`] when `by` or `reason` is
/// empty, longer than [`MAX_REASON_CHARS`], or has a line break, before anything is
/// written.
pub async fn cancel(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    by: &str,
    reason: &str,
) -> Result<bool> {
    act(transaction, CANCEL, id, by, reason).await
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
/// Fails on a database error, and with [`Error::NotACode`] when `by` or `reason` is
/// empty, longer than [`MAX_REASON_CHARS`], or has a line break, before anything is
/// written.
pub async fn retry(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    by: &str,
    reason: &str,
) -> Result<bool> {
    act(transaction, RETRY_FAILED, id, by, reason).await
}

/// Run one act's statement, which changes the job and keeps the act together.
async fn act(
    transaction: &mut Transaction<'_, Postgres>,
    statement: &'static str,
    id: Uuid,
    by: &str,
    reason: &str,
) -> Result<bool> {
    check_code(by, "by")?;
    check_code(reason, "reason")?;
    let kept = sqlx::query(statement)
        .bind(id)
        .bind(by)
        .bind(reason)
        .execute(&mut **transaction)
        .await?
        .rows_affected();
    Ok(kept == 1)
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

/// A time column of `row`.
fn time(row: &PgRow, column: &str) -> Result<Timestamp> {
    Ok(row.try_get::<jiff_sqlx::Timestamp, _>(column)?.to_jiff())
}

/// A time column of `row` that may be `NULL`.
fn time_or_none(row: &PgRow, column: &str) -> Result<Option<Timestamp>> {
    Ok(row
        .try_get::<Option<jiff_sqlx::Timestamp>, _>(column)?
        .map(jiff_sqlx::Timestamp::to_jiff))
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

/// A Postgres-backed job queue.
pub struct Queue {
    pool: PgPool,
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
    pub fn from_pool(pool: PgPool, options: QueueOptions) -> Self {
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
        let pool = PgPoolOptions::new()
            // The loops, housekeeping, schedules, and the listener.
            .max_connections(loops.saturating_add(3))
            .connect(uri)
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
        // `CREATE ... IF NOT EXISTS` is not safe against itself: two services
        // booting together can both pass the check and one then fails on a
        // catalog constraint. The lock makes setup run one at a time, and
        // ends with the transaction.
        let mut setup = queue.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(SETUP_LOCK)
            .execute(&mut *setup)
            .await?;
        for statement in SETUP_STATEMENTS {
            sqlx::query(statement).execute(&mut *setup).await?;
        }
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
        let mut missing: Vec<String> = sqlx::query_scalar(MISSING_TRIGGERS)
            .bind(ACTS_TRIGGERS.to_vec())
            .fetch_all(&self.pool)
            .await?;
        let unique_index: bool =
            sqlx::query_scalar("SELECT to_regclass('axa_queue_unique') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !unique_index {
            missing.push("axa_queue_unique".to_string());
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::Database(sqlx::Error::Configuration(
                format!(
                    "the job queue is missing, or has turned off, {}: run SETUP_STATEMENTS",
                    missing.join(", ")
                )
                .into(),
            )))
        }
    }

    /// The pool, for a service's own queries or [`enqueue_on`] outside a
    /// transaction.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
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
            run_at: time(&row, "run_at")?,
            started_at: time_or_none(&row, "started_at")?,
            created_at: time(&row, "created_at")?,
            updated_at: time(&row, "updated_at")?,
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
                    at: time(row, "at")?,
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
        let mut queued = 0;
        for schedule in &self.schedules {
            if self.tick(schedule).await? {
                queued += 1;
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
                let now: jiff_sqlx::Timestamp = sqlx::query_scalar("SELECT now()")
                    .fetch_one(&self.pool)
                    .await?;
                Some(next_after(cron, now.to_jiff())?)
            }
        };
        sqlx::query(SCHEDULE_CREATE)
            .bind(&schedule.name)
            .bind(first.map(jiff_sqlx::Timestamp::from))
            .bind(schedule.times.interval().map(millis))
            .bind(schedule.times.pattern())
            .execute(&self.pool)
            .await?;
        let mut transaction = self.pool.begin().await?;
        // Another process holding the row is queuing this time already.
        let Some(row) = sqlx::query(SCHEDULE_LOCK)
            .bind(&schedule.name)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Ok(false);
        };
        if !row.try_get::<bool, _>("due")? {
            transaction.commit().await?;
            return Ok(false);
        }
        let busy = match row.try_get::<Option<Uuid>, _>("last_job")? {
            Some(last) => {
                sqlx::query_scalar::<_, bool>(SCHEDULE_BUSY)
                    .bind(last)
                    .fetch_one(&mut *transaction)
                    .await?
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
                    .bind(&schedule.name)
                    .bind(interval.as_secs_f64())
                    .bind(job)
                    .execute(&mut *transaction)
                    .await?;
            }
            Times::Cron(cron, _) => {
                sqlx::query(SCHEDULE_ADVANCE_TO)
                    .bind(&schedule.name)
                    .bind(jiff_sqlx::Timestamp::from(next_after(
                        cron,
                        time(&row, "now")?,
                    )?))
                    .bind(job)
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
        let names: Vec<&str> = self.schedules.iter().map(|s| s.name.as_str()).collect();
        let result = sqlx::query(SCHEDULE_ARCHIVE)
            .bind(&names)
            .bind(self.options.stale_minutes)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
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
                    due_at: time(row, "due_at")?,
                    last_job: row.try_get("last_job")?,
                    last_state: row
                        .try_get::<Option<&str>, _>("last_status")?
                        .map(JobState::parse)
                        .transpose()?,
                    last_error: row.try_get("last_error")?,
                    seen_at: time(row, "seen_at")?,
                    archived_at: time_or_none(row, "archived_at")?,
                    created_at: time(row, "created_at")?,
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
        // Not heard from for this long, a run is taken for lost by any worker.
        let stale =
            Duration::from_secs(u64::try_from(self.options.stale_minutes).unwrap_or(0) * 60);
        // This worker stops the job before then, two heartbeats early, and at half the
        // time at the latest, so that it has stopped before another can take it.
        let give_up = stale.saturating_sub(every * 2).max(stale / 2);
        let mut last_heard = tokio::time::Instant::now();
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
        let mut next_beat = tokio::time::Instant::now() + every;
        let mut failing = false;
        let limit = claim
            .timeout
            .map(|timeout| tokio::time::Instant::now() + timeout);
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
                    let limit = claim.timeout.unwrap_or_default();
                    return Outcome::Done(Err(
                        Error::Job(format!("timed out after {}s", limit.as_secs_f64())).to_string().into(),
                    ));
                }
                () = tokio::time::sleep_until(last_heard + give_up), if failing => {
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
                    next_beat = tokio::time::Instant::now() + every;
                    match answer {
                        Ok(Ok(done)) if done.rows_affected() == 1 => {
                            last_heard = tokio::time::Instant::now();
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
                .bind(claim.id)
                .bind(&text)
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

    async fn claim(&self, names: &[String]) -> Result<Option<Claimed>> {
        let Some(row) = sqlx::query(CLAIM)
            .bind(names)
            .fetch_optional(&self.pool)
            .await?
        else {
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
                    .bind(id)
                    .bind(attempts)
                    .bind("completed")
                    .bind(None::<String>)
                    .execute(&self.pool)
                    .await?
            }
            Outcome::Done(Err(error)) if attempts < job.limit => {
                tracing::warn!(job = %name, %id, attempts, %error, "job failed, will retry");
                let wait = retry_wait(self.options.retry_backoff, attempts, error.retry_in);
                sqlx::query(RETRY)
                    .bind(id)
                    .bind(attempts)
                    .bind(cut(error.text))
                    .bind(wait.as_secs_f64())
                    .execute(&self.pool)
                    .await?
            }
            Outcome::Done(Err(error)) => {
                tracing::error!(job = %name, %id, attempts, %error, "job failed");
                sqlx::query(FINISH)
                    .bind(id)
                    .bind(attempts)
                    .bind("failed")
                    .bind(cut(error.text))
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
        let wake = Arc::new(Notify::new());
        loops.push(AbortOnDrop(tokio::spawn(listen(
            self.pool.clone(),
            wake.clone(),
            self.options.poll_interval,
            shutdown.clone(),
        ))));
        for _ in 0..self.options.workers.max(1) {
            let queue = self.clone();
            let wake = wake.clone();
            let mut shutdown = shutdown.clone();
            loops.push(AbortOnDrop(tokio::spawn(async move {
                while !*shutdown.borrow() {
                    let round = async {
                        queue.run_one().await.unwrap_or_else(|error| {
                            tracing::error!(%error, "job queue error");
                            false
                        })
                    };
                    work_or_wait(&wake, queue.options.poll_interval, &mut shutdown, round).await;
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

/// Run `round`, and when it found no work, wait for `poll`, a wake, or shutdown. The
/// wait is listened for before `round` starts, so a notice that comes while `round`
/// looks, after it found nothing, still wakes the loop rather than being lost.
async fn work_or_wait(
    wake: &Notify,
    poll: Duration,
    shutdown: &mut watch::Receiver<bool>,
    round: impl Future<Output = bool>,
) {
    let woken = wake.notified();
    tokio::pin!(woken);
    woken.as_mut().enable();
    if !round.await {
        tokio::select! {
            () = tokio::time::sleep(poll) => {},
            () = woken => {},
            _ = shutdown.changed() => {},
        }
    }
}

/// Wake the idle job loops at each notice on [`NOTIFY_CHANNEL`], until `shutdown`.
/// A listener that fails is opened again after `gap`, the loops polling meanwhile.
async fn listen(
    pool: PgPool,
    wake: Arc<Notify>,
    gap: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    while !*shutdown.borrow() {
        match sqlx::postgres::PgListener::connect_with(&pool).await {
            Ok(mut listener) => match listener.listen(NOTIFY_CHANNEL).await {
                Ok(()) => loop {
                    tokio::select! {
                        notice = listener.try_recv() => match notice {
                            // A notice, or a dropped connection, which the next call opens again:
                            // a notice sent meanwhile is lost, so wake the loops to look.
                            Ok(_) => wake.notify_waiters(),
                            Err(error) => {
                                tracing::warn!(%error, "job queue listener failed, polling");
                                break;
                            }
                        },
                        _ = shutdown.changed() => return,
                    }
                },
                Err(error) => tracing::warn!(%error, "job queue could not listen, polling"),
            },
            Err(error) => tracing::warn!(%error, "job queue could not listen, polling"),
        }
        tokio::select! {
            () = tokio::time::sleep(gap) => {},
            _ = shutdown.changed() => {},
        }
    }
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
mod tests {
    use super::*;

    #[test]
    fn a_stored_word_that_is_not_a_known_state_or_act_is_refused() {
        assert!(JobState::parse("cancelled").is_ok());
        assert!(ActKind::parse("released").is_ok());
        let error = JobState::parse("lost").unwrap_err().to_string();
        assert!(error.contains("unknown status"), "{error}");
        let error = ActKind::parse("lost").unwrap_err().to_string();
        assert!(error.contains("unknown kind"), "{error}");
        let error = Error::NotATenant.to_string();
        assert!(error.contains("lower-case letters"), "{error}");
    }

    #[tokio::test]
    async fn a_wake_during_a_round_that_found_nothing_is_not_lost() {
        let wake = Notify::new();
        let (_stop, mut shutdown) = watch::channel(false);
        // The notice comes while the round looks, before the loop would wait.
        let round = async {
            wake.notify_waiters();
            false
        };
        let waited = tokio::time::timeout(
            Duration::from_secs(5),
            work_or_wait(&wake, Duration::from_hours(1), &mut shutdown, round),
        )
        .await;
        assert!(
            waited.is_ok(),
            "the wake was lost and the loop waited for its poll"
        );
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
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://u@localhost/none")
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
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://axa:secret-password@127.0.0.1:1/axa")
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
