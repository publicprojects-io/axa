//! The queue against a real Postgres. The tests share three tables, so run them with
//! `--test-threads=1`. `axa_job_acts` refuses `DELETE` and `TRUNCATE`, so a test
//! starts by dropping it.
//!
//! ```sh
//! AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-postgres-job-queue -- --ignored --test-threads=1
//! ```

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axa_postgres_job_queue::{
    ActKind, Error, JobState, NewJob, Queue, QueueOptions, SETUP_STATEMENTS, Schedule,
    enqueue_all_on,
};
use sqlx::{PgPool, Row};
use tokio::sync::watch;
use uuid::Uuid;

fn uri() -> String {
    std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL")
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
    let admin = PgPool::connect(&uri()).await.expect("connect");
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

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Cancel `id` as a person would, in a transaction of its own.
async fn cancel(queue: &Queue, id: Uuid) -> bool {
    let mut transaction = queue.pool().begin().await.unwrap();
    let cancelled =
        axa_postgres_job_queue::cancel(&mut transaction, id, "ops:alice", "test_cancel")
            .await
            .unwrap();
    transaction.commit().await.unwrap();
    cancelled
}

/// Retry `id` as a person would, in a transaction of its own.
async fn retry(queue: &Queue, id: Uuid) -> bool {
    let mut transaction = queue.pool().begin().await.unwrap();
    let retried = axa_postgres_job_queue::retry(&mut transaction, id, "ops:alice", "test_retry")
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
    let past = chrono_past(&queue).await;
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

async fn chrono_past(
    queue: &Queue,
) -> axa_postgres_job_queue::DateTime<axa_postgres_job_queue::Utc> {
    sqlx::query_scalar("SELECT now() - interval '1 minute'")
        .fetch_one(queue.pool())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
        "UPDATE axa_queue SET status = 'failed', error = 'from the lost run', updated_at = now()
         WHERE id = $1 AND attempts = 1 AND status = 'running'",
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn connecting_to_tables_without_the_new_columns_fails_and_changes_nothing() {
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
            "SELECT count(*) FROM information_schema.tables WHERE table_name = 'axa_schedule'"
        )
        .await,
        0
    );
    for statement in SETUP_STATEMENTS {
        sqlx::query(statement).execute(queue.pool()).await.unwrap();
    }
    assert!(Queue::connect_existing(&uri(), quick()).await.is_ok());
}

// Schedules.

fn every(queue: &mut Queue, name: &str, interval: Duration) {
    queue.schedule(Schedule::every(
        name,
        interval,
        NewJob::new("Tick", serde_json::json!({ "from": name })),
    ));
}

async fn jobs_named(pool: &PgPool, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM axa_queue WHERE name = $1")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Seconds between the row's due time and `from`, and between now and the due
/// time, on the database's clock.
async fn due(pool: &PgPool, name: &str) -> (f64, f64) {
    let row = sqlx::query(
        "SELECT EXTRACT(EPOCH FROM due_at - created_at)::float8 AS since,
                EXTRACT(EPOCH FROM due_at - now())::float8 AS ahead
         FROM axa_schedule WHERE name = $1",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap();
    (row.get("since"), row.get("ahead"))
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
                    sqlx::query(
                        "UPDATE axa_queue SET status = 'completed' WHERE id IN (
                             SELECT id FROM axa_queue WHERE status <> 'completed'
                             ORDER BY id FOR UPDATE SKIP LOCKED)",
                    )
                    .execute(queue.pool())
                    .await
                    .unwrap();
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
        "UPDATE axa_schedule SET created_at = created_at - interval '95 seconds',
                due_at = due_at - interval '95 seconds'",
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_due_time_is_skipped_while_the_last_job_has_not_finished() {
    let mut queue = fresh(quick()).await;
    every(&mut queue, "busy", Duration::from_secs(1));
    assert_eq!(queue.tick_schedules().await.unwrap(), 1);
    for state in ["queued", "running"] {
        sqlx::query("UPDATE axa_queue SET status = $1")
            .bind(state)
            .execute(queue.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE axa_schedule SET due_at = now() - interval '1 second'")
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
        sqlx::query("UPDATE axa_queue SET status = $1")
            .bind(state)
            .execute(queue.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE axa_schedule SET due_at = now() - interval '1 second'")
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
        "SELECT EXTRACT(EPOCH FROM q.created_at - s.created_at)::float8
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_first_time_applies_only_when_the_row_is_made() {
    let mut queue = fresh(quick()).await;
    let first: axa_postgres_job_queue::DateTime<axa_postgres_job_queue::Utc> =
        sqlx::query_scalar("SELECT date_trunc('second', now()) + interval '1 hour'")
            .fetch_one(queue.pool())
            .await
            .unwrap();
    let job = NewJob::new("Tick", serde_json::Value::Null);
    queue.schedule(
        Schedule::every("nightly", Duration::from_hours(24), job.clone()).first_at(first),
    );
    assert_eq!(
        queue.tick_schedules().await.unwrap(),
        0,
        "not due until its first time"
    );
    let stored: axa_postgres_job_queue::DateTime<axa_postgres_job_queue::Utc> =
        sqlx::query_scalar("SELECT due_at FROM axa_schedule WHERE name = 'nightly'")
            .fetch_one(queue.pool())
            .await
            .unwrap();
    assert_eq!(stored, first);

    // A later release with another first time leaves the row where it is.
    let earlier: axa_postgres_job_queue::DateTime<axa_postgres_job_queue::Utc> =
        sqlx::query_scalar("SELECT now() - interval '5 hours'")
            .fetch_one(queue.pool())
            .await
            .unwrap();
    let mut later = replica(quick()).await;
    later.schedule(Schedule::every("nightly", Duration::from_hours(24), job).first_at(earlier));
    later.tick_schedules().await.unwrap();
    let stored: axa_postgres_job_queue::DateTime<axa_postgres_job_queue::Utc> =
        sqlx::query_scalar("SELECT due_at FROM axa_schedule WHERE name = 'nightly'")
            .fetch_one(queue.pool())
            .await
            .unwrap();
    assert_eq!(stored, first);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
    sqlx::query("UPDATE axa_queue SET attempts = attempts + 1, updated_at = now()")
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
    sqlx::query("UPDATE axa_queue SET updated_at = now() - interval '2 hours'")
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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

async fn archived(pool: &PgPool, name: &str) -> Option<bool> {
    sqlx::query_scalar("SELECT archived_at IS NOT NULL FROM axa_schedule WHERE name = $1")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
    sqlx::query("UPDATE axa_schedule SET seen_at = now() - interval '2 hours'")
        .execute(new.pool())
        .await
        .unwrap();
    new.tick_schedules().await.unwrap(); // `kept` is seen again
    assert_eq!(new.archive_schedules().await.unwrap(), 1);
    assert_eq!(new.archive_schedules().await.unwrap(), 0);
    assert_eq!(archived(new.pool(), "retired").await, Some(true));
    assert_eq!(archived(new.pool(), "kept").await, Some(false));

    // Archived, it queues nothing, its row stays, and its last job still runs.
    sqlx::query("UPDATE axa_schedule SET due_at = now() - interval '1 minute'")
        .execute(new.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE axa_queue SET status = 'completed' WHERE id <> $1")
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
    let due_before: axa_postgres_job_queue::DateTime<axa_postgres_job_queue::Utc> =
        sqlx::query_scalar("SELECT due_at FROM axa_schedule WHERE name = 'retired'")
            .fetch_one(new.pool())
            .await
            .unwrap();
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
    let due_after: axa_postgres_job_queue::DateTime<axa_postgres_job_queue::Utc> =
        sqlx::query_scalar("SELECT due_at FROM axa_schedule WHERE name = 'retired'")
            .fetch_one(back.pool())
            .await
            .unwrap();
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
    sqlx::query("UPDATE axa_queue SET status = 'needs_review' WHERE id = $1")
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_act_commits_with_its_change_and_the_callers_audit() {
    let queue = fresh(quick()).await;
    sqlx::query("CREATE TABLE IF NOT EXISTS service_audit (job UUID NOT NULL)")
        .execute(queue.pool())
        .await
        .unwrap();
    let id = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    let audits = async |id: Uuid| -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM service_audit WHERE job = $1")
            .bind(id)
            .fetch_one(queue.pool())
            .await
            .unwrap()
    };

    // Rolled back: the job, its act, and the service's audit row all go.
    let mut transaction = queue.pool().begin().await.unwrap();
    assert!(
        axa_postgres_job_queue::cancel(&mut transaction, id, "ops:alice", "duplicate")
            .await
            .unwrap()
    );
    sqlx::query("INSERT INTO service_audit (job) VALUES ($1)")
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
        axa_postgres_job_queue::cancel(&mut transaction, id, "ops:alice", "duplicate")
            .await
            .unwrap()
    );
    sqlx::query("INSERT INTO service_audit (job) VALUES ($1)")
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
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
        let error = axa_postgres_job_queue::cancel(&mut transaction, id, by, reason)
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::NotACode { what: w } if w == what),
            "{error}"
        );
        let error = axa_postgres_job_queue::retry(&mut transaction, id, by, reason)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::NotACode { .. }), "{error}");
        transaction.commit().await.unwrap();
    }
    assert_eq!(state(&queue, id).await, JobState::Queued);
    assert!(queue.acts(id).await.unwrap().is_empty());
    // The table refuses one written by hand too.
    let by_hand = sqlx::query(
        "INSERT INTO axa_job_acts (job_id, job_name, act, attempts, by, reason)
         VALUES ($1, 'Nobody', 'cancelled', 0, 'ops:alice', $2)",
    )
    .bind(id)
    .bind("two\nlines")
    .execute(queue.pool())
    .await;
    assert!(by_hand.is_err());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_acts_table_refuses_every_change() {
    let queue = fresh(quick()).await;
    let id = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(cancel(&queue, id).await);
    let changes = [
        "UPDATE axa_job_acts SET reason = 'changed'",
        "DELETE FROM axa_job_acts",
        "TRUNCATE axa_job_acts",
    ];
    // Whoever sends them, and with triggers for replication turned off too.
    for role in ["origin", "replica"] {
        let mut connection = queue.pool().acquire().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SET session_replication_role = {role}"
        )))
        .execute(&mut *connection)
        .await
        .unwrap();
        for change in changes {
            let error = sqlx::query(change)
                .execute(&mut *connection)
                .await
                .expect_err(change)
                .to_string();
            assert!(error.contains("refused"), "{change}: {error}");
        }
        sqlx::query("RESET session_replication_role")
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    let acts = queue.acts(id).await.unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0].reason, "test_cancel");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn acts_are_kept_when_their_job_is_swept() {
    let queue = fresh(quick()).await;
    let id = queue
        .enqueue("Nobody", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(cancel(&queue, id).await);
    sqlx::query("UPDATE axa_queue SET updated_at = now() - interval '30 days'")
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
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_missing_or_turned_off_acts_trigger_stops_the_queue() {
    let queue = fresh(quick()).await;
    let setup = async || {
        for statement in SETUP_STATEMENTS {
            sqlx::query(statement).execute(queue.pool()).await.unwrap();
        }
    };
    assert!(Queue::connect_existing(&uri(), quick()).await.is_ok());
    for change in [
        "ALTER TABLE axa_job_acts DISABLE TRIGGER axa_job_acts_refuse",
        // On, but not always: `session_replication_role` could turn it off.
        "ALTER TABLE axa_job_acts ENABLE TRIGGER axa_job_acts_refuse_truncate",
        "DROP TRIGGER axa_job_acts_refuse ON axa_job_acts",
        "DROP TABLE axa_job_acts",
    ] {
        sqlx::query(change).execute(queue.pool()).await.unwrap();
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
