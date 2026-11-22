#![allow(
    clippy::arithmetic_side_effects,
    reason = "RCA-SCOPE-4: tests may use plain operators"
)]
//! The queue against a real MySQL 8.4. The tests share three tables, so run them with
//! `--test-threads=1`. `axa_job_acts` refuses `DELETE`, so a test starts by dropping
//! it.
//!
//! ```sh
//! AXA_TEST_MYSQL_URL=mysql://... cargo test -p axa-mysql-job-queue -- --ignored --test-threads=1
//! ```

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axa_mysql_job_queue::{
    ActKind, Error, JobState, NewJob, Queue, QueueOptions, SETUP_STATEMENTS, Schedule, Timestamp,
    enqueue_all_on,
};
use jiff::{SignedDuration, tz::TimeZone};
use sqlx::{MySqlPool, Row};
use tokio::sync::watch;
use uuid::Uuid;

fn uri() -> String {
    std::env::var("AXA_TEST_MYSQL_URL").expect("AXA_TEST_MYSQL_URL")
}

fn quick() -> QueueOptions {
    QueueOptions {
        workers: 4,
        poll_interval: Duration::from_millis(20),
        retry_backoff: Duration::ZERO,
        ..QueueOptions::default()
    }
}

/// A queue on empty tables. The acts table is dropped, as it refuses `DELETE`, and
/// `connect` makes it again.
async fn fresh(options: QueueOptions) -> Queue {
    let admin = MySqlPool::connect(&uri()).await.expect("connect");
    sqlx::query("DROP TABLE IF EXISTS axa_job_acts")
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    let queue = Queue::connect(&uri(), options).await.expect("connect");
    for statement in ["DELETE FROM axa_queue", "DELETE FROM axa_schedule"] {
        sqlx::query(statement).execute(queue.pool()).await.unwrap();
    }
    queue
}

/// Another process on the same tables.
async fn replica(options: QueueOptions) -> Queue {
    Queue::connect_existing(&uri(), options)
        .await
        .expect("connect")
}

fn start(queue: Queue) -> (Arc<Queue>, watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let queue = Arc::new(queue);
    let (stop, stopped) = watch::channel(false);
    let run = tokio::spawn(queue.clone().run(stopped));
    (queue, stop, run)
}

async fn until(mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..400 {
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("gave up waiting");
}

async fn state(queue: &Queue, id: Uuid) -> JobState {
    queue.job(id).await.unwrap().expect("a job").state
}

async fn count(pool: &MySqlPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Cancel `id` as a person would, in a transaction of its own, trying again when
/// MySQL picks it as the victim of a deadlock with a loop's claim.
async fn cancel(queue: &Queue, id: Uuid) -> bool {
    loop {
        let mut transaction = queue.pool().begin().await.unwrap();
        match axa_mysql_job_queue::cancel(&mut transaction, id, "ops:alice", "test_cancel").await {
            Ok(cancelled) => {
                transaction.commit().await.unwrap();
                return cancelled;
            }
            Err(Error::Database(sqlx::Error::Database(error)))
                if error.code().as_deref() == Some("40001") =>
            {
                transaction.rollback().await.unwrap();
            }
            Err(error) => panic!("cancel failed: {error}"),
        }
    }
}

/// Retry `id` as a person would, in a transaction of its own.
async fn retry(queue: &Queue, id: Uuid) -> bool {
    let mut transaction = queue.pool().begin().await.unwrap();
    let retried = axa_mysql_job_queue::retry(&mut transaction, id, "ops:alice", "test_retry")
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    retried
}

/// Counts jobs running now and the most ever running at once.
#[derive(Clone, Default)]
struct Gauge {
    now: Arc<AtomicUsize>,
    most: Arc<AtomicUsize>,
    done: Arc<AtomicUsize>,
}

impl Gauge {
    fn handler(
        &self,
        millis: u64,
    ) -> impl Fn(
        serde_json::Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
    + Send
    + Sync
    + 'static {
        let gauge = self.clone();
        move |_| {
            let gauge = gauge.clone();
            Box::pin(async move {
                let now = gauge.now.fetch_add(1, Ordering::SeqCst) + 1;
                gauge.most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(millis)).await;
                gauge.now.fetch_sub(1, Ordering::SeqCst);
                gauge.done.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    fn most(&self) -> usize {
        self.most.load(Ordering::SeqCst)
    }

    fn done(&self) -> usize {
        self.done.load(Ordering::SeqCst)
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_process_runs_no_more_jobs_at_once_than_its_loops() {
    let mut queue = fresh(QueueOptions {
        workers: 3,
        ..quick()
    })
    .await;
    let gauge = Gauge::default();
    queue.handle("Slow", gauge.handler(60));
    for _ in 0..24 {
        queue
            .enqueue("Slow", serde_json::Value::Null)
            .await
            .unwrap();
    }
    let (_queue, stop, run) = start(queue);
    until(async || gauge.done() == 24).await;
    stop.send(true).unwrap();
    run.await.unwrap();
    assert_eq!(
        gauge.most(),
        3,
        "the loops ran in parallel up to their number"
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_limit_holds_one_job_type_and_the_rest_keep_the_loops() {
    let mut queue = fresh(quick()).await;
    let limited = Gauge::default();
    let free = Gauge::default();
    queue.handle("Limited", limited.handler(50));
    queue.handle("Free", free.handler(50));
    queue.limit("Limited", 1);
    for _ in 0..8 {
        queue
            .enqueue("Limited", serde_json::Value::Null)
            .await
            .unwrap();
        queue
            .enqueue("Free", serde_json::Value::Null)
            .await
            .unwrap();
    }
    let (_queue, stop, run) = start(queue);
    until(async || limited.done() == 8 && free.done() == 8).await;
    stop.send(true).unwrap();
    run.await.unwrap();
    assert_eq!(limited.most(), 1);
    assert!(
        free.most() >= 2,
        "free jobs used the other loops: {}",
        free.most()
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn two_processes_run_each_job_once() {
    let mut first = fresh(quick()).await;
    let mut second = replica(quick()).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    for queue in [&mut first, &mut second] {
        let seen = seen.clone();
        queue.handle("Once", move |args: serde_json::Value| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(args["n"].as_i64().unwrap());
                Ok::<(), String>(())
            }
        });
    }
    for n in 0..60 {
        first
            .enqueue("Once", serde_json::json!({ "n": n }))
            .await
            .unwrap();
    }
    let (_a, stop_a, run_a) = start(first);
    let (_b, stop_b, run_b) = start(second);
    until(async || seen.lock().unwrap().len() >= 60).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop_a.send(true).unwrap();
    stop_b.send(true).unwrap();
    run_a.await.unwrap();
    run_b.await.unwrap();
    let seen = seen.lock().unwrap();
    let unique: HashSet<_> = seen.iter().collect();
    assert_eq!((seen.len(), unique.len()), (60, 60));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn jobs_in_flight_finish_at_shutdown_and_dropping_the_run_stops_them() {
    let mut queue = fresh(quick()).await;
    let gauge = Gauge::default();
    queue.handle("Slow", gauge.handler(300));
    let finished = queue
        .enqueue("Slow", serde_json::Value::Null)
        .await
        .unwrap();
    let (queue, stop, run) = start(queue);
    until(async || state(&queue, finished).await == JobState::Running).await;
    stop.send(true).unwrap();
    run.await.unwrap();
    assert_eq!(state(&queue, finished).await, JobState::Completed);

    // A caller with a time limit drops the run at it, and the job stops too.
    let mut queue = fresh(quick()).await;
    let stopped = Gauge::default();
    queue.handle("Slow", stopped.handler(1_000));
    let cut_off = queue
        .enqueue("Slow", serde_json::Value::Null)
        .await
        .unwrap();
    let (queue, _stop, run) = start(queue);
    until(async || state(&queue, cut_off).await == JobState::Running).await;
    run.abort();
    let _ = run.await;
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert_eq!(stopped.done(), 0, "the job was stopped with its loop");
    // Left for stale recovery, as a crash would be.
    assert_eq!(state(&queue, cut_off).await, JobState::Running);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_delayed_job_is_not_claimed_before_its_time() {
    let mut queue = fresh(quick()).await;
    queue.handle("Job", |_| async { Ok::<(), String>(()) });
    let later = queue
        .enqueue_job(&NewJob::new("Job", serde_json::Value::Null).run_in(Duration::from_hours(1)))
        .await
        .unwrap();
    let soon = queue
        .enqueue_job(
            // Far enough off that the checks below finish first, however slow the machine.
            &NewJob::new("Job", serde_json::Value::Null).run_in(Duration::from_secs(2)),
        )
        .await
        .unwrap();
    let past = a_minute_ago(&queue).await;
    let due = queue
        .enqueue_job(&NewJob::new("Job", serde_json::Value::Null).run_at(past))
        .await
        .unwrap();
    assert!(queue.run_one().await.unwrap());
    assert_eq!(state(&queue, due).await, JobState::Completed);
    assert!(!queue.run_one().await.unwrap(), "nothing else is due yet");
    assert_eq!(state(&queue, soon).await, JobState::Queued);
    let until = Instant::now() + Duration::from_secs(20);
    while !queue.run_one().await.unwrap() {
        assert!(Instant::now() < until, "the delayed job never came due");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(state(&queue, soon).await, JobState::Completed);
    assert_eq!(state(&queue, later).await, JobState::Queued);
    let counts = queue.counts().await.unwrap();
    assert_eq!((counts.ready, counts.waiting, counts.completed), (0, 1, 2));
}

async fn a_minute_ago(queue: &Queue) -> Timestamp {
    time_of(queue.pool(), "SELECT UTC_TIMESTAMP(6) - INTERVAL 1 MINUTE").await
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_jobs_due_time_is_the_same_in_two_session_time_zones() {
    async fn at_offset(offset: &str) -> Queue {
        let options = uri()
            .parse::<sqlx::mysql::MySqlConnectOptions>()
            .unwrap()
            .timezone(offset.to_owned());
        let pool = MySqlPool::connect_with(options).await.unwrap();
        let actual: String = sqlx::query_scalar("SELECT @@session.time_zone")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(actual, offset);
        Queue::from_pool(pool, quick())
    }

    let setup = fresh(quick()).await;
    setup.pool().close().await;
    let producer = at_offset("+05:45").await;
    let mut worker = at_offset("-03:30").await;
    worker.handle("Job", |_| async { Ok::<(), String>(()) });
    let future = time_of(
        producer.pool(),
        "SELECT UTC_TIMESTAMP() + INTERVAL 1 HOUR + INTERVAL 123456 MICROSECOND",
    )
    .await;
    let due = a_minute_ago(&producer).await;
    let job = NewJob::new("Job", serde_json::Value::Null);
    let later = producer
        .enqueue_job(&job.clone().run_at(future))
        .await
        .unwrap();
    let now = producer.enqueue_job(&job.run_at(due)).await.unwrap();

    assert_eq!(worker.job(later).await.unwrap().unwrap().run_at, future);
    assert!(worker.run_one().await.unwrap());
    assert_eq!(state(&worker, now).await, JobState::Completed);
    assert!(!worker.run_one().await.unwrap());
    assert_eq!(state(&worker, later).await, JobState::Queued);
}

/// The one time a query gives, a `DATETIME` in UTC.
async fn time_of(pool: &MySqlPool, sql: &str) -> Timestamp {
    let micros: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT TIMESTAMPDIFF(MICROSECOND, '1970-01-01 00:00:00', ({sql}))"
    )))
    .fetch_one(pool)
    .await
    .unwrap();
    Timestamp::from_microsecond(micros).unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_higher_priority_runs_first() {
    let mut queue = fresh(quick()).await;
    let order = Arc::new(Mutex::new(Vec::new()));
    {
        let order = order.clone();
        queue.handle("Job", move |args: serde_json::Value| {
            let order = order.clone();
            async move {
                order.lock().unwrap().push(args.as_i64().unwrap());
                Ok::<(), String>(())
            }
        });
    }
    for (n, priority) in [(1, 0), (2, 5), (3, -1), (4, 5)] {
        queue
            .enqueue_job(&NewJob::new("Job", serde_json::json!(n)).priority(priority))
            .await
            .unwrap();
    }
    while queue.run_one().await.unwrap() {}
    assert_eq!(*order.lock().unwrap(), vec![2, 4, 1, 3]);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_job_status_follows_it_and_leaves_out_its_arguments() {
    let mut queue = fresh(quick()).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    {
        let gate = gate.clone();
        queue.handle("Gated", move |args: serde_json::Value| {
            let gate = gate.clone();
            async move {
                gate.notified().await;
                if args["fail"].as_bool().unwrap() {
                    Err("x".repeat(5_000))
                } else {
                    Ok(())
                }
            }
        });
    }
    let ok = queue
        .enqueue("Gated", serde_json::json!({ "fail": false }))
        .await
        .unwrap();
    let info = queue.job(ok).await.unwrap().unwrap();
    assert_eq!(
        (info.state, info.attempts, info.name.as_str()),
        (JobState::Queued, 0, "Gated")
    );
    assert!(!format!("{info:?}").contains("fail"));
    let queue = Arc::new(queue);
    let run = {
        let queue = queue.clone();
        tokio::spawn(async move { queue.run_one().await.unwrap() })
    };
    until(async || state(&queue, ok).await == JobState::Running).await;
    let info = queue.job(ok).await.unwrap().unwrap();
    assert!(info.started_at.is_some());
    assert_eq!(info.attempts, 1);
    gate.notify_one();
    assert!(run.await.unwrap());
    assert_eq!(state(&queue, ok).await, JobState::Completed);

    let bad = queue
        .enqueue("Gated", serde_json::json!({ "fail": true }))
        .await
        .unwrap();
    let run = {
        let queue = queue.clone();
        tokio::spawn(async move { queue.run_one().await.unwrap() })
    };
    until(async || state(&queue, bad).await == JobState::Running).await;
    gate.notify_one();
    run.await.unwrap();
    let info = queue.job(bad).await.unwrap().unwrap();
    assert_eq!(info.state, JobState::Failed);
    let error = info.error.unwrap();
    assert!(
        error.len() <= 1024 && error.starts_with("xxx"),
        "{}",
        error.len()
    );
    assert!(queue.job(Uuid::new_v4()).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_cancelled_job_never_runs_and_a_running_one_cannot_be_cancelled() {
    let mut queue = fresh(quick()).await;
    let gauge = Gauge::default();
    queue.handle("Slow", gauge.handler(200));
    let cancelled = queue
        .enqueue("Slow", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(cancel(&queue, cancelled).await);
    assert!(!cancel(&queue, cancelled).await, "only once");
    assert!(!queue.run_one().await.unwrap());
    assert_eq!(state(&queue, cancelled).await, JobState::Cancelled);

    let running = queue
        .enqueue("Slow", serde_json::Value::Null)
        .await
        .unwrap();
    let queue = Arc::new(queue);
    let run = {
        let queue = queue.clone();
        tokio::spawn(async move { queue.run_one().await.unwrap() })
    };
    until(async || state(&queue, running).await == JobState::Running).await;
    assert!(!cancel(&queue, running).await);
    run.await.unwrap();
    assert_eq!(state(&queue, running).await, JobState::Completed);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn cancelling_while_loops_claim_runs_each_job_or_cancels_it_never_both() {
    let mut queue = fresh(quick()).await;
    let ran = Arc::new(Mutex::new(HashSet::new()));
    {
        let ran = ran.clone();
        queue.handle("Job", move |args: serde_json::Value| {
            let ran = ran.clone();
            async move {
                ran.lock()
                    .unwrap()
                    .insert(args.as_str().unwrap().to_string());
                Ok::<(), String>(())
            }
        });
    }
    let mut ids = Vec::new();
    for n in 0..80 {
        let id = Uuid::new_v4().to_string();
        let job = queue.enqueue("Job", serde_json::json!(id)).await.unwrap();
        ids.push((n, id, job));
    }
    let (queue, stop, run) = start(queue);
    let mut cancelled = HashSet::new();
    for (_, name, id) in &ids {
        if cancel(&queue, *id).await {
            cancelled.insert(name.clone());
        }
    }
    until(async || queue.counts().await.unwrap().ready == 0).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.send(true).unwrap();
    run.await.unwrap();
    let ran = ran.lock().unwrap();
    assert_eq!(ran.len() + cancelled.len(), 80);
    assert!(ran.is_disjoint(&cancelled));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_failed_job_retried_gets_one_more_try_and_a_lost_run_cannot_overwrite_it() {
    let mut queue = fresh(quick()).await;
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let fail = fail.clone();
        queue.handle("Flaky", move |_| {
            let fail = fail.clone();
            async move {
                if fail.load(Ordering::SeqCst) {
                    Err("down".to_string())
                } else {
                    Ok(())
                }
            }
        });
    }
    let id = queue
        .enqueue("Flaky", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(queue.run_one().await.unwrap());
    assert_eq!(state(&queue, id).await, JobState::Failed);
    assert!(!retry(&queue, Uuid::new_v4()).await);
    assert!(retry(&queue, id).await);
    assert!(!retry(&queue, id).await, "it is queued, not failed");
    let info = queue.job(id).await.unwrap().unwrap();
    assert_eq!((info.state, info.max_attempts), (JobState::Queued, Some(2)));

    // A run lost at attempt one still holds its old claim, and its result is
    // refused.
    let lost = sqlx::query(
        "UPDATE axa_queue SET status = 'failed', error = 'from the lost run', updated_at = UTC_TIMESTAMP(6)
         WHERE id = ? AND attempts = 1 AND status = 'running'",
    )
    .bind(id)
    .execute(queue.pool())
    .await
    .unwrap();
    assert_eq!(lost.rows_affected(), 0);

    fail.store(false, Ordering::SeqCst);
    assert!(queue.run_one().await.unwrap());
    let info = queue.job(id).await.unwrap().unwrap();
    assert_eq!((info.state, info.attempts), (JobState::Completed, 2));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_batch_is_queued_whole_or_not_at_all() {
    let queue = fresh(quick()).await;
    let jobs: Vec<_> = (0..3)
        .map(|n| NewJob::new("Job", serde_json::json!(n)))
        .collect();
    let ids = queue.enqueue_all(&jobs).await.unwrap();
    assert_eq!(ids.len(), 3);
    for id in &ids {
        assert_eq!(state(&queue, *id).await, JobState::Queued);
    }
    let mut transaction = queue.pool().begin().await.unwrap();
    let dropped = enqueue_all_on(&mut transaction, &jobs).await.unwrap();
    transaction.rollback().await.unwrap();
    for id in dropped {
        assert!(queue.job(id).await.unwrap().is_none());
    }
    assert_eq!(
        count(queue.pool(), "SELECT count(*) FROM axa_queue").await,
        3
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn connecting_to_tables_missing_or_out_of_date_fails_and_changes_nothing() {
    let queue = fresh(quick()).await;
    sqlx::query("DROP TABLE axa_schedule")
        .execute(queue.pool())
        .await
        .unwrap();
    assert!(Queue::connect_existing(&uri(), quick()).await.is_err());
    sqlx::query("ALTER TABLE axa_queue DROP COLUMN priority")
        .execute(queue.pool())
        .await
        .unwrap();
    assert!(Queue::connect_existing(&uri(), quick()).await.is_err());
    assert_eq!(
        count(
            queue.pool(),
            "SELECT count(*) FROM information_schema.tables
             WHERE table_schema = DATABASE() AND table_name = 'axa_schedule'"
        )
        .await,
        0
    );
    // `CREATE TABLE IF NOT EXISTS` leaves a table it finds, so an old one goes first.
    sqlx::query("DROP TABLE axa_queue")
        .execute(queue.pool())
        .await
        .unwrap();
    for statement in SETUP_STATEMENTS {
        sqlx::raw_sql(statement)
            .execute(queue.pool())
            .await
            .unwrap();
    }
    assert!(Queue::connect_existing(&uri(), quick()).await.is_ok());
}

// Schedules.

fn every(queue: &mut Queue, name: &str, interval: Duration) {
    queue
        .schedule(
            Schedule::every(
                name,
                interval,
                NewJob::new("Tick", serde_json::json!({ "from": name })),
            )
            .unwrap(),
        )
        .unwrap();
}

/// Mark completed every unfinished job no other connection holds.
async fn finish_unheld(pool: &MySqlPool) {
    let mut transaction = pool.begin().await.unwrap();
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM axa_queue WHERE status <> 'completed' ORDER BY id FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    for id in ids {
        sqlx::query("UPDATE axa_queue SET status = 'completed' WHERE id = ?")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();
}

async fn jobs_named(pool: &MySqlPool, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM axa_queue WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Seconds between the row's due time and `from`, and between now and the due
/// time, on the database's clock.
async fn due(pool: &MySqlPool, name: &str) -> (f64, f64) {
    let row = sqlx::query(
        "SELECT TIMESTAMPDIFF(MICROSECOND, created_at, due_at) / 1e6 AS since,
                TIMESTAMPDIFF(MICROSECOND, UTC_TIMESTAMP(6), due_at) / 1e6 AS ahead
         FROM axa_schedule WHERE name = ?",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap();
    (row.get("since"), row.get("ahead"))
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn replicas_ticking_together_queue_each_due_time_once() {
    let mut replicas = Vec::new();
    replicas.push(fresh(quick()).await);
    for _ in 0..5 {
        replicas.push(replica(quick()).await);
    }
    for queue in &mut replicas {
        every(queue, "race", Duration::from_secs(1));
    }
    let pool = replicas[0].pool().clone();
    let replicas: Vec<_> = replicas.into_iter().map(Arc::new).collect();
    // Five due times, every replica ticking as fast as it can.
    let ticks: Vec<_> = replicas
        .iter()
        .map(|queue| {
            let queue = queue.clone();
            tokio::spawn(async move {
                let end = tokio::time::Instant::now() + Duration::from_millis(4_500);
                let mut mine = 0;
                while tokio::time::Instant::now() < end {
                    mine += queue.tick_schedules().await.unwrap();
                    // A due time is skipped while its last job is queued, so
                    // finish each job as soon as it is there. Only rows no other
                    // replica holds, so that six of these never deadlock.
                    finish_unheld(queue.pool()).await;
                }
                mine
            })
        })
        .collect();
    let mut total = 0;
    for tick in ticks {
        total += tick.await.unwrap();
    }
    let jobs = jobs_named(&pool, "Tick").await;
    assert_eq!(i64::try_from(total).unwrap(), jobs);
    assert!(
        (4..=6).contains(&jobs),
        "one job a second for 4.5 seconds: {jobs}"
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn missed_times_queue_one_job_and_the_schedule_keeps_its_place() {
    let mut queue = fresh(quick()).await;
    every(&mut queue, "missed", Duration::from_secs(10));
    assert_eq!(queue.tick_schedules().await.unwrap(), 1);
    sqlx::query("UPDATE axa_queue SET status = 'completed'")
        .execute(queue.pool())
        .await
        .unwrap();
    // As if every process had been down for about 95 seconds.
    sqlx::query(
        "UPDATE axa_schedule SET created_at = created_at - INTERVAL 95 SECOND,
                due_at = due_at - INTERVAL 95 SECOND",
    )
    .execute(queue.pool())
    .await
    .unwrap();
    assert_eq!(queue.tick_schedules().await.unwrap(), 1);
    assert_eq!(queue.tick_schedules().await.unwrap(), 0);
    assert_eq!(jobs_named(queue.pool(), "Tick").await, 2);
    let (since, ahead) = due(queue.pool(), "missed").await;
    assert!(
        ahead > 0.0 && ahead <= 10.0,
        "the next time is ahead: {ahead}"
    );
    assert!(
        ((since / 10.0) - (since / 10.0).round()).abs() < 0.01,
        "still on the grid of the first time: {since}"
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_due_time_is_skipped_while_the_last_job_has_not_finished() {
    let mut queue = fresh(quick()).await;
    every(&mut queue, "busy", Duration::from_secs(1));
    assert_eq!(queue.tick_schedules().await.unwrap(), 1);
    for state in ["queued", "running"] {
        sqlx::query("UPDATE axa_queue SET status = ?")
            .bind(state)
            .execute(queue.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE axa_schedule SET due_at = UTC_TIMESTAMP(6) - INTERVAL 1 SECOND")
            .execute(queue.pool())
            .await
            .unwrap();
        assert_eq!(
            queue.tick_schedules().await.unwrap(),
            0,
            "skipped while {state}"
        );
        let (_, ahead) = due(queue.pool(), "busy").await;
        assert!(ahead > 0.0, "the skipped time is passed, not kept");
    }
    for state in ["completed", "failed", "cancelled", "needs_review"] {
        sqlx::query("UPDATE axa_queue SET status = ?")
            .bind(state)
            .execute(queue.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE axa_schedule SET due_at = UTC_TIMESTAMP(6) - INTERVAL 1 SECOND")
            .execute(queue.pool())
            .await
            .unwrap();
        assert_eq!(
            queue.tick_schedules().await.unwrap(),
            1,
            "queued after {state}"
        );
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn due_times_count_from_the_first_and_do_not_drift() {
    let mut queue = fresh(quick()).await;
    every(&mut queue, "steady", Duration::from_secs(1));
    queue.handle("Tick", |_| async {
        // Slower than nothing, so a schedule counting from when a job ran would drift.
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok::<(), String>(())
    });
    let (queue, stop, run) = start(queue);
    tokio::time::sleep(Duration::from_millis(3_600)).await;
    stop.send(true).unwrap();
    run.await.unwrap();
    let (since, _) = due(queue.pool(), "steady").await;
    assert!(
        (since - since.round()).abs() < 0.01,
        "whole seconds from the first: {since}"
    );
    let times: Vec<f64> = sqlx::query_scalar(
        "SELECT TIMESTAMPDIFF(MICROSECOND, s.created_at, q.created_at) / 1e6
         FROM axa_queue q, axa_schedule s ORDER BY q.created_at",
    )
    .fetch_all(queue.pool())
    .await
    .unwrap();
    assert!((3..=5).contains(&times.len()), "{times:?}");
    // The due times themselves are checked above. A job is queued once its time has
    // come and before the next one, however late a busy machine wakes the poller.
    for (n, at) in times.iter().enumerate() {
        let n = f64::from(u32::try_from(n).unwrap());
        assert!(*at >= n && *at < n + 1.0, "job {n} queued {at} seconds in");
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_first_time_applies_only_when_the_row_is_made() {
    let mut queue = fresh(quick()).await;
    let first = time_of(queue.pool(), "SELECT UTC_TIMESTAMP() + INTERVAL 1 HOUR").await;
    let job = NewJob::new("Tick", serde_json::Value::Null);
    queue
        .schedule(
            Schedule::every("nightly", Duration::from_hours(24), job.clone())
                .unwrap()
                .first_at(first),
        )
        .unwrap();
    assert_eq!(
        queue.tick_schedules().await.unwrap(),
        0,
        "not due until its first time"
    );
    let stored = time_of(
        queue.pool(),
        "SELECT due_at FROM axa_schedule WHERE name = 'nightly'",
    )
    .await;
    assert_eq!(stored, first);

    // A later release with another first time leaves the row where it is.
    let earlier = time_of(queue.pool(), "SELECT UTC_TIMESTAMP(6) - INTERVAL 5 HOUR").await;
    let mut later = replica(quick()).await;
    later
        .schedule(
            Schedule::every("nightly", Duration::from_hours(24), job)
                .unwrap()
                .first_at(earlier),
        )
        .unwrap();
    later.tick_schedules().await.unwrap();
    let stored = time_of(
        queue.pool(),
        "SELECT due_at FROM axa_schedule WHERE name = 'nightly'",
    )
    .await;
    assert_eq!(stored, first);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_scheduled_job_is_an_ordinary_job_and_shutdown_stops_the_ticks() {
    let mut queue = fresh(quick()).await;
    every(&mut queue, "report", Duration::from_secs(1));
    let runs = Arc::new(AtomicUsize::new(0));
    {
        let runs = runs.clone();
        queue.handle("Tick", move |args: serde_json::Value| {
            let runs = runs.clone();
            async move {
                assert_eq!(args["from"], "report");
                runs.fetch_add(1, Ordering::SeqCst);
                Ok::<(), String>(())
            }
        });
    }
    let (queue, stop, run) = start(queue);
    until(async || runs.load(Ordering::SeqCst) >= 1).await;
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .expect("the ticks stopped")
        .unwrap();
    let after = jobs_named(queue.pool(), "Tick").await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        jobs_named(queue.pool(), "Tick").await,
        after,
        "nothing ticks after shutdown"
    );
    let counts = queue.counts().await.unwrap();
    assert!(counts.completed >= 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_run_that_loses_its_claim_is_stopped_and_waits_for_review() {
    let mut queue = fresh(QueueOptions {
        heartbeat_interval: Duration::from_millis(30),
        ..quick()
    })
    .await;
    let finished = Arc::new(AtomicUsize::new(0));
    {
        let finished = finished.clone();
        queue.handle("Long", move |_| {
            let finished = finished.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(800)).await;
                finished.fetch_add(1, Ordering::SeqCst);
                Ok::<(), String>(())
            }
        });
    }
    let id = queue
        .enqueue("Long", serde_json::Value::Null)
        .await
        .unwrap();
    let queue = Arc::new(queue);
    let run = {
        let queue = queue.clone();
        tokio::spawn(async move { queue.run_one().await.unwrap() })
    };
    until(async || state(&queue, id).await == JobState::Running).await;
    // Another worker took it over, as after a long outage.
    sqlx::query("UPDATE axa_queue SET attempts = attempts + 1, updated_at = UTC_TIMESTAMP(6)")
        .execute(queue.pool())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_millis(500), run)
        .await
        .expect("the run stopped at its next report")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "the job itself was stopped"
    );
    let info = queue.job(id).await.unwrap().unwrap();
    assert_eq!(info.state, JobState::NeedsReview);
    assert!(info.error.unwrap().contains("took its claim"));
    // Nothing retries it by itself.
    sqlx::query("UPDATE axa_queue SET updated_at = UTC_TIMESTAMP(6) - INTERVAL 2 HOUR")
        .execute(queue.pool())
        .await
        .unwrap();
    assert_eq!(queue.requeue_stale().await.unwrap(), 0);
    assert_eq!(queue.purge_finished().await.unwrap(), 0);
    assert!(!queue.run_one().await.unwrap());
    assert_eq!(queue.counts().await.unwrap().needs_review, 1);
    // A person releases it, and the act is kept.
    assert!(retry(&queue, id).await);
    assert_eq!(state(&queue, id).await, JobState::Queued);
    assert_eq!(queue.acts(id).await.unwrap()[0].kind, ActKind::Released);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_run_that_finishes_after_losing_its_claim_waits_for_review() {
    let mut queue = fresh(quick()).await;
    queue.handle("Quick", |_| async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok::<(), String>(())
    });
    let id = queue
        .enqueue("Quick", serde_json::Value::Null)
        .await
        .unwrap();
    let queue = Arc::new(queue);
    let run = {
        let queue = queue.clone();
        tokio::spawn(async move { queue.run_one().await.unwrap() })
    };
    until(async || state(&queue, id).await == JobState::Running).await;
    sqlx::query("UPDATE axa_queue SET attempts = attempts + 1")
        .execute(queue.pool())
        .await
        .unwrap();
    run.await.unwrap();
    assert_eq!(state(&queue, id).await, JobState::NeedsReview);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_job_has_no_time_limit_unless_it_is_given_one() {
    let mut queue = fresh(quick()).await;
    queue.handle("Sleep", |millis: serde_json::Value| async move {
        tokio::time::sleep(Duration::from_millis(millis.as_u64().unwrap())).await;
        Ok::<(), String>(())
    });
    let unlimited = queue
        .enqueue("Sleep", serde_json::json!(1_500))
        .await
        .unwrap();
    assert!(queue.run_one().await.unwrap());
    let info = queue.job(unlimited).await.unwrap().unwrap();
    assert_eq!((info.state, info.timeout), (JobState::Completed, None));

    let limited = queue
        .enqueue_job(
            &NewJob::new("Sleep", serde_json::json!(5_000)).timeout(Duration::from_millis(200)),
        )
        .await
        .unwrap();
    let started = Instant::now();
    assert!(queue.run_one().await.unwrap());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "stopped at its limit"
    );
    let info = queue.job(limited).await.unwrap().unwrap();
    assert_eq!(info.state, JobState::Failed);
    assert_eq!(info.timeout, Some(Duration::from_millis(200)));
    assert_eq!(
        info.error.as_deref(),
        Some("worker error: timed out after 0.2s")
    );
}

async fn archived(pool: &MySqlPool, name: &str) -> Option<bool> {
    sqlx::query_scalar::<_, i64>("SELECT archived_at IS NOT NULL FROM axa_schedule WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
        .map(|archived| archived != 0)
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_schedule_gone_from_code_is_archived_kept_and_restored_when_it_returns() {
    let mut old = fresh(quick()).await;
    every(&mut old, "retired", Duration::from_secs(1));
    every(&mut old, "kept", Duration::from_secs(1));
    assert_eq!(old.tick_schedules().await.unwrap(), 2);
    let leftover =
        sqlx::query_scalar::<_, Uuid>("SELECT last_job FROM axa_schedule WHERE name = 'retired'")
            .fetch_one(old.pool())
            .await
            .unwrap();

    // A new release drops `retired`. While a process may still have it, it stays.
    let mut new = replica(quick()).await;
    every(&mut new, "kept", Duration::from_secs(1));
    assert_eq!(
        new.archive_schedules().await.unwrap(),
        0,
        "seen recently, so maybe still live"
    );
    // Once no process has seen it for `stale_minutes`, it is archived, once.
    sqlx::query("UPDATE axa_schedule SET seen_at = UTC_TIMESTAMP(6) - INTERVAL 2 HOUR")
        .execute(new.pool())
        .await
        .unwrap();
    new.tick_schedules().await.unwrap(); // `kept` is seen again
    assert_eq!(new.archive_schedules().await.unwrap(), 1);
    assert_eq!(new.archive_schedules().await.unwrap(), 0);
    assert_eq!(archived(new.pool(), "retired").await, Some(true));
    assert_eq!(archived(new.pool(), "kept").await, Some(false));

    // Archived, it queues nothing, its row stays, and its last job still runs.
    sqlx::query("UPDATE axa_schedule SET due_at = UTC_TIMESTAMP(6) - INTERVAL 1 MINUTE")
        .execute(new.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE axa_queue SET status = 'completed' WHERE id <> ?")
        .bind(leftover)
        .execute(new.pool())
        .await
        .unwrap();
    assert_eq!(new.tick_schedules().await.unwrap(), 1, "only `kept`");
    assert_eq!(state(&new, leftover).await, JobState::Queued);
    let listed = new.schedules().await.unwrap();
    let retired = listed.iter().find(|s| s.name == "retired").unwrap();
    assert!(retired.archived_at.is_some());
    assert_eq!(retired.interval, Some(Duration::from_secs(1)));
    assert_eq!(retired.last_job, Some(leftover));
    assert_eq!(retired.last_state, Some(JobState::Queued));

    // It returns to code: the same row is restored, keeping its due time.
    let due_before = time_of(
        new.pool(),
        "SELECT due_at FROM axa_schedule WHERE name = 'retired'",
    )
    .await;
    let mut back = replica(quick()).await;
    every(&mut back, "retired", Duration::from_secs(1));
    back.tick_schedules().await.unwrap();
    assert_eq!(archived(back.pool(), "retired").await, Some(false));
    assert_eq!(
        count(
            back.pool(),
            "SELECT count(*) FROM axa_schedule WHERE name = 'retired'"
        )
        .await,
        1
    );
    let due_after = time_of(
        back.pool(),
        "SELECT due_at FROM axa_schedule WHERE name = 'retired'",
    )
    .await;
    // Its first time does not reset: the row keeps its place, and the time missed
    // while archived is skipped, as its last job is still queued.
    assert!(due_after > due_before);
    let last: Uuid = sqlx::query_scalar("SELECT last_job FROM axa_schedule WHERE name = 'retired'")
        .fetch_one(back.pool())
        .await
        .unwrap();
    assert_eq!(last, leftover);
}

// A person's acts.

/// A failed job, from a handler that always fails.
async fn failed_job(queue: &mut Queue) -> Uuid {
    queue.handle("Fails", |_| async { Err::<(), _>("down") });
    let id = queue
        .enqueue("Fails", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(queue.run_one().await.unwrap());
    assert_eq!(state(queue, id).await, JobState::Failed);
    id
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn each_act_is_kept_with_who_and_why_and_a_refused_one_is_not() {
    let mut queue = fresh(quick()).await;
    let queued = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    let failed = failed_job(&mut queue).await;
    let review = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    sqlx::query("UPDATE axa_queue SET status = 'needs_review' WHERE id = ?")
        .bind(review)
        .execute(queue.pool())
        .await
        .unwrap();

    assert!(cancel(&queue, queued).await);
    assert!(retry(&queue, failed).await);
    assert!(retry(&queue, review).await);
    // Acts that do not apply change nothing and are not kept.
    assert!(!cancel(&queue, queued).await, "already cancelled");
    assert!(!retry(&queue, queued).await, "cancelled is final");
    assert!(!retry(&queue, failed).await, "it is queued, not failed");
    assert!(!cancel(&queue, Uuid::new_v4()).await, "no such job");
    assert_eq!(
        count(queue.pool(), "SELECT count(*) FROM axa_job_acts").await,
        3
    );

    for (id, kind, name, attempts) in [
        (queued, ActKind::Cancelled, "Nobody", 0),
        (failed, ActKind::Retried, "Fails", 1),
        (review, ActKind::Released, "Nobody", 0),
    ] {
        let acts = queue.acts(id).await.unwrap();
        assert_eq!(acts.len(), 1, "{kind:?}");
        let act = &acts[0];
        assert_eq!(act.job_id, id);
        assert_eq!(act.kind, kind);
        assert_eq!(act.job_name, name);
        assert_eq!(act.attempts, attempts);
        assert_eq!(act.by, "ops:alice");
        assert!(act.reason.starts_with("test_"));
    }
    assert!(queue.acts(Uuid::new_v4()).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn an_act_commits_with_its_change_and_the_callers_audit() {
    let queue = fresh(quick()).await;
    sqlx::query("CREATE TABLE IF NOT EXISTS service_audit (job BINARY(16) NOT NULL)")
        .execute(queue.pool())
        .await
        .unwrap();
    let id = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    let audits = async |id: Uuid| -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM service_audit WHERE job = ?")
            .bind(id)
            .fetch_one(queue.pool())
            .await
            .unwrap()
    };

    // Rolled back: the job, its act, and the service's audit row all go.
    let mut transaction = queue.pool().begin().await.unwrap();
    assert!(
        axa_mysql_job_queue::cancel(&mut transaction, id, "ops:alice", "duplicate")
            .await
            .unwrap()
    );
    sqlx::query("INSERT INTO service_audit (job) VALUES (?)")
        .bind(id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert_eq!(state(&queue, id).await, JobState::Queued);
    assert!(queue.acts(id).await.unwrap().is_empty());
    assert_eq!(audits(id).await, 0);

    // Committed: all three stand.
    let mut transaction = queue.pool().begin().await.unwrap();
    assert!(
        axa_mysql_job_queue::cancel(&mut transaction, id, "ops:alice", "duplicate")
            .await
            .unwrap()
    );
    sqlx::query("INSERT INTO service_audit (job) VALUES (?)")
        .bind(id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(state(&queue, id).await, JobState::Cancelled);
    assert_eq!(queue.acts(id).await.unwrap().len(), 1);
    assert_eq!(audits(id).await, 1);
    sqlx::query("DROP TABLE service_audit")
        .execute(queue.pool())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn an_act_needs_a_short_code_for_who_and_why() {
    let queue = fresh(quick()).await;
    let id = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    let long = "x".repeat(201);
    for (by, reason, what) in [
        ("", "duplicate", "by"),
        ("ops:alice", "", "reason"),
        ("ops:alice", long.as_str(), "reason"),
        ("ops\nalice", "duplicate", "by"),
        ("ops:alice", "two\nlines", "reason"),
    ] {
        let mut transaction = queue.pool().begin().await.unwrap();
        let error = axa_mysql_job_queue::cancel(&mut transaction, id, by, reason)
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::NotACode { what: w } if w == what),
            "{error}"
        );
        let error = axa_mysql_job_queue::retry(&mut transaction, id, by, reason)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::NotACode { .. }), "{error}");
        transaction.commit().await.unwrap();
    }
    assert_eq!(state(&queue, id).await, JobState::Queued);
    assert!(queue.acts(id).await.unwrap().is_empty());
    // The table refuses one written by hand too.
    let by_hand = sqlx::query(
        "INSERT INTO axa_job_acts (job_id, job_name, act, attempts, `by`, reason, `at`)
         VALUES (?, 'Nobody', 'cancelled', 0, 'ops:alice', ?, UTC_TIMESTAMP(6))",
    )
    .bind(id)
    .bind("two\nlines")
    .execute(queue.pool())
    .await;
    assert!(by_hand.is_err());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn the_acts_table_refuses_every_change() {
    let queue = fresh(quick()).await;
    let id = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(cancel(&queue, id).await);
    // MySQL has no trigger for `TRUNCATE`, which a role without `DROP` cannot send.
    for change in [
        "UPDATE axa_job_acts SET reason = 'changed'",
        "DELETE FROM axa_job_acts",
    ] {
        let error = sqlx::query(change)
            .execute(queue.pool())
            .await
            .expect_err(change)
            .to_string();
        assert!(error.contains("refused"), "{change}: {error}");
    }
    let acts = queue.acts(id).await.unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0].reason, "test_cancel");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn acts_are_kept_when_their_job_is_swept() {
    let queue = fresh(quick()).await;
    let id = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(cancel(&queue, id).await);
    sqlx::query("UPDATE axa_queue SET updated_at = UTC_TIMESTAMP(6) - INTERVAL 30 DAY")
        .execute(queue.pool())
        .await
        .unwrap();
    assert_eq!(queue.purge_finished().await.unwrap(), 1);
    assert!(queue.job(id).await.unwrap().is_none());
    let acts = queue.acts(id).await.unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(
        (acts[0].kind, acts[0].job_name.as_str()),
        (ActKind::Cancelled, "Nobody")
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_missing_acts_trigger_stops_the_queue() {
    let queue = fresh(quick()).await;
    let setup = async || {
        for statement in SETUP_STATEMENTS {
            sqlx::raw_sql(statement)
                .execute(queue.pool())
                .await
                .unwrap();
        }
    };
    assert!(Queue::connect_existing(&uri(), quick()).await.is_ok());
    for change in [
        "DROP TRIGGER axa_job_acts_refuse_update",
        "DROP TRIGGER axa_job_acts_refuse_delete",
        "DROP TABLE axa_job_acts",
    ] {
        sqlx::raw_sql(change).execute(queue.pool()).await.unwrap();
        let error = Queue::connect_existing(&uri(), quick())
            .await
            .expect_err(change)
            .to_string();
        assert!(error.contains("axa_job_acts"), "{change}: {error}");
        setup().await;
        assert!(Queue::connect_existing(&uri(), quick()).await.is_ok());
    }
    // `connect` makes them.
    sqlx::query("DROP TABLE axa_job_acts")
        .execute(queue.pool())
        .await
        .unwrap();
    assert!(Queue::connect(&uri(), quick()).await.is_ok());
    assert!(Queue::connect_existing(&uri(), quick()).await.is_ok());
}

/// A tenant and the arguments, as a handler that takes a tenant saw them.
type Seen = (Option<String>, serde_json::Value);

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_job_carries_its_tenant_to_a_handler_that_takes_one() {
    let mut queue = fresh(quick()).await;
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let record = Arc::clone(&seen);
    queue.handle_with_tenant("Tenanted", move |tenant, args| {
        record.lock().unwrap().push((tenant, args));
        async { Ok::<(), String>(()) }
    });
    queue.handle("Plain", |_args| async { Ok::<(), String>(()) });

    let job = NewJob::new("Tenanted", serde_json::json!({ "id": 7 })).tenant("acme");
    assert_eq!(job.tenant_id(), Some("acme"));
    let acme = queue.enqueue_job(&job).await.unwrap();
    let none = queue
        .enqueue("Tenanted", serde_json::json!({ "id": 8 }))
        .await
        .unwrap();
    let plain = queue
        .enqueue_job(&NewJob::new("Plain", serde_json::Value::Null).tenant("acme"))
        .await
        .unwrap();
    assert_eq!(
        queue.job(acme).await.unwrap().unwrap().tenant.as_deref(),
        Some("acme")
    );
    assert_eq!(queue.job(none).await.unwrap().unwrap().tenant, None);

    while queue.run_one().await.unwrap() {}
    let mut seen = seen.lock().unwrap().clone();
    seen.sort_by_key(|(_, args)| args["id"].as_i64());
    assert_eq!(
        seen,
        vec![
            (Some("acme".to_string()), serde_json::json!({ "id": 7 })),
            (None, serde_json::json!({ "id": 8 })),
        ]
    );
    // A handler that takes no tenant never runs a tenant's job.
    let plain = queue.job(plain).await.unwrap().unwrap();
    assert_eq!(plain.state, JobState::Failed);
    assert!(
        plain.error.unwrap().contains("handle_with_tenant"),
        "the failure says what to do"
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_tenant_that_is_not_a_tenant_id_is_refused() {
    let queue = fresh(quick()).await;
    for bad in [
        "",
        "Acme",
        "-acme",
        "acme-",
        "acme corp",
        "acme'--",
        &"a".repeat(64),
    ] {
        let job = NewJob::new("Tenanted", serde_json::Value::Null).tenant(bad);
        assert!(
            matches!(queue.enqueue_job(&job).await, Err(Error::NotATenant)),
            "{bad:?}"
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM axa_queue")
        .fetch_one(queue.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "nothing was written");
    // The table holds to the same rule, whatever writes to it.
    let direct = sqlx::query(
        "INSERT INTO axa_queue (id, name, args, tenant, run_at, created_at, updated_at)
         VALUES (?, 'x', 'null', 'Acme', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .bind(Uuid::new_v4())
    .execute(queue.pool())
    .await;
    assert!(direct.is_err());
}

// Unique keys.

async fn set_status(queue: &Queue, id: Uuid, status: &str) {
    sqlx::query("UPDATE axa_queue SET status = ? WHERE id = ?")
        .bind(status)
        .bind(id)
        .execute(queue.pool())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_job_with_a_key_is_queued_once_while_it_is_queued_or_running() {
    let queue = fresh(quick()).await;
    let job = |id: i64| NewJob::new("Sync", serde_json::json!({ "id": id })).unique("sync:42");
    assert_eq!(job(1).unique_key(), Some("sync:42"));
    let first = queue.enqueue_job(&job(1)).await.unwrap();
    assert_eq!(queue.enqueue_job(&job(2)).await.unwrap(), first);
    set_status(&queue, first, "running").await;
    assert_eq!(queue.enqueue_job(&job(3)).await.unwrap(), first);
    assert_eq!(
        queue
            .job(first)
            .await
            .unwrap()
            .unwrap()
            .unique_key
            .as_deref(),
        Some("sync:42")
    );
    let args: serde_json::Value = sqlx::query_scalar("SELECT args FROM axa_queue WHERE id = ?")
        .bind(first)
        .fetch_one(queue.pool())
        .await
        .unwrap();
    assert_eq!(
        args,
        serde_json::json!({ "id": 1 }),
        "the first job's arguments stand"
    );

    // Another tenant, another name, or another key is another job, and so is one
    // with no key.
    let others = [
        job(4).tenant("acme"),
        NewJob::new("Other", serde_json::Value::Null).unique("sync:42"),
        NewJob::new("Sync", serde_json::Value::Null).unique("sync:43"),
        NewJob::new("Sync", serde_json::Value::Null),
        NewJob::new("Sync", serde_json::Value::Null),
    ];
    let mut ids = HashSet::from([first]);
    for other in &others {
        assert!(
            ids.insert(queue.enqueue_job(other).await.unwrap()),
            "{other:?}"
        );
    }
    assert_eq!(queue.enqueue_job(&job(5).tenant("acme")).await.unwrap(), {
        let acme: Uuid = sqlx::query_scalar("SELECT id FROM axa_queue WHERE tenant = 'acme'")
            .fetch_one(queue.pool())
            .await
            .unwrap();
        acme
    });

    // Once the job is done in any way, the key is free again.
    let mut last = first;
    for status in ["completed", "failed", "cancelled", "needs_review"] {
        set_status(&queue, last, status).await;
        let next = queue.enqueue_job(&job(6)).await.unwrap();
        assert_ne!(next, last, "free after {status}");
        last = next;
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_key_holds_against_a_job_in_a_transaction_not_yet_committed() {
    let queue = Arc::new(fresh(quick()).await);
    let job = || NewJob::new("Sync", serde_json::Value::Null).unique("sync:7");
    for commit in [true, false] {
        sqlx::query("DELETE FROM axa_queue")
            .execute(queue.pool())
            .await
            .unwrap();
        let mut transaction = queue.pool().begin().await.unwrap();
        let held = axa_mysql_job_queue::enqueue_on(&mut transaction, &job())
            .await
            .unwrap();
        let other = {
            let queue = Arc::clone(&queue);
            tokio::spawn(async move { queue.enqueue_job(&job()).await.unwrap() })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !other.is_finished(),
            "the second enqueue waits for the first"
        );
        if commit {
            transaction.commit().await.unwrap();
            assert_eq!(other.await.unwrap(), held, "and then finds its job");
        } else {
            transaction.rollback().await.unwrap();
            assert_ne!(other.await.unwrap(), held, "or queues its own");
        }
        assert_eq!(
            count(queue.pool(), "SELECT count(*) FROM axa_queue").await,
            1
        );
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_failed_job_is_not_retried_while_another_of_its_key_is_queued() {
    let queue = fresh(quick()).await;
    let job = || NewJob::new("Sync", serde_json::Value::Null).unique("sync:9");
    let failed = queue.enqueue_job(&job()).await.unwrap();
    set_status(&queue, failed, "failed").await;
    let queued = queue.enqueue_job(&job()).await.unwrap();
    assert!(!retry(&queue, failed).await, "another of its key is queued");
    assert_eq!(state(&queue, failed).await, JobState::Failed);
    assert!(
        queue.acts(failed).await.unwrap().is_empty(),
        "and no act is kept"
    );
    set_status(&queue, queued, "completed").await;
    assert!(
        retry(&queue, failed).await,
        "and once it is done, it may be"
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_key_that_is_not_a_code_is_refused() {
    let queue = fresh(quick()).await;
    for bad in [String::new(), "a\nb".to_string(), "k".repeat(201)] {
        let job = NewJob::new("Sync", serde_json::Value::Null).unique(bad.clone());
        assert!(
            matches!(
                queue.enqueue_job(&job).await,
                Err(Error::NotACode { what: "unique key" })
            ),
            "{bad:?}"
        );
    }
    assert_eq!(
        count(queue.pool(), "SELECT count(*) FROM axa_queue").await,
        0
    );
    let direct = sqlx::query(
        "INSERT INTO axa_queue (id, name, args, unique_key, run_at, created_at, updated_at)
        VALUES (?, 'x', 'null', '', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .bind(Uuid::new_v4())
    .execute(queue.pool())
    .await;
    assert!(direct.is_err(), "the table refuses it too");
    // And a queue without the key's index is stopped at boot.
    sqlx::query("DROP INDEX axa_queue_unique ON axa_queue")
        .execute(queue.pool())
        .await
        .unwrap();
    assert!(Queue::connect_existing(&uri(), quick()).await.is_err());
}

// Cron schedules.

fn cron(queue: &mut Queue, name: &str, pattern: &str) {
    queue
        .schedule(
            Schedule::cron(
                name,
                pattern,
                NewJob::new("Tick", serde_json::json!({ "from": name })),
            )
            .unwrap(),
        )
        .unwrap();
}

async fn set_due(pool: &MySqlPool, name: &str, seconds_ago: u32) {
    sqlx::query(
        "UPDATE axa_schedule SET due_at = UTC_TIMESTAMP(6) - INTERVAL ? SECOND WHERE name = ?",
    )
    .bind(seconds_ago)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

/// The schedule's due time, and the database's now.
async fn due_and_now(pool: &MySqlPool, name: &str) -> (Timestamp, Timestamp) {
    let row = sqlx::query(
        "SELECT TIMESTAMPDIFF(MICROSECOND, '1970-01-01 00:00:00', due_at) AS due_at,
            TIMESTAMPDIFF(MICROSECOND, '1970-01-01 00:00:00', UTC_TIMESTAMP(6)) AS now
        FROM axa_schedule WHERE name = ?",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap();
    let time = |column| Timestamp::from_microsecond(row.get(column)).unwrap();
    (time("due_at"), time("now"))
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_cron_schedule_first_runs_at_its_patterns_next_time() {
    let mut queue = fresh(quick()).await;
    cron(&mut queue, "at-two", "0 2 * * *");
    assert_eq!(queue.tick_schedules().await.unwrap(), 0, "not due at boot");
    let (due, now) = due_and_now(queue.pool(), "at-two").await;
    assert!(due > now && due.duration_since(now) <= SignedDuration::from_hours(24));
    let due = TimeZone::UTC.to_datetime(due);
    assert_eq!(
        (due.hour(), due.minute(), due.second()),
        (2, 0, 0),
        "in UTC"
    );
    let info = queue.schedules().await.unwrap();
    assert_eq!(info[0].cron.as_deref(), Some("0 2 * * *"));
    assert_eq!(info[0].interval, None);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_cron_schedule_queues_once_for_missed_times_and_skips_while_busy() {
    let mut queue = fresh(quick()).await;
    cron(&mut queue, "secondly", "* * * * * *");
    queue.tick_schedules().await.unwrap();
    // That tick makes the row, and queues a job too if a second has passed since, which
    // would leave the schedule busy below. Start with none.
    sqlx::query("DELETE FROM axa_queue")
        .execute(queue.pool())
        .await
        .unwrap();
    // Every process was down for a week.
    set_due(queue.pool(), "secondly", 7 * 86_400).await;
    assert_eq!(
        queue.tick_schedules().await.unwrap(),
        1,
        "one job for them all"
    );
    let (due, now) = due_and_now(queue.pool(), "secondly").await;
    assert!(
        due > now - SignedDuration::from_secs(1),
        "the next is the pattern's after now"
    );
    assert_eq!(due.subsec_microsecond(), 0, "on the pattern's whole second");
    // Due again while its job is still queued: skipped, and moved on.
    set_due(queue.pool(), "secondly", 1).await;
    assert_eq!(queue.tick_schedules().await.unwrap(), 0);
    assert_eq!(jobs_named(queue.pool(), "Tick").await, 1);
    let (due, now) = due_and_now(queue.pool(), "secondly").await;
    assert!(due > now - SignedDuration::from_secs(1));
    // Once it finished, the next due time queues again.
    sqlx::query("UPDATE axa_queue SET status = 'completed'")
        .execute(queue.pool())
        .await
        .unwrap();
    set_due(queue.pool(), "secondly", 1).await;
    assert_eq!(queue.tick_schedules().await.unwrap(), 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_changed_pattern_takes_its_next_time_from_the_new_one() {
    let hour = |due: Timestamp| TimeZone::UTC.to_datetime(due).hour();
    let mut queue = fresh(quick()).await;
    every(&mut queue, "nightly", Duration::from_hours(24));
    queue.tick_schedules().await.unwrap();
    let mut queue = replica(quick()).await;
    cron(&mut queue, "nightly", "0 2 * * *");
    queue.tick_schedules().await.unwrap();
    let (due, _) = due_and_now(queue.pool(), "nightly").await;
    assert_eq!(hour(due), 2, "from an interval to a pattern");
    let mut queue = replica(quick()).await;
    cron(&mut queue, "nightly", "0 3 * * *");
    queue.tick_schedules().await.unwrap();
    let (due, _) = due_and_now(queue.pool(), "nightly").await;
    assert_eq!(hour(due), 3, "from one pattern to another");
    let info = queue.schedules().await.unwrap();
    assert_eq!(info[0].cron.as_deref(), Some("0 3 * * *"));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn replicas_ticking_a_cron_schedule_queue_each_due_time_once() {
    let mut replicas = Vec::new();
    replicas.push(fresh(quick()).await);
    for _ in 0..5 {
        replicas.push(replica(quick()).await);
    }
    for queue in &mut replicas {
        cron(queue, "race", "* * * * * *");
    }
    let pool = replicas[0].pool().clone();
    let replicas: Vec<_> = replicas.into_iter().map(Arc::new).collect();
    let ticks: Vec<_> = replicas
        .iter()
        .map(|queue| {
            let queue = queue.clone();
            tokio::spawn(async move {
                let end = tokio::time::Instant::now() + Duration::from_millis(4_500);
                let mut mine = 0;
                while tokio::time::Instant::now() < end {
                    mine += queue.tick_schedules().await.unwrap();
                    finish_unheld(queue.pool()).await;
                }
                mine
            })
        })
        .collect();
    let mut total = 0;
    for tick in ticks {
        total += tick.await.unwrap();
    }
    let jobs = jobs_named(&pool, "Tick").await;
    assert_eq!(i64::try_from(total).unwrap(), jobs);
    assert!(
        (3..=5).contains(&jobs),
        "one job a second for 4.5 seconds: {jobs}"
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_job_that_asks_for_a_delay_is_tried_again_after_it() {
    let mut queue = fresh(quick()).await;
    queue.handle("Busy", |_| async {
        Err(axa_mysql_job_queue::RetryAfter::new(
            Duration::from_mins(2),
            "the receiver is busy",
        ))
    });
    let job = NewJob::new("Busy", serde_json::Value::Null).max_attempts(Some(3));
    let id = queue.enqueue_job(&job).await.unwrap();
    assert!(queue.run_one().await.unwrap());
    let info = queue.job(id).await.unwrap().unwrap();
    assert_eq!(info.state, JobState::Queued);
    assert_eq!(info.error.as_deref(), Some("the receiver is busy"));
    let wait = info.run_at.duration_since(info.updated_at).as_millis();
    assert!((119_000..=121_000).contains(&wait), "{wait}");
    assert!(!queue.run_one().await.unwrap(), "not before its time");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_job_that_panics_fails_and_its_loop_carries_on() {
    let mut queue = fresh(QueueOptions {
        workers: 1,
        ..quick()
    })
    .await;
    // Its job's arguments are null, so it panics.
    queue.handle("Panics", |args: serde_json::Value| async move {
        assert!(!args.is_null(), "the panic's own text");
        Ok::<(), String>(())
    });
    let gauge = Gauge::default();
    queue.handle("Fine", gauge.handler(0));
    assert_eq!(queue.handled(), HashSet::from(["Panics", "Fine"]));
    queue.ping().await.unwrap();
    let panics = queue
        .enqueue_limited("Panics", serde_json::Value::Null, Some(1))
        .await
        .unwrap();
    assert_eq!(
        queue.job(panics).await.unwrap().unwrap().max_attempts,
        Some(1)
    );
    queue
        .enqueue("Fine", serde_json::Value::Null)
        .await
        .unwrap();
    let (queue, stop, run) = start(queue);
    until(async || gauge.done() == 1 && state(&queue, panics).await == JobState::Failed).await;
    let failed = queue.job(panics).await.unwrap().unwrap();
    assert_eq!(failed.attempts, 1, "its own limit of one try");
    let error = failed.error.unwrap();
    assert!(error.contains("the job panicked"), "{error}");
    assert!(!error.contains("own text"), "{error}");
    stop.send(true).unwrap();
    run.await.unwrap();
}
