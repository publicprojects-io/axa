//! The Postgres job queue, from [`axa_worker`], with [`BackgroundWorker`] jobs.
//!
//! The engine is `axa-worker`: the `axa_queue` table, claims with `FOR UPDATE SKIP
//! LOCKED`, retries, stale recovery, retention, the pool of job loops, delayed jobs,
//! priorities, job status, and recurring schedules. Its crate documentation says how
//! each behaves. This module puts the [`BackgroundWorker`] trait over it, so a job
//! type is registered by its type and queued by [`BackgroundWorker::perform_later`],
//! and its errors are [`crate::Error`].
//!
//! # Running it
//!
//! Nothing runs jobs by itself. A service needs a process calling [`Queue::run`],
//! usually through `start --all` or `start --worker`, and [`warn_if_unserved`] says
//! so at boot when it has none. Several processes may run at once.

use std::{sync::Arc, time::Duration};

use serde::{Serialize, de::DeserializeOwned};
use sqlx::{PgConnection, PgExecutor, PgPool};
use tokio::sync::watch;
use uuid::Uuid;

pub use axa_worker::{
    Counts, DateTime, JobInfo, JobState, NewJob, QueueOptions, SETUP_STATEMENTS, Schedule,
    ScheduleInfo, Utc,
};

use crate::{AppContext, Error, Result, worker::BackgroundWorker};

impl From<axa_worker::Error> for Error {
    fn from(error: axa_worker::Error) -> Self {
        match error {
            axa_worker::Error::Database(error) => Self::Sqlx(error),
            axa_worker::Error::Encode(error) => Self::JSON(error),
            axa_worker::Error::Job(text) => Self::Worker(text),
            other => Self::Worker(other.to_string()),
        }
    }
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
            "jobs are queued but this process runs no worker: use `start --all`, \
             or run `start --worker` separately"
        );
    }
    unserved
}

/// A job for worker `W` with `args`, with `W`'s name and limit on tries, to delay,
/// prioritise, or queue in a batch.
///
/// ```ignore
/// let later = queue::job::<ReportWorker, _>(&Args { id })?.run_in(Duration::from_mins(5));
/// queue::enqueue_job_on(&mut *transaction, &later).await?;
/// ```
///
/// # Errors
///
/// Fails when the arguments cannot be encoded.
pub fn job<W, A>(args: &A) -> Result<NewJob>
where
    W: BackgroundWorker<A>,
    A: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    let job =
        NewJob::new(W::class_name(), serde_json::to_value(args)?).max_attempts(W::max_attempts());
    Ok(match W::timeout() {
        Some(limit) => job.timeout(limit),
        None => job,
    })
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
    enqueue_job_on(executor, &job::<W, A>(args)?).await
}

/// Add `job` on `executor`, usually the caller's transaction, and return its id.
/// Build `job` with [`job`] to delay it or give it a priority.
///
/// # Errors
///
/// Fails on a database error.
pub async fn enqueue_job_on<'e, E: PgExecutor<'e>>(executor: E, job: &NewJob) -> Result<String> {
    Ok(axa_worker::enqueue_on(executor, job).await?.to_string())
}

/// Add every job in `jobs` on `connection`, returning their ids in order. On a
/// transaction they commit or roll back together.
///
/// # Errors
///
/// Fails on a database error.
pub async fn enqueue_all_on(connection: &mut PgConnection, jobs: &[NewJob]) -> Result<Vec<String>> {
    let ids = axa_worker::enqueue_all_on(connection, jobs).await?;
    Ok(ids.iter().map(Uuid::to_string).collect())
}

/// A Postgres-backed job queue whose jobs are [`BackgroundWorker`]s.
#[derive(Debug)]
pub struct Queue {
    /// Shared only once [`Queue::run`] starts, which takes the queue whole, so
    /// registering through `&mut self` always has it alone.
    engine: Arc<axa_worker::Queue>,
}

impl Queue {
    fn wrap(engine: axa_worker::Queue) -> Self {
        Self {
            engine: Arc::new(engine),
        }
    }

    fn engine_mut(&mut self) -> &mut axa_worker::Queue {
        Arc::get_mut(&mut self.engine).expect("a queue is shared only once it runs")
    }

    /// Connect to tables that a migration made, and change no schema, for a database
    /// role that may not. A migration runs [`SETUP_STATEMENTS`].
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be reached, or a table is missing or out of
    /// date, so a service finds out at boot rather than at its first job.
    pub async fn connect_existing(uri: &str, options: QueueOptions) -> Result<Self> {
        Ok(Self::wrap(
            axa_worker::Queue::connect_existing(uri, options).await?,
        ))
    }

    /// Connect and create the tables if needed, which runs DDL at boot. Use
    /// [`Queue::connect_existing`] where a service's role may not.
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be reached or the tables not created.
    pub async fn connect(uri: &str, options: QueueOptions) -> Result<Self> {
        Ok(Self::wrap(axa_worker::Queue::connect(uri, options).await?))
    }

    /// The engine, for what this module does not wrap.
    #[must_use]
    pub fn engine(&self) -> &axa_worker::Queue {
        &self.engine
    }

    /// The pool, for a service's own queries.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        self.engine.pool()
    }

    /// Register a worker to handle its jobs.
    pub fn register<A, W>(&mut self, worker: W)
    where
        A: Serialize + DeserializeOwned + Send + Sync + 'static,
        W: BackgroundWorker<A> + 'static,
    {
        let worker = Arc::new(worker);
        self.engine_mut().handle(W::class_name(), move |args| {
            let worker = worker.clone();
            async move {
                let args: A = serde_json::from_value(args).map_err(Error::from)?;
                worker.perform(args).await
            }
        });
    }

    /// Run at most `jobs` of worker `W`'s jobs at once in this process, and at least
    /// one. The limit is per process, not across replicas.
    pub fn limit<A, W>(&mut self, jobs: usize)
    where
        A: Serialize + DeserializeOwned + Send + Sync + 'static,
        W: BackgroundWorker<A>,
    {
        self.engine_mut().limit(W::class_name(), jobs);
    }

    /// Queue a job of worker `W` with `args` every `interval`, under the schedule
    /// name `name`. See [`Schedule`] for the first time and what happens to missed
    /// and overlapping times.
    ///
    /// # Errors
    ///
    /// Fails when the arguments cannot be encoded.
    ///
    /// # Panics
    ///
    /// When the name is empty, over 200 characters, or used twice, or the interval
    /// is under a second.
    pub fn every<A, W>(&mut self, name: &str, interval: Duration, args: &A) -> Result<()>
    where
        A: Serialize + DeserializeOwned + Send + Sync + 'static,
        W: BackgroundWorker<A>,
    {
        let job = job::<W, A>(args)?;
        self.engine_mut()
            .schedule(Schedule::every(name, interval, job));
        Ok(())
    }

    /// Add a schedule built with [`Schedule::every`], as for a first time.
    ///
    /// # Panics
    ///
    /// When a schedule of that name was added already.
    pub fn schedule(&mut self, schedule: Schedule) {
        self.engine_mut().schedule(schedule);
    }

    /// Add a job and return its id.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn enqueue(&self, name: &str, args: serde_json::Value) -> Result<String> {
        Ok(self.engine.enqueue(name, args).await?.to_string())
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
        Ok(self
            .engine
            .enqueue_limited(name, args, max_attempts)
            .await?
            .to_string())
    }

    /// Add `job` and return its id.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn enqueue_job(&self, job: &NewJob) -> Result<String> {
        Ok(self.engine.enqueue_job(job).await?.to_string())
    }

    /// Add every job in `jobs` in one transaction, so all are queued or none.
    ///
    /// # Errors
    ///
    /// Fails on a database error, and then none was queued.
    pub async fn enqueue_all(&self, jobs: &[NewJob]) -> Result<Vec<String>> {
        let ids = self.engine.enqueue_all(jobs).await?;
        Ok(ids.iter().map(Uuid::to_string).collect())
    }

    /// What the queue knows of the job `id`, or `None` when there is no such job,
    /// including when `id` is not one.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn job(&self, id: &str) -> Result<Option<JobInfo>> {
        let Ok(id) = Uuid::parse_str(id) else {
            return Ok(None);
        };
        Ok(self.engine.job(id).await?)
    }

    /// Cancel the job `id` if it is still queued, returning whether it was.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn cancel(&self, id: &str) -> Result<bool> {
        let Ok(id) = Uuid::parse_str(id) else {
            return Ok(false);
        };
        Ok(self.engine.cancel(id).await?)
    }

    /// Queue the failed job `id` again, now, for one more try, returning whether it
    /// had failed.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn retry(&self, id: &str) -> Result<bool> {
        let Ok(id) = Uuid::parse_str(id) else {
            return Ok(false);
        };
        Ok(self.engine.retry(id).await?)
    }

    /// How many jobs are in each state, for metrics.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn counts(&self) -> Result<Counts> {
        Ok(self.engine.counts().await?)
    }

    /// A queue on a pool nothing listens to, for testing what depends on it.
    #[cfg(all(test, not(feature = "sea-orm")))]
    pub(crate) fn unreachable() -> Self {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy("postgres://axa:axa@127.0.0.1:1/axa")
            .unwrap();
        Self::wrap(axa_worker::Queue::from_pool(pool, QueueOptions::default()))
    }

    /// Check the database connection.
    ///
    /// # Errors
    ///
    /// Fails when the database does not answer.
    pub async fn ping(&self) -> Result<()> {
        Ok(self.engine.ping().await?)
    }

    /// Recover jobs abandoned by a worker that died: queue again those with
    /// attempts left and fail the rest, returning how many were queued again.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn requeue_stale(&self) -> Result<u64> {
        Ok(self.engine.requeue_stale().await?)
    }

    /// Delete finished and cancelled rows older than the retention period,
    /// returning how many went.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn purge_finished(&self) -> Result<u64> {
        Ok(self.engine.purge_finished().await?)
    }

    /// Queue a job for each schedule that is due, returning how many were queued.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn tick_schedules(&self) -> Result<usize> {
        Ok(self.engine.tick_schedules().await?)
    }

    /// Every schedule the table holds, archived or not, with its last job's state.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn schedules(&self) -> Result<Vec<ScheduleInfo>> {
        Ok(self.engine.schedules().await?)
    }

    /// Archive the schedules no live process has in code, returning how many.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn archive_schedules(&self) -> Result<u64> {
        Ok(self.engine.archive_schedules().await?)
    }

    /// Claim and run one job, returning whether there was one. A job's own
    /// failure is recorded on its row, not returned.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn run_one(&self) -> Result<bool> {
        Ok(self.engine.run_one().await?)
    }

    /// Run job loops, housekeeping, and schedules until `shutdown` changes to
    /// `true`. Jobs in progress finish first. `start` bounds that wait by the
    /// shutdown time limit, and stops the jobs still running at it.
    pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>) {
        self.engine.clone().run(shutdown).await;
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
    use sqlx::{Row, postgres::PgPoolOptions};
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
            .fetch_one(queue.pool())
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
        sqlx::query(hours_ago).execute(queue.pool()).await.unwrap();
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
            .execute(queue.pool())
            .await
            .unwrap();
        assert!(worker.await.unwrap());
        // It cannot write its result, and stops for a person to look.
        assert_eq!(status_of(&queue, &id).await.0, "needs_review");
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
            header_read_timeout_seconds: 0,
            keep_alive_timeout_seconds: 0,
            send_stall_timeout_seconds: 0,
            max_connections: 1,
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
            header_read_timeout_seconds: 0,
            keep_alive_timeout_seconds: 0,
            send_stall_timeout_seconds: 0,
            max_connections: 1,
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
    async fn requests_and_jobs_share_one_limit_and_no_job_starts_during_it() {
        let _table = TABLE.lock().await;
        let mut queue = queue_with(QueueOptions {
            workers: 2,
            poll_interval: Duration::from_millis(20),
            ..QueueOptions::default()
        })
        .await;
        queue.register(HangWorker);
        queue.register(CountWorker);
        let queue = Arc::new(queue);
        queue
            .enqueue("HangWorker", serde_json::json!(null))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let config = crate::config::Server {
            shutdown_timeout_seconds: 1,
            header_read_timeout_seconds: 0,
            keep_alive_timeout_seconds: 0,
            send_stall_timeout_seconds: 0,
            max_connections: 1,
            port: address.port(),
            ..local_server()
        };
        let router = axum::Router::new().route(
            "/",
            axum::routing::get(std::future::pending::<&'static str>),
        );
        let client = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                if let Ok(mut stream) = tokio::net::TcpStream::connect(address).await {
                    stream
                        .write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
                        .await
                        .unwrap();
                    let mut held = Vec::new();
                    let _ = stream.read_to_end(&mut held).await;
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        // This job is queued after the stop, so a loop that heard leaves it alone.
        let stop = tokio::time::sleep(Duration::from_millis(300));
        let late = {
            let queue = queue.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(600)).await;
                let args = serde_json::json!({ "fail": false });
                queue.enqueue("CountWorker", args).await.unwrap()
            })
        };
        let run = crate::server::run_until(Some(router), Some(queue.clone()), &config, stop);
        // Told at 0.3 seconds, with one second to go. Two separate limits would
        // take until 2.3.
        let ended = tokio::time::timeout(Duration::from_millis(1800), run).await;
        client.abort();
        ended.expect("shutdown took longer than its limit").unwrap();
        let late = late.await.unwrap();
        assert_eq!(status_of(&queue, &late).await.0, "queued");
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

        let mut transaction = queue.pool().begin().await.unwrap();
        let dropped = enqueue_on::<CountWorker, _, _>(&mut *transaction, &args)
            .await
            .unwrap();
        transaction.rollback().await.unwrap();
        let row = sqlx::query("SELECT 1 AS n FROM axa_queue WHERE id = $1")
            .bind(Uuid::parse_str(&dropped).unwrap())
            .fetch_optional(queue.pool())
            .await
            .unwrap();
        assert!(row.is_none());

        let mut transaction = queue.pool().begin().await.unwrap();
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
            .execute(queue.pool())
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
            .execute(queue.pool())
            .await
            .unwrap();
        queue.register(CountWorker);
        queue
    }

    async fn status_of(queue: &Queue, id: &str) -> (String, Option<String>) {
        let row = sqlx::query("SELECT status, error FROM axa_queue WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(queue.pool())
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
        .execute(queue.pool())
        .await
        .unwrap();
        assert_eq!(queue.requeue_stale().await.unwrap(), 1);
        assert_eq!(status_of(&queue, &stale).await.0, "queued");
        assert!(queue.run_one().await.unwrap());

        // Concurrent loops run each job exactly once.
        sqlx::query("DELETE FROM axa_queue")
            .execute(queue.pool())
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
                .fetch_one(queue.pool())
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
        let backoff = axa_worker::backoff;
        assert_eq!(backoff(base, 1), base);
        assert_eq!(backoff(base, 2), base * 2);
        assert_eq!(backoff(base, 3), base * 4);
        assert_eq!(backoff(base, 1_000), Duration::from_hours(1));
    }

    #[test]
    fn a_service_without_a_queue_gets_no_worker_warning() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        let ctx = AppContext::for_test(Environment::Test, config);
        assert!(!warn_if_unserved(&ctx, false));
    }

    async fn queue_with(options: QueueOptions) -> Queue {
        let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
        let mut queue = Queue::connect(&uri, options).await.unwrap();
        sqlx::query("DELETE FROM axa_queue")
            .execute(queue.pool())
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
        .execute(queue.pool())
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
            let pool = queue.pool().clone();
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
    async fn connecting_to_an_existing_table_changes_no_schema() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        let uri = std::env::var("AXA_TEST_DATABASE_URL").unwrap();
        assert!(
            Queue::connect_existing(&uri, QueueOptions::default())
                .await
                .is_ok()
        );

        sqlx::query("ALTER TABLE axa_queue DROP COLUMN max_attempts")
            .execute(queue.pool())
            .await
            .unwrap();
        let outdated = Queue::connect_existing(&uri, QueueOptions::default()).await;
        assert!(outdated.is_err());
        let columns: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_name = 'axa_queue' AND column_name = 'max_attempts'",
        )
        .fetch_one(queue.pool())
        .await
        .unwrap();
        assert_eq!(columns, 0);

        // Put the column back for the tests that follow.
        for statement in SETUP_STATEMENTS {
            sqlx::query(statement).execute(queue.pool()).await.unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn the_attempts_column_is_added_to_an_older_table() {
        let _table = TABLE.lock().await;
        let queue = queue().await;
        sqlx::query("ALTER TABLE axa_queue DROP COLUMN attempts")
            .execute(queue.pool())
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
        let mut ctx = AppContext::for_test(Environment::Test, config);
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
        let mut ctx = AppContext::for_test(Environment::Test, config);
        let queue = Arc::new(queue().await);
        ctx.queue_provider = Some(queue.clone());
        let id = CountWorker::perform_later(&ctx, Args { fail: false })
            .await
            .unwrap();
        assert_eq!(status_of(&queue, &id).await.0, "queued");
    }

    /// A worker with a time limit of its own.
    struct TimedWorker;

    #[async_trait]
    impl BackgroundWorker<()> for TimedWorker {
        fn build(_ctx: &AppContext) -> Self {
            Self
        }

        fn timeout() -> Option<Duration> {
            Some(Duration::from_secs(5))
        }

        async fn perform(&self, (): ()) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn perform_in_delays_a_queued_job_and_carries_its_workers_timeout() {
        let _table = TABLE.lock().await;
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |name| {
            (name == "WORKER_MODE").then(|| "BackgroundQueue".to_string())
        })
        .unwrap();
        let mut ctx = AppContext::for_test(Environment::Test, config);
        let mut queue = queue().await;
        queue.register(TimedWorker);
        let queue = Arc::new(queue);
        ctx.queue_provider = Some(queue.clone());

        let later = TimedWorker::perform_in(&ctx, (), Duration::from_hours(1))
            .await
            .unwrap();
        let info = queue.job(&later).await.unwrap().unwrap();
        assert_eq!(info.state, JobState::Queued);
        assert_eq!(info.timeout, Some(Duration::from_secs(5)));
        assert!((info.run_at - info.created_at).num_minutes() >= 59);
        assert!(!queue.run_one().await.unwrap(), "not before its time");

        // A worker that sets no limit gives its jobs none.
        let plain = CountWorker::perform_later(&ctx, Args { fail: false })
            .await
            .unwrap();
        assert_eq!(queue.job(&plain).await.unwrap().unwrap().timeout, None);
        assert!(queue.job("not an id").await.unwrap().is_none());
    }
}
