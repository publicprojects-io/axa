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
//! A job gets [`QueueOptions::max_attempts`] tries. The default is one, because
//! a retry repeats the job's work, and only its author knows that is safe. Raise
//! it for jobs written to be repeated. Each retry waits twice as long as the
//! last, starting from [`QueueOptions::retry_backoff`].
//!
//! A job still `running` after [`QueueOptions::stale_minutes`] is assumed lost
//! with its worker. If it has attempts left it is queued again, and otherwise
//! it is marked failed. A crash can never loop forever.
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
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
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
/// Advisory lock key for table setup: `axa_q` as bytes.
const SETUP_LOCK: i64 = 0x0061_7861_5F71;
const ENQUEUE: &str = "INSERT INTO axa_queue (id, name, args) VALUES ($1, $2, $3)";
const CLAIM: &str = "UPDATE axa_queue
    SET status = 'running', started_at = now(), updated_at = now(), attempts = attempts + 1
    WHERE id = (
        SELECT id FROM axa_queue
        WHERE status = 'queued' AND run_at <= now() AND name = ANY($1)
        ORDER BY run_at
        FOR UPDATE SKIP LOCKED
        LIMIT 1
    )
    RETURNING id, name, args, attempts";
const FINISH: &str =
    "UPDATE axa_queue SET status = $2, error = $3, updated_at = now() WHERE id = $1";
const RETRY: &str = "UPDATE axa_queue
    SET status = 'queued', error = $2, started_at = NULL, updated_at = now(),
        run_at = now() + make_interval(secs => $3)
    WHERE id = $1";
const REQUEUE_STALE: &str = "UPDATE axa_queue
    SET status = 'queued', started_at = NULL, updated_at = now()
    WHERE status = 'running' AND started_at < now() - make_interval(mins => $1)
      AND attempts < $2";
const FAIL_STALE: &str = "UPDATE axa_queue
    SET status = 'failed', error = 'abandoned by its worker', updated_at = now()
    WHERE status = 'running' AND started_at < now() - make_interval(mins => $1)
      AND attempts >= $2";
const PURGE: &str = "DELETE FROM axa_queue
    WHERE status IN ('completed', 'failed')
      AND updated_at < now() - make_interval(days => $1)";

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

/// Queue tuning.
#[derive(Debug, Clone)]
pub struct QueueOptions {
    /// Concurrent job loops.
    pub workers: usize,
    /// Sleep between polls of an empty queue.
    pub poll_interval: Duration,
    /// Jobs running longer than this many minutes are assumed lost with
    /// their worker.
    pub stale_minutes: i32,
    /// Tries a job gets, including the first. One means no retries.
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
        let id = Uuid::new_v4();
        sqlx::query(ENQUEUE)
            .bind(id)
            .bind(name)
            .bind(args)
            .execute(&self.pool)
            .await?;
        Ok(id.to_string())
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
        let outcome = match self.handlers.get(&name) {
            Some(handler) => handler(args).await,
            None => Err(Error::Worker(format!("no handler for job {name}"))),
        };
        match outcome {
            Ok(()) => {
                sqlx::query(FINISH)
                    .bind(id)
                    .bind("completed")
                    .bind(None::<String>)
                    .execute(&self.pool)
                    .await?;
            }
            Err(error) if attempts < self.options.max_attempts => {
                tracing::warn!(job = %name, %id, attempts, %error, "job failed, will retry");
                let wait = backoff(self.options.retry_backoff, attempts);
                sqlx::query(RETRY)
                    .bind(id)
                    .bind(error.to_string())
                    .bind(wait.as_secs_f64())
                    .execute(&self.pool)
                    .await?;
            }
            Err(error) => {
                tracing::error!(job = %name, %id, attempts, %error, "job failed");
                sqlx::query(FINISH)
                    .bind(id)
                    .bind("failed")
                    .bind(error.to_string())
                    .execute(&self.pool)
                    .await?;
            }
        }
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
            "UPDATE axa_queue SET status = 'running', started_at = now() - interval '2 hours'
             WHERE id = $1",
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
             SET status = 'running', attempts = $2, started_at = now() - interval '2 hours'
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
