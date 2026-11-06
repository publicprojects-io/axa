//! A Postgres job queue using `FOR UPDATE SKIP LOCKED`.
//!
//! Jobs live in one `axa_queue` table, created on [`Queue::connect`]. Each
//! worker loop claims the oldest ready job in a single statement, so two
//! loops can never claim the same row and no external broker is needed.
//!
//! A job moves from `queued` to `running` to `completed` or `failed`, with
//! the failure text kept on the row.
//!
//! # Attempts
//!
//! A job gets [`QueueOptions::max_attempts`] tries, unless its worker sets
//! [`BackgroundWorker::max_attempts`]. The default is one, because a retry
//! repeats the job's work, and only its author knows that is safe. Raise it for
//! jobs written to be repeated. Each retry waits twice as long as the last,
//! starting from [`QueueOptions::retry_backoff`].
//!
//! A job that panics counts as failed, and its loop carries on.
//!
//! # Lost jobs
//!
//! A running job reports in every [`QueueOptions::heartbeat_interval`], so a job
//! may run for as long as it needs. One not heard from for
//! [`QueueOptions::stale_minutes`] is assumed lost with its worker. If it has
//! attempts left it is queued again, and otherwise it is marked failed. A crash
//! can never loop forever. A job that was given up on cannot then overwrite the
//! result of its replacement.
//!
//! The heartbeat shows that the process is alive, not that the job is getting
//! anywhere, so a job that hangs holds its loop until the process stops.
//!
//! # Queuing inside a transaction
//!
//! [`enqueue_on`] takes the caller's transaction, so a job and the record it is
//! about commit together or not at all. [`Queue::enqueue`] uses its own
//! connection, so a job queued that way can outlive a rollback.
//!
//! # Retention
//!
//! Finished rows are deleted after [`QueueOptions::retention_days`], because
//! the `args` column is plain text and a job table should not become a second
//! copy of the data. Never put a secret in job arguments.
//!
//! # Running it
//!
//! Nothing runs jobs by itself. A service needs a process calling [`Queue::run`],
//! usually through `start --all` or `start --worker`, and
//! [`warn_if_unserved`] says so at boot when it has none.

use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};

use serde::{Serialize, de::DeserializeOwned};
use sqlx::{PgExecutor, PgPool, Row, postgres::PgPoolOptions};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{AppContext, Error, Result, worker::BackgroundWorker};

type JobFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type Handler = Arc<dyn Fn(serde_json::Value) -> JobFuture + Send + Sync>;

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
/// Tables created by axa 0.1.0 have no `attempts` column.
const SETUP_ATTEMPTS: &str =
    "ALTER TABLE axa_queue ADD COLUMN IF NOT EXISTS attempts INTEGER NOT NULL DEFAULT 0";
/// A worker's own limit on tries, when it sets one.
const SETUP_MAX_ATTEMPTS: &str =
    "ALTER TABLE axa_queue ADD COLUMN IF NOT EXISTS max_attempts INTEGER";
/// Advisory lock key for table setup: `axa_q` as bytes.
const SETUP_LOCK: i64 = 0x0061_7861_5F71;
const ENQUEUE: &str =
    "INSERT INTO axa_queue (id, name, args, max_attempts) VALUES ($1, $2, $3, $4)";
const CLAIM: &str = "UPDATE axa_queue
    SET status = 'running', started_at = now(), updated_at = now(), attempts = attempts + 1
    WHERE id = (
        SELECT id FROM axa_queue
        WHERE status = 'queued' AND run_at <= now() AND name = ANY($1)
        ORDER BY run_at
        FOR UPDATE SKIP LOCKED
        LIMIT 1
    )
    RETURNING id, name, args, attempts, max_attempts";
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
    WHERE status IN ('completed', 'failed')
      AND updated_at < now() - make_interval(days => $1)";

/// The shortest gap between heartbeats, so a zero setting cannot spin.
const MIN_HEARTBEAT: Duration = Duration::from_millis(20);

/// The longest wait between attempts.
const MAX_BACKOFF: Duration = Duration::from_hours(1);

/// How long to wait before the attempt after `attempts`, doubling each time.
fn backoff(base: Duration, attempts: i32) -> Duration {
    let doublings = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(16);
    base.saturating_mul(1 << doublings).min(MAX_BACKOFF)
}

/// Warn when a service queues jobs and nothing here will run them, returning
/// whether it warned.
///
/// Queued jobs then wait forever, and nothing else says so: the request that
/// queued the job has already succeeded.
pub fn warn_if_unserved(ctx: &AppContext, worker_running: bool) -> bool {
    let unserved = ctx.queue_provider.is_some() && !worker_running;
    if unserved {
        tracing::warn!(
            "jobs are queued but this process runs no worker: use `start --all`, \\
             or run `start --worker` separately"
        );
    }
    unserved
}

/// Add a job for worker `W` on `executor`, usually the caller's transaction, and
/// return its id. The job then commits or rolls back with the transaction.
///
/// ```ignore
/// let mut transaction = pool.begin().await?;
/// sqlx::query("INSERT INTO reports (id) VALUES ($1)")
///     .bind(id)
///     .execute(&mut *transaction)
///     .await?;
/// enqueue_on::<ReportWorker, _, _>(&mut *transaction, &Args { id }).await?;
/// transaction.commit().await?;
/// ```
///
/// # Errors
///
/// Fails when the arguments cannot be encoded, or on a database error.
pub async fn enqueue_on<'e, W, A, E>(executor: E, args: &A) -> Result<String>
where
    W: BackgroundWorker<A>,
    A: Serialize + DeserializeOwned + Send + Sync + 'static,
    E: PgExecutor<'e>,
{
    insert(
        executor,
        &W::class_name(),
        serde_json::to_value(args)?,
        W::max_attempts(),
    )
    .await
}

async fn insert<'e, E: PgExecutor<'e>>(
    executor: E,
    name: &str,
    args: serde_json::Value,
    max_attempts: Option<i32>,
) -> Result<String> {
    let id = Uuid::new_v4();
    sqlx::query(ENQUEUE)
        .bind(id)
        .bind(name)
        .bind(args)
        .bind(max_attempts)
        .execute(executor)
        .await?;
    Ok(id.to_string())
}

/// Queue tuning.
#[derive(Debug, Clone)]
pub struct QueueOptions {
    /// Concurrent job loops.
    pub workers: usize,
    /// Sleep between polls of an empty queue.
    pub poll_interval: Duration,
    /// A running job not heard from for this many minutes is assumed lost with
    /// its worker.
    pub stale_minutes: i32,
    /// How often a running job reports in. Keep it well under `stale_minutes`.
    pub heartbeat_interval: Duration,
    /// Tries a job gets, including the first, unless its worker sets its own.
    /// One means no retries.
    pub max_attempts: i32,
    /// The wait before the first retry.
    pub retry_backoff: Duration,
    /// Days to keep finished rows.
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
    options: QueueOptions,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("handlers", &self.handlers.len())
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Queue {
    /// Connect and create the queue table if needed.
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be reached or the table not created.
    pub async fn connect(uri: &str, options: QueueOptions) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(
                u32::try_from(options.workers)
                    .unwrap_or(2)
                    .saturating_add(1),
            )
            .connect(uri)
            .await?;
        // `CREATE ... IF NOT EXISTS` is not safe against itself: two services
        // booting together can both pass the check and one then fails on a
        // catalog constraint. The lock makes setup run one at a time, and
        // ends with the transaction.
        let mut setup = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(SETUP_LOCK)
            .execute(&mut *setup)
            .await?;
        sqlx::query(SETUP).execute(&mut *setup).await?;
        sqlx::query(SETUP_ATTEMPTS).execute(&mut *setup).await?;
        sqlx::query(SETUP_MAX_ATTEMPTS).execute(&mut *setup).await?;
        sqlx::query(SETUP_INDEX).execute(&mut *setup).await?;
        setup.commit().await?;
        Ok(Self {
            pool,
            handlers: HashMap::new(),
            options,
        })
    }

    /// Register a worker to handle its jobs.
    pub fn register<A, W>(&mut self, worker: W)
    where
        A: Serialize + DeserializeOwned + Send + Sync + 'static,
        W: BackgroundWorker<A> + 'static,
    {
        let worker = Arc::new(worker);
        let handler: Handler = Arc::new(move |args| {
            let worker = worker.clone();
            Box::pin(async move { worker.perform(serde_json::from_value(args)?).await })
        });
        self.handlers.insert(W::class_name(), handler);
    }

    /// Add a job and return its id.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn enqueue(&self, name: &str, args: serde_json::Value) -> Result<String> {
        insert(&self.pool, name, args, None).await
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
    ) -> Result<String> {
        insert(&self.pool, name, args, max_attempts).await
    }

    /// A queue on a pool nothing listens to, for testing what depends on it.
    #[cfg(all(test, not(feature = "sea-orm")))]
    pub(crate) fn unreachable() -> Self {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy("postgres://axa:axa@127.0.0.1:1/axa")
            .unwrap();
        Self {
            pool,
            handlers: HashMap::new(),
            options: QueueOptions::default(),
        }
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

    /// Delete finished rows older than the retention period, returning how
    /// many went.
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

    /// One round of housekeeping. Failures are logged, because the next round
    /// can try again.
    async fn maintain(&self) {
        if let Err(error) = self.requeue_stale().await {
            tracing::error!(%error, "could not recover stale jobs");
        }
        if let Err(error) = self.purge_finished().await {
            tracing::error!(%error, "could not delete old jobs");
        }
    }

    /// Run `job` to its end in its own task, reporting in as it goes, so a panic
    /// stops only the job.
    async fn supervise(&self, job: JobFuture, id: Uuid, attempts: i32) -> Result<()> {
        let mut job = tokio::spawn(job);
        let every = self.options.heartbeat_interval.max(MIN_HEARTBEAT);
        loop {
            tokio::select! {
                finished = &mut job => {
                    return finished.unwrap_or_else(|_| {
                        Err(Error::Worker("the job panicked".to_string()))
                    });
                }
                () = tokio::time::sleep(every) => {
                    let beat = sqlx::query(HEARTBEAT)
                        .bind(id)
                        .bind(attempts)
                        .execute(&self.pool)
                        .await;
                    if let Err(error) = beat {
                        tracing::warn!(%id, %error, "job heartbeat failed");
                    }
                }
            }
        }
    }

    /// Note a run that finished after its job was given up on.
    fn note_superseded(rows: u64, id: Uuid, name: &str) {
        if rows == 0 {
            tracing::warn!(job = %name, %id, "job finished after being given up on");
        }
    }

    /// Claim and run one job, returning whether there was one. A job's own
    /// failure is recorded on its row, not returned.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn run_one(&self) -> Result<bool> {
        let names: Vec<String> = self.handlers.keys().cloned().collect();
        let Some(row) = sqlx::query(CLAIM)
            .bind(&names)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(false);
        };
        let id: Uuid = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let args: serde_json::Value = row.try_get("args")?;
        let attempts: i32 = row.try_get("attempts")?;
        let limit = row
            .try_get::<Option<i32>, _>("max_attempts")?
            .unwrap_or(self.options.max_attempts);
        let outcome = match self.handlers.get(&name) {
            Some(handler) => self.supervise(handler(args), id, attempts).await,
            None => Err(Error::Worker(format!("no handler for job {name}"))),
        };
        let finished = match outcome {
            Ok(()) => {
                sqlx::query(FINISH)
                    .bind(id)
                    .bind(attempts)
                    .bind("completed")
                    .bind(None::<String>)
                    .execute(&self.pool)
                    .await?
            }
            Err(error) if attempts < limit => {
                tracing::warn!(job = %name, %id, attempts, %error, "job failed, will retry");
                let wait = backoff(self.options.retry_backoff, attempts);
                sqlx::query(RETRY)
                    .bind(id)
                    .bind(attempts)
                    .bind(error.to_string())
                    .bind(wait.as_secs_f64())
                    .execute(&self.pool)
                    .await?
            }
            Err(error) => {
                tracing::error!(job = %name, %id, attempts, %error, "job failed");
                sqlx::query(FINISH)
                    .bind(id)
                    .bind(attempts)
                    .bind("failed")
                    .bind(error.to_string())
                    .execute(&self.pool)
                    .await?
            }
        };
        Self::note_superseded(finished.rows_affected(), id, &name);
        Ok(true)
    }

    /// Run job loops until `shutdown` changes to `true`. Jobs in progress
    /// finish first. Housekeeping runs straight away, to recover anything a
    /// previous process left behind, and then on a timer.
    pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>) {
        let mut loops = Vec::new();
        {
            let queue = self.clone();
            let mut shutdown = shutdown.clone();
            loops.push(tokio::spawn(async move {
                while !*shutdown.borrow() {
                    queue.maintain().await;
                    tokio::select! {
                        () = tokio::time::sleep(queue.options.maintenance_interval) => {},
                        _ = shutdown.changed() => {},
                    }
                }
            }));
        }
        for _ in 0..self.options.workers.max(1) {
            let queue = self.clone();
            let mut shutdown = shutdown.clone();
            loops.push(tokio::spawn(async move {
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
            }));
        }
        for job_loop in loops {
            let _ = job_loop.await;
        }
    }
}

/// These need a real Postgres, so they are ignored by default. Run them with
/// `AXA_TEST_DATABASE_URL=postgres://... cargo test --all-features -- --ignored`.
#[cfg(all(test, not(feature = "sea-orm")))]
mod tests {
    use super::*;
    use crate::{
        AppContext, Environment,
        config::{Config, Defaults},
    };
    use async_trait::async_trait;
    use serde::Deserialize;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNT: AtomicUsize = AtomicUsize::new(0);

    /// Held for a whole test: these tests share one table, and each one
    /// starts by emptying or dropping it.
    static TABLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[derive(Serialize, Deserialize)]
    struct Args {
        fail: bool,
    }

    struct CountWorker;

    #[async_trait]
    impl BackgroundWorker<Args> for CountWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        async fn perform(&self, args: Args) -> Result<()> {
            if args.fail {
                return Err(Error::Worker("told to fail".to_string()));
            }
            COUNT.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct PanicWorker;

    #[async_trait]
    impl BackgroundWorker<()> for PanicWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        async fn perform(&self, (): ()) -> Result<()> {
            panic!("the job's own bug");
        }
    }

    /// Works for as long as its argument says.
    struct SlowWorker;

    #[async_trait]
    impl BackgroundWorker<u64> for SlowWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        async fn perform(&self, millis: u64) -> Result<()> {
            tokio::time::sleep(Duration::from_millis(millis)).await;
            Ok(())
        }
    }

    /// Always fails, and may be tried three times.
    struct RetryingWorker;

    #[async_trait]
    impl BackgroundWorker<()> for RetryingWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        fn max_attempts() -> Option<i32> {
            Some(3)
        }

        async fn perform(&self, (): ()) -> Result<()> {
            Err(Error::Worker("always".to_string()))
        }
    }

    async fn attempts_of(queue: &Queue, id: &str) -> i32 {
        sqlx::query("SELECT attempts FROM axa_queue WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(&queue.pool)
            .await
            .unwrap()
            .get("attempts")
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_job_that_panics_fails_and_its_loop_carries_on() {
        let _table = TABLE.lock().await;
        let mut queue = queue().await;
        queue.register(PanicWorker);
        let queue = Arc::new(queue);
        let panics = queue
            .enqueue("PanicWorker", serde_json::json!(null))
            .await
            .unwrap();
        let args = serde_json::json!({ "fail": false });
        let after = queue.enqueue("CountWorker", args).await.unwrap();
        let (stop, stopped) = watch::channel(false);
        let running = tokio::spawn(queue.clone().run(stopped));
        for _ in 0..100 {
            if status_of(&queue, &after).await.0 == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stop.send(true).unwrap();
        running.await.unwrap();
        let (status, error) = status_of(&queue, &panics).await;
        assert_eq!(status, "failed");
        assert_eq!(error.as_deref(), Some("worker error: the job panicked"));
        assert_eq!(status_of(&queue, &after).await.0, "completed");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_long_job_that_reports_in_is_not_taken_for_lost() {
        let _table = TABLE.lock().await;
        let mut queue = queue_with(QueueOptions {
            heartbeat_interval: Duration::from_millis(30),
            max_attempts: 2,
            ..QueueOptions::default()
        })
        .await;
        queue.register(SlowWorker);
        let queue = Arc::new(queue);
        let id = queue
            .enqueue("SlowWorker", serde_json::json!(600))
            .await
            .unwrap();
        let worker = {
            let queue = queue.clone();
            tokio::spawn(async move { queue.run_one().await.unwrap() })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        // As if it had been running for hours, and silent until now. The next
        // heartbeat brings `updated_at` back, and `started_at` stays old.
        let hours_ago = "UPDATE axa_queue SET started_at = now() - interval '2 hours',
             updated_at = now() - interval '2 hours'";
        sqlx::query(hours_ago).execute(&queue.pool).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(queue.requeue_stale().await.unwrap(), 0);
        assert_eq!(status_of(&queue, &id).await.0, "running");
        assert!(worker.await.unwrap());
        assert_eq!(status_of(&queue, &id).await.0, "completed");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_job_given_up_on_cannot_overwrite_its_replacement() {
        let _table = TABLE.lock().await;
        let mut queue = queue().await;
        queue.register(SlowWorker);
        let queue = Arc::new(queue);
        let id = queue
            .enqueue("SlowWorker", serde_json::json!(200))
            .await
            .unwrap();
        let worker = {
            let queue = queue.clone();
            tokio::spawn(async move { queue.run_one().await.unwrap() })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Given up on and claimed again by another worker.
        sqlx::query("UPDATE axa_queue SET attempts = attempts + 1")
            .execute(&queue.pool)
            .await
            .unwrap();
        assert!(worker.await.unwrap());
        assert_eq!(status_of(&queue, &id).await.0, "running");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_worker_alone_stops_when_told() {
        let _table = TABLE.lock().await;
        let queue = Arc::new(queue().await);
        crate::server::run_until(None, Some(queue), &local_server(), async {})
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn the_job_loops_stop_when_the_server_cannot_start() {
        let _table = TABLE.lock().await;
        let queue = Arc::new(queue().await);
        let config = crate::config::Server {
            port: 1,
            binding: "256.0.0.1".to_string(),
            ..local_server()
        };
        let router = Some(axum::Router::new());
        let run = crate::server::run_until(router, Some(queue), &config, std::future::pending());
        let outcome = tokio::time::timeout(Duration::from_secs(10), run).await;
        assert!(outcome.expect("the job loops kept running").is_err());
    }

    fn local_server() -> crate::config::Server {
        crate::config::Server {
            port: 0,
            binding: "127.0.0.1".to_string(),
            host: String::new(),
            shutdown_timeout_seconds: 0,
        }
    }

    /// Never finishes.
    struct HangWorker;

    #[async_trait]
    impl BackgroundWorker<()> for HangWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        async fn perform(&self, (): ()) -> Result<()> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_hung_job_cannot_hold_shutdown_past_its_limit() {
        let _table = TABLE.lock().await;
        let mut queue = queue().await;
        queue.register(HangWorker);
        let queue = Arc::new(queue);
        let id = queue
            .enqueue("HangWorker", serde_json::json!(null))
            .await
            .unwrap();
        let config = crate::config::Server {
            shutdown_timeout_seconds: 1,
            ..local_server()
        };
        for _ in 0..50 {
            if status_of(&queue, &id).await.0 == "queued" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let stop = tokio::time::sleep(Duration::from_millis(500));
        let run = crate::server::run_until(None, Some(queue.clone()), &config, stop);
        let ended = tokio::time::timeout(Duration::from_secs(5), run).await;
        ended.expect("a hung job held shutdown").unwrap();
        assert_eq!(status_of(&queue, &id).await.0, "running");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_job_in_progress_finishes_before_shutdown() {
        let _table = TABLE.lock().await;
        let mut queue = queue().await;
        queue.register(SlowWorker);
        let queue = Arc::new(queue);
        let id = queue
            .enqueue("SlowWorker", serde_json::json!(300))
            .await
            .unwrap();
        let (stop, stopped) = watch::channel(false);
        let running = tokio::spawn(queue.clone().run(stopped));
        for _ in 0..50 {
            if status_of(&queue, &id).await.0 == "running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(status_of(&queue, &id).await.0, "running");
        stop.send(true).unwrap();
        running.await.unwrap();
        assert_eq!(status_of(&queue, &id).await.0, "completed");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_worker_can_allow_more_tries_than_the_queue() {
        let _table = TABLE.lock().await;
        let mut queue = queue_with(QueueOptions {
            retry_backoff: Duration::ZERO,
            ..QueueOptions::default()
        })
        .await;
        queue.register(RetryingWorker);
        let id = queue
            .enqueue_limited(
                "RetryingWorker",
                serde_json::json!(null),
                RetryingWorker::max_attempts(),
            )
            .await
            .unwrap();
        for _ in 0..3 {
            assert!(queue.run_one().await.unwrap());
        }
        assert_eq!(attempts_of(&queue, &id).await, 3);
        assert_eq!(status_of(&queue, &id).await.0, "failed");
        assert!(!queue.run_one().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_lost_job_uses_its_workers_limit_too() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        let id = queue
            .enqueue_limited("CountWorker", serde_json::json!({ "fail": false }), Some(3))
            .await
            .unwrap();
        make_stale(&queue, &id, 2).await;
        assert_eq!(queue.requeue_stale().await.unwrap(), 1);
        make_stale(&queue, &id, 3).await;
        assert_eq!(queue.requeue_stale().await.unwrap(), 0);
        assert_eq!(status_of(&queue, &id).await.0, "failed");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_job_queued_in_a_transaction_rolls_back_with_it() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        let args = Args { fail: false };

        let mut transaction = queue.pool.begin().await.unwrap();
        let dropped = enqueue_on::<CountWorker, _, _>(&mut *transaction, &args)
            .await
            .unwrap();
        transaction.rollback().await.unwrap();
        let row = sqlx::query("SELECT 1 AS n FROM axa_queue WHERE id = $1")
            .bind(Uuid::parse_str(&dropped).unwrap())
            .fetch_optional(&queue.pool)
            .await
            .unwrap();
        assert!(row.is_none());

        let mut transaction = queue.pool.begin().await.unwrap();
        let kept = enqueue_on::<CountWorker, _, _>(&mut *transaction, &args)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        assert_eq!(status_of(&queue, &kept).await.0, "queued");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn the_max_attempts_column_is_added_to_an_older_table() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        sqlx::query("ALTER TABLE axa_queue DROP COLUMN max_attempts")
            .execute(&queue.pool)
            .await
            .unwrap();
        let queue = self::queue().await;
        queue
            .enqueue("CountWorker", serde_json::json!({ "fail": false }))
            .await
            .unwrap();
        assert!(queue.run_one().await.unwrap());
    }

    async fn queue() -> Queue {
        let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
        let mut queue = Queue::connect(&uri, QueueOptions::default()).await.unwrap();
        sqlx::query("DELETE FROM axa_queue")
            .execute(&queue.pool)
            .await
            .unwrap();
        queue.register(CountWorker);
        queue
    }

    async fn status_of(queue: &Queue, id: &str) -> (String, Option<String>) {
        let row = sqlx::query("SELECT status, error FROM axa_queue WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(&queue.pool)
            .await
            .unwrap();
        (row.get("status"), row.get("error"))
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn queue_behaviour_against_postgres() {
        let _table = TABLE.lock().await;
        let queue = Arc::new(queue().await);
        queue.ping().await.unwrap();

        // An empty queue has nothing to run.
        assert!(!queue.run_one().await.unwrap());

        // A job runs and is marked completed.
        let before = COUNT.load(Ordering::SeqCst);
        let args = serde_json::json!({ "fail": false });
        let id = queue.enqueue("CountWorker", args.clone()).await.unwrap();
        assert!(queue.run_one().await.unwrap());
        assert_eq!(COUNT.load(Ordering::SeqCst), before + 1);
        assert_eq!(status_of(&queue, &id).await.0, "completed");

        // A failure is kept on the row and does not fail the loop.
        let id = queue
            .enqueue("CountWorker", serde_json::json!({ "fail": true }))
            .await
            .unwrap();
        assert!(queue.run_one().await.unwrap());
        let (status, error) = status_of(&queue, &id).await;
        assert_eq!(status, "failed");
        assert!(error.unwrap().contains("told to fail"));

        // Bad arguments fail the job rather than panicking the worker.
        let id = queue
            .enqueue("CountWorker", serde_json::json!({ "wrong": 1 }))
            .await
            .unwrap();
        assert!(queue.run_one().await.unwrap());
        assert_eq!(status_of(&queue, &id).await.0, "failed");

        // A job with no handler stays queued for a worker that has one.
        let id = queue.enqueue("OtherWorker", args.clone()).await.unwrap();
        assert!(!queue.run_one().await.unwrap());
        assert_eq!(status_of(&queue, &id).await.0, "queued");

        // A stale running job is queued again, and a recent one is not.
        let stale = queue.enqueue("CountWorker", args.clone()).await.unwrap();
        sqlx::query(
            "UPDATE axa_queue SET status = 'running', started_at = now() - interval '2 hours',
             updated_at = now() - interval '2 hours' WHERE id = $1",
        )
        .bind(Uuid::parse_str(&stale).unwrap())
        .execute(&queue.pool)
        .await
        .unwrap();
        assert_eq!(queue.requeue_stale().await.unwrap(), 1);
        assert_eq!(status_of(&queue, &stale).await.0, "queued");
        assert!(queue.run_one().await.unwrap());

        // Concurrent loops run each job exactly once.
        sqlx::query("DELETE FROM axa_queue")
            .execute(&queue.pool)
            .await
            .unwrap();
        let before = COUNT.load(Ordering::SeqCst);
        for _ in 0..20 {
            queue.enqueue("CountWorker", args.clone()).await.unwrap();
        }
        let (stop, stopped) = watch::channel(false);
        let running = tokio::spawn(queue.clone().run(stopped));
        for _ in 0..100 {
            if COUNT.load(Ordering::SeqCst) >= before + 20 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        stop.send(true).unwrap();
        running.await.unwrap();
        assert_eq!(COUNT.load(Ordering::SeqCst), before + 20);
        let left: i64 =
            sqlx::query("SELECT count(*) AS n FROM axa_queue WHERE status <> 'completed'")
                .fetch_one(&queue.pool)
                .await
                .unwrap()
                .get("n");
        assert_eq!(left, 0);
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn services_booting_together_all_connect() {
        let _table = TABLE.lock().await;
        let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
        let admin = PgPoolOptions::new().connect(&uri).await.unwrap();
        // Setup only races when the table is really missing.
        for _ in 0..5 {
            sqlx::query("DROP TABLE IF EXISTS axa_queue")
                .execute(&admin)
                .await
                .unwrap();
            let boots: Vec<_> = (0..8)
                .map(|_| {
                    let uri = uri.clone();
                    tokio::spawn(async move { Queue::connect(&uri, QueueOptions::default()).await })
                })
                .collect();
            for boot in boots {
                boot.await.unwrap().unwrap();
            }
        }
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
    fn a_service_without_a_queue_gets_no_worker_warning() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        let ctx = AppContext::new(Environment::Test, (), config);
        assert!(!warn_if_unserved(&ctx, false));
    }

    async fn queue_with(options: QueueOptions) -> Queue {
        let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
        let mut queue = Queue::connect(&uri, options).await.unwrap();
        sqlx::query("DELETE FROM axa_queue")
            .execute(&queue.pool)
            .await
            .unwrap();
        queue.register(CountWorker);
        queue
    }

    async fn make_stale(queue: &Queue, id: &str, attempts: i32) {
        sqlx::query(
            "UPDATE axa_queue
             SET status = 'running', attempts = $2, started_at = now() - interval '2 hours',
                 updated_at = now() - interval '2 hours'
             WHERE id = $1",
        )
        .bind(Uuid::parse_str(id).unwrap())
        .bind(attempts)
        .execute(&queue.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_failing_job_retries_up_to_its_limit() {
        let _table = TABLE.lock().await;
        let queue = queue_with(QueueOptions {
            max_attempts: 2,
            retry_backoff: Duration::ZERO,
            ..QueueOptions::default()
        })
        .await;
        let id = queue
            .enqueue("CountWorker", serde_json::json!({ "fail": true }))
            .await
            .unwrap();

        assert!(queue.run_one().await.unwrap());
        let (status, error) = status_of(&queue, &id).await;
        assert_eq!(status, "queued");
        assert!(error.unwrap().contains("told to fail"));

        assert!(queue.run_one().await.unwrap());
        assert_eq!(status_of(&queue, &id).await.0, "failed");
        assert!(!queue.run_one().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_retry_waits_for_its_backoff() {
        let _table = TABLE.lock().await;
        let queue = queue_with(QueueOptions {
            max_attempts: 2,
            retry_backoff: Duration::from_mins(10),
            ..QueueOptions::default()
        })
        .await;
        queue
            .enqueue("CountWorker", serde_json::json!({ "fail": true }))
            .await
            .unwrap();
        assert!(queue.run_one().await.unwrap());
        assert!(!queue.run_one().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn by_default_a_failed_job_is_not_repeated() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        let id = queue
            .enqueue("CountWorker", serde_json::json!({ "fail": true }))
            .await
            .unwrap();
        assert!(queue.run_one().await.unwrap());
        assert_eq!(status_of(&queue, &id).await.0, "failed");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_job_that_keeps_killing_workers_ends_as_failed() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        let id = queue
            .enqueue("CountWorker", serde_json::json!({ "fail": false }))
            .await
            .unwrap();
        make_stale(&queue, &id, 1).await;
        assert_eq!(queue.requeue_stale().await.unwrap(), 0);
        let (status, error) = status_of(&queue, &id).await;
        assert_eq!(status, "failed");
        assert!(error.unwrap().contains("abandoned"));
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_stale_job_with_attempts_left_runs_again() {
        let _table = TABLE.lock().await;
        let queue = queue_with(QueueOptions {
            max_attempts: 3,
            ..QueueOptions::default()
        })
        .await;
        let id = queue
            .enqueue("CountWorker", serde_json::json!({ "fail": false }))
            .await
            .unwrap();
        make_stale(&queue, &id, 1).await;
        assert_eq!(queue.requeue_stale().await.unwrap(), 1);
        assert_eq!(status_of(&queue, &id).await.0, "queued");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn old_finished_rows_are_deleted_and_nothing_else() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        let args = serde_json::json!({ "fail": false });
        let old_done = queue.enqueue("CountWorker", args.clone()).await.unwrap();
        let old_failed = queue.enqueue("CountWorker", args.clone()).await.unwrap();
        let new_done = queue.enqueue("CountWorker", args.clone()).await.unwrap();
        let old_queued = queue.enqueue("CountWorker", args.clone()).await.unwrap();
        let set = |id: String, status: &'static str, days: i32| {
            let pool = queue.pool.clone();
            async move {
                sqlx::query(
                    "UPDATE axa_queue SET status = $2,
                     updated_at = now() - make_interval(days => $3) WHERE id = $1",
                )
                .bind(Uuid::parse_str(&id).unwrap())
                .bind(status)
                .bind(days)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        set(old_done, "completed", 30).await;
        set(old_failed, "failed", 30).await;
        set(new_done.clone(), "completed", 1).await;
        set(old_queued.clone(), "queued", 30).await;

        assert_eq!(queue.purge_finished().await.unwrap(), 2);
        assert_eq!(status_of(&queue, &new_done).await.0, "completed");
        assert_eq!(status_of(&queue, &old_queued).await.0, "queued");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn the_attempts_column_is_added_to_an_older_table() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        sqlx::query("ALTER TABLE axa_queue DROP COLUMN attempts")
            .execute(&queue.pool)
            .await
            .unwrap();
        let queue = self::queue().await;
        queue
            .enqueue("CountWorker", serde_json::json!({ "fail": false }))
            .await
            .unwrap();
        assert!(queue.run_one().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_service_with_a_queue_and_no_worker_is_warned() {
        let _table = TABLE.lock().await;
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        let mut ctx = AppContext::new(Environment::Test, (), config);
        ctx.queue_provider = Some(Arc::new(queue().await));
        assert!(warn_if_unserved(&ctx, false));
        assert!(!warn_if_unserved(&ctx, true));
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn perform_later_enqueues_in_queue_mode() {
        let _table = TABLE.lock().await;
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |name| {
            (name == "WORKER_MODE").then(|| "BackgroundQueue".to_string())
        })
        .unwrap();
        let mut ctx = AppContext::new(Environment::Test, (), config);
        let queue = Arc::new(queue().await);
        ctx.queue_provider = Some(queue.clone());
        let id = CountWorker::perform_later(&ctx, Args { fail: false })
            .await
            .unwrap();
        assert_eq!(status_of(&queue, &id).await.0, "queued");
    }
}
