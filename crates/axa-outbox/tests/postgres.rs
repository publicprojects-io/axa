//! The outbox against a real Postgres. These are ignored by default. Run them with
//! `AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-outbox --all-features -- --ignored`.
//!
//! A row's history is the point of most of these, so each one reads the table rather
//! than trusting what the API says. They share the table, and each starts by emptying it.

use std::{
    collections::{HashMap, HashSet},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axa_outbox::{Claimed, Event, Failure, Outbox, OutboxOptions, SETUP_STATEMENTS, Stats, write};
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tokio::sync::watch;
use uuid::Uuid;

/// Held for a whole test: these tests share one table.
static TABLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Being set makes a process the relay that gets killed.
const ROLE: &str = "AXA_OUTBOX_TEST_ROLE";

/// How long a test waits for anything, after which it fails.
const DEADLINE: Duration = Duration::from_secs(20);

fn uri() -> String {
    std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL")
}

/// Quick to poll, and retrying at once.
fn quick() -> OutboxOptions {
    OutboxOptions {
        poll_interval: Duration::from_millis(10),
        retry_backoff: Duration::ZERO,
        ..OutboxOptions::default()
    }
}

async fn admin() -> PgPool {
    PgPoolOptions::new().connect(&uri()).await.unwrap()
}

/// An outbox on an empty table, and a pool for looking at it.
async fn fresh(options: OutboxOptions) -> (Outbox, PgPool) {
    let outbox = Outbox::connect(&uri(), options).await.unwrap();
    let pool = admin().await;
    sqlx::query("DELETE FROM axa_outbox")
        .execute(&pool)
        .await
        .unwrap();
    (outbox, pool)
}

/// Write an event in its own committed transaction, returning it.
async fn add(pool: &PgPool, key: &str) -> Event {
    let event = Event::new("person", "updated", key, json!({ "key": key }));
    let mut transaction = pool.begin().await.unwrap();
    write(&mut transaction, &event).await.unwrap();
    transaction.commit().await.unwrap();
    event
}

/// One row's status, attempts, and error.
async fn row(pool: &PgPool, event: &Event) -> (String, i32, Option<String>) {
    let row = sqlx::query("SELECT status, attempts, error FROM axa_outbox WHERE event_id = $1")
        .bind(event.event_id)
        .fetch_one(pool)
        .await
        .unwrap();
    (row.get("status"), row.get("attempts"), row.get("error"))
}

async fn status(pool: &PgPool, event: &Event) -> String {
    row(pool, event).await.0
}

async fn rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM axa_outbox")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// As if every claim had gone quiet an hour ago, which is how `stale_minutes` passes.
async fn go_quiet(pool: &PgPool) {
    sqlx::query(
        "UPDATE axa_outbox SET updated_at = now() - interval '1 hour' WHERE status = 'claimed'",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Wait until nothing pending or claimed is left.
async fn drained(outbox: &Outbox) -> Stats {
    let until = Instant::now() + DEADLINE;
    loop {
        let stats = outbox.stats().await.unwrap();
        if stats.pending == 0 && stats.claimed == 0 {
            return stats;
        }
        assert!(Instant::now() < until, "not drained: {stats:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// What a broker took, in order, shared with a publisher.
#[derive(Clone, Default)]
struct Broker(Arc<Mutex<Vec<Claimed>>>);

impl Broker {
    fn take(&self, event: Claimed) {
        self.0.lock().unwrap().push(event);
    }

    fn event_ids(&self) -> Vec<Uuid> {
        self.0.lock().unwrap().iter().map(|e| e.event_id).collect()
    }

    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    /// How many times each event arrived.
    fn counts(&self) -> HashMap<Uuid, usize> {
        let mut counts = HashMap::new();
        for id in self.event_ids() {
            *counts.entry(id).or_default() += 1;
        }
        counts
    }
}

/// A publisher that always works.
fn taking(
    broker: &Broker,
) -> impl Fn(Claimed) -> std::future::Ready<Result<(), Failure>> + Send + Sync + 'static {
    let broker = broker.clone();
    move |event| {
        broker.take(event);
        std::future::ready(Ok(()))
    }
}

/// A relay running in the background, stopped when asked.
struct Relay {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl Relay {
    fn start<P, F>(outbox: &Outbox, publish: P) -> Self
    where
        P: Fn(Claimed) -> F + Send + Sync + 'static,
        F: Future<Output = Result<(), Failure>> + Send + 'static,
    {
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(outbox.clone().run(publish, stopped));
        Self { stop, task }
    }

    async fn stop(self) {
        self.stop.send(true).unwrap();
        tokio::time::timeout(DEADLINE, self.task)
            .await
            .expect("the relay did not stop")
            .unwrap();
    }
}

// OBX-1: an event commits or rolls back with the change.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_event_rolls_back_with_its_transaction() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    let mut transaction = pool.begin().await.unwrap();
    write(
        &mut transaction,
        &Event::new("person", "updated", "1", json!({})),
    )
    .await
    .unwrap();
    assert_eq!(rows(&pool).await, 0, "seen before the commit");
    transaction.rollback().await.unwrap();
    assert_eq!(rows(&pool).await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_event_commits_with_its_transaction() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    assert_eq!(status(&pool, &event).await, "pending");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_repeated_event_id_fails_the_write() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let mut transaction = pool.begin().await.unwrap();
    assert!(write(&mut transaction, &event).await.is_err());
}

#[cfg(feature = "sea-orm")]
#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_seaorm_transaction_carries_an_event_too() {
    use sea_orm::{Database, TransactionTrait};
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let db = Database::connect(uri()).await.unwrap();

    let dropped = Event::new("person", "updated", "1", json!({ "name": "gone" }));
    let transaction = db.begin().await.unwrap();
    axa_outbox::write_seaorm(&transaction, &dropped)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert_eq!(rows(&pool).await, 0);

    let kept = Event::new("person", "updated", "1", json!({ "name": "Ada" }));
    let transaction = db.begin().await.unwrap();
    axa_outbox::write_seaorm(&transaction, &kept).await.unwrap();
    transaction.commit().await.unwrap();

    let batch = outbox.claim().await.unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].event_id, kept.event_id);
    assert_eq!(batch[0].payload, json!({ "name": "Ada" }));
    assert_eq!(batch[0].key, "1");
}

// OBX-2 and OBX-3: nothing committed is lost, and sent means the broker has it.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_event_committed_before_a_crash_is_still_published() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    // The commit happened, and no relay ever heard of it. A later one finds the row.
    let event = add(&pool, "1").await;
    let outbox = Outbox::connect_existing(&uri(), quick()).await.unwrap();
    let broker = Broker::default();
    let relay = Relay::start(&outbox, taking(&broker));
    drained(&outbox).await;
    relay.stop().await;
    assert_eq!(broker.event_ids(), [event.event_id]);
    assert_eq!(status(&pool, &event).await, "sent");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_failed_publish_is_not_marked_sent() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 5,
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let relay = Relay::start(&outbox, |_| async {
        Err(Failure::rejected("the broker said no"))
    });
    let until = Instant::now() + DEADLINE;
    while row(&pool, &event).await.1 < 2 {
        assert!(Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    relay.stop().await;
    let (status, _, error) = row(&pool, &event).await;
    assert_ne!(status, "sent");
    assert_eq!(error.as_deref(), Some("the broker said no"));
}

/// The child's part: publish to a table, as a broker that outlives the relay, and hang
/// on the third event after the broker has it, as a crash before the ack does.
async fn relay_until_killed() {
    let pool = admin().await;
    let outbox = Outbox::connect_existing(
        &uri(),
        OutboxOptions {
            batch_size: 10,
            ..quick()
        },
    )
    .await
    .unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let (_stop, stopped) = watch::channel(false);
    outbox
        .run(
            move |event| {
                let (pool, seen) = (pool.clone(), seen.clone());
                async move {
                    sqlx::query("INSERT INTO axa_outbox_test_broker (event_id) VALUES ($1)")
                        .bind(event.event_id)
                        .execute(&pool)
                        .await
                        .map_err(Failure::unavailable)?;
                    if seen.fetch_add(1, Ordering::SeqCst) == 2 {
                        std::future::pending::<()>().await;
                    }
                    Ok(())
                }
            },
            stopped,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_relay_killed_mid_batch_loses_nothing() {
    const TEST: &str = "a_relay_killed_mid_batch_loses_nothing";
    if std::env::var_os(ROLE).is_some() {
        relay_until_killed().await;
        return;
    }
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    sqlx::query("DROP TABLE IF EXISTS axa_outbox_test_broker")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE axa_outbox_test_broker (event_id UUID NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    let mut events = Vec::new();
    for number in 0..20 {
        events.push(add(&pool, &number.to_string()).await);
    }
    let broker = async |pool: &PgPool| -> Vec<Uuid> {
        sqlx::query_scalar("SELECT event_id FROM axa_outbox_test_broker")
            .fetch_all(pool)
            .await
            .unwrap()
    };

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([TEST, "--exact", "--nocapture", "--include-ignored"])
        .env(ROLE, "relay")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + DEADLINE;
    while broker(&pool).await.len() < 3 {
        assert!(Instant::now() < until, "the relay published nothing");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // SIGKILL: no unwinding, no release, and no chance to say anything.
    child.kill().unwrap();
    child.wait().unwrap();

    // Two acknowledged, one taken and not acknowledged, seven claimed and never tried,
    // and ten never claimed.
    let mut statuses: HashMap<String, usize> = HashMap::new();
    for event in &events {
        *statuses.entry(status(&pool, event).await).or_default() += 1;
    }
    assert_eq!(statuses["sent"], 2, "{statuses:?}");
    assert_eq!(statuses["claimed"], 8, "{statuses:?}");
    assert_eq!(statuses["pending"], 10, "{statuses:?}");

    // Rows in a live claim are left alone until it goes quiet.
    let replacement = Outbox::connect_existing(&uri(), quick()).await.unwrap();
    assert_eq!(replacement.requeue_stale().await.unwrap(), 0);
    go_quiet(&pool).await;

    let pool_for_publisher = pool.clone();
    let relay = Relay::start(&replacement, move |event| {
        let pool = pool_for_publisher.clone();
        async move {
            sqlx::query("INSERT INTO axa_outbox_test_broker (event_id) VALUES ($1)")
                .bind(event.event_id)
                .execute(&pool)
                .await
                .map_err(Failure::unavailable)?;
            Ok(())
        }
    });
    drained(&replacement).await;
    relay.stop().await;

    let arrived = broker(&pool).await;
    let mut counts: HashMap<Uuid, usize> = HashMap::new();
    for id in &arrived {
        *counts.entry(*id).or_default() += 1;
    }
    for event in &events {
        assert_eq!(status(&pool, event).await, "sent");
        assert!(counts.contains_key(&event.event_id), "lost an event");
    }
    // Exactly the row caught between the broker and the ack came twice.
    assert_eq!(arrived.len(), 21);
    assert_eq!(counts.values().filter(|count| **count == 2).count(), 1);
    sqlx::query("DROP TABLE axa_outbox_test_broker")
        .execute(&pool)
        .await
        .unwrap();
}

// OBX-4: several relays.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn several_relays_publish_each_event_once() {
    let _table = TABLE.lock().await;
    let options = OutboxOptions {
        batch_size: 20,
        ..quick()
    };
    let (outbox, pool) = fresh(options.clone()).await;
    let mut events = HashSet::new();
    let mut transaction = pool.begin().await.unwrap();
    for number in 0..400 {
        let event = Event::new("person", "updated", number.to_string(), json!({}));
        write(&mut transaction, &event).await.unwrap();
        events.insert(event.event_id);
    }
    transaction.commit().await.unwrap();

    let broker = Broker::default();
    let mut relays = Vec::new();
    for _ in 0..8 {
        // Separate connections, as separate processes would have.
        let relay = Outbox::connect_existing(&uri(), options.clone())
            .await
            .unwrap();
        let broker = broker.clone();
        relays.push(Relay::start(&relay, move |event| {
            let broker = broker.clone();
            async move {
                tokio::task::yield_now().await;
                broker.take(event);
                Ok(())
            }
        }));
    }
    drained(&outbox).await;
    for relay in relays {
        relay.stop().await;
    }
    assert_eq!(broker.len(), 400);
    let counts = broker.counts();
    assert_eq!(counts.keys().copied().collect::<HashSet<_>>(), events);
    assert!(counts.values().all(|count| *count == 1), "a repeat");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn two_claims_at_once_share_nothing() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(OutboxOptions {
        batch_size: 5,
        ..quick()
    })
    .await;
    for number in 0..40 {
        add(&pool, &number.to_string()).await;
    }
    let mut claims = Vec::new();
    for _ in 0..8 {
        let outbox = Outbox::connect_existing(
            &uri(),
            OutboxOptions {
                batch_size: 5,
                ..quick()
            },
        )
        .await
        .unwrap();
        claims.push(tokio::spawn(async move { outbox.claim().await.unwrap() }));
    }
    let mut held = Vec::new();
    for claim in claims {
        held.extend(claim.await.unwrap().into_iter().map(|event| event.id));
    }
    let unique: HashSet<_> = held.iter().collect();
    assert_eq!(held.len(), unique.len(), "one row in two claims");
}

// OBX-5: a claim whose relay died comes back.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_claim_not_heard_from_comes_back() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    assert_eq!(outbox.claim().await.unwrap().len(), 1);
    assert_eq!(outbox.claim().await.unwrap().len(), 0, "taken twice");
    go_quiet(&pool).await;
    // Nothing else has to run: the next claim recovers it.
    let again = outbox.claim().await.unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].event_id, event.event_id);
    assert_eq!(again[0].attempts, 2);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_row_that_keeps_killing_relays_ends_as_failed() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 2,
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    outbox.claim().await.unwrap();
    go_quiet(&pool).await;
    assert_eq!(outbox.claim().await.unwrap().len(), 1);
    go_quiet(&pool).await;
    assert_eq!(outbox.claim().await.unwrap().len(), 0);
    let (status, attempts, error) = row(&pool, &event).await;
    assert_eq!(status, "failed");
    assert_eq!(attempts, 2);
    assert_eq!(error.as_deref(), Some("abandoned by its relay"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_slow_publish_that_reports_in_is_not_taken_for_lost() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        heartbeat_interval: Duration::from_millis(30),
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let relay = Relay::start(&outbox, |_| async {
        tokio::time::sleep(Duration::from_millis(400)).await;
        Ok(())
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(status(&pool, &event).await, "claimed");
    go_quiet(&pool).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(outbox.requeue_stale().await.unwrap(), 0);
    drained(&outbox).await;
    relay.stop().await;
    assert_eq!(row(&pool, &event).await.1, 1);
    assert_eq!(status(&pool, &event).await, "sent");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_batch_of_quick_publishes_reports_in_too() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        heartbeat_interval: Duration::from_millis(50),
        ..quick()
    })
    .await;
    // Each publish is shorter than the heartbeat, and the batch is much longer.
    let mut events = Vec::new();
    for number in 0..30 {
        events.push(add(&pool, &number.to_string()).await);
    }
    let relay = Relay::start(&outbox, |_| async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(())
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    go_quiet(&pool).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(outbox.requeue_stale().await.unwrap(), 0);
    drained(&outbox).await;
    relay.stop().await;
    for event in &events {
        assert_eq!(row(&pool, event).await.1, 1, "published twice");
    }
}

// OBX-6: stale work.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_late_failure_changes_nothing() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let first = outbox.claim().await.unwrap();
    go_quiet(&pool).await;
    let second = outbox.claim().await.unwrap();
    outbox.failed(&first[0], "from the past").await.unwrap();
    assert_eq!(row(&pool, &event).await, ("claimed".to_string(), 2, None));
    outbox.sent(&second).await.unwrap();
    assert_eq!(status(&pool, &event).await, "sent");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_late_release_changes_nothing() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let first = outbox.claim().await.unwrap();
    go_quiet(&pool).await;
    let _second = outbox.claim().await.unwrap();
    assert_eq!(outbox.release(&first).await.unwrap(), 0);
    assert_eq!(row(&pool, &event).await.0, "claimed");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_late_success_still_counts() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let first = outbox.claim().await.unwrap();
    go_quiet(&pool).await;
    outbox.requeue_stale().await.unwrap();
    // The broker has it, so the row need not go out again.
    assert_eq!(outbox.sent(&first).await.unwrap(), 1);
    assert_eq!(status(&pool, &event).await, "sent");
    assert_eq!(outbox.claim().await.unwrap().len(), 0);
    assert_eq!(outbox.sent(&first).await.unwrap(), 0);
}

// OBX-7: a poison row.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_poison_row_does_not_hold_up_other_keys() {
    let _table = TABLE.lock().await;
    // Its retry is far off, so it never finishes during the test, and a round that
    // stopped at it would sleep past the end of the test too.
    let (outbox, pool) = fresh(OutboxOptions {
        retry_backoff: Duration::from_mins(10),
        poll_interval: Duration::from_hours(1),
        ..quick()
    })
    .await;
    let poison = add(&pool, "poison").await;
    let mut others = Vec::new();
    for number in 0..10 {
        others.push(add(&pool, &number.to_string()).await);
    }
    let broker = Broker::default();
    let publisher = broker.clone();
    let poison_id = poison.event_id;
    let relay = Relay::start(&outbox, move |event| {
        let broker = publisher.clone();
        async move {
            if event.event_id == poison_id {
                return Err(Failure::rejected("too large"));
            }
            broker.take(event);
            Ok(())
        }
    });
    let until = Instant::now() + DEADLINE;
    while broker.len() < 10 {
        assert!(Instant::now() < until, "held up behind the poison row");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    relay.stop().await;
    for event in &others {
        assert_eq!(status(&pool, event).await, "sent");
    }
    let (status, attempts, error) = row(&pool, &poison).await;
    assert_eq!((status.as_str(), attempts), ("pending", 1));
    assert_eq!(error.as_deref(), Some("too large"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_row_out_of_attempts_is_kept_and_frees_its_key() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 3,
        retention_days: 0,
        ..quick()
    })
    .await;
    let poison = add(&pool, "1").await;
    let behind = add(&pool, "1").await;
    let broker = Broker::default();
    let publisher = broker.clone();
    let tries = Arc::new(AtomicUsize::new(0));
    let counted = tries.clone();
    let poison_id = poison.event_id;
    let relay = Relay::start(&outbox, move |event| {
        let broker = publisher.clone();
        let counted = counted.clone();
        async move {
            if event.event_id == poison_id {
                counted.fetch_add(1, Ordering::SeqCst);
                return Err(Failure::rejected("too large"));
            }
            broker.take(event);
            Ok(())
        }
    });
    let stats = drained(&outbox).await;
    relay.stop().await;
    assert_eq!(tries.load(Ordering::SeqCst), 3);
    assert_eq!(stats.failed, 1);
    assert_eq!(status(&pool, &poison).await, "failed");
    assert_eq!(broker.event_ids(), [behind.event_id]);
    // Still owed, so even a sweep of everything sent leaves it.
    outbox.purge_sent().await.unwrap();
    assert_eq!(status(&pool, &poison).await, "failed");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn failed_rows_can_be_tried_again() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 1,
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let batch = outbox.claim().await.unwrap();
    outbox.failed(&batch[0], "too large").await.unwrap();
    assert_eq!(status(&pool, &event).await, "failed");
    assert_eq!(outbox.claim().await.unwrap().len(), 0);

    assert_eq!(outbox.retry_failed().await.unwrap(), 1);
    assert_eq!(row(&pool, &event).await, ("pending".to_string(), 0, None));
    assert_eq!(outbox.claim().await.unwrap().len(), 1);
}

// OBX-8: order.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_key_goes_out_in_order_across_relays() {
    let _table = TABLE.lock().await;
    let options = OutboxOptions {
        batch_size: 7,
        ..quick()
    };
    let (outbox, pool) = fresh(options.clone()).await;
    let mut transaction = pool.begin().await.unwrap();
    for _ in 0..40 {
        for key in ["a", "b", "c", "d", "e"] {
            let event = Event::new("person", "updated", key, json!({}));
            write(&mut transaction, &event).await.unwrap();
        }
    }
    transaction.commit().await.unwrap();

    let broker = Broker::default();
    let mut relays = Vec::new();
    for _ in 0..4 {
        let relay = Outbox::connect_existing(&uri(), options.clone())
            .await
            .unwrap();
        let broker = broker.clone();
        relays.push(Relay::start(&relay, move |event| {
            let broker = broker.clone();
            async move {
                tokio::task::yield_now().await;
                broker.take(event);
                Ok(())
            }
        }));
    }
    drained(&outbox).await;
    for relay in relays {
        relay.stop().await;
    }
    let mut last: HashMap<String, i64> = HashMap::new();
    let arrivals = broker.0.lock().unwrap();
    assert_eq!(arrivals.len(), 200);
    for event in arrivals.iter() {
        let before = last.insert(event.key.clone(), event.id).unwrap_or(0);
        assert!(before < event.id, "{} out of order", event.key);
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn one_topic_and_key_is_one_line() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    add(&pool, "1").await;
    add(&pool, "1").await;
    let other_topic = Event::new("place", "updated", "1", json!({}));
    let mut transaction = pool.begin().await.unwrap();
    write(&mut transaction, &other_topic).await.unwrap();
    transaction.commit().await.unwrap();
    // The second event waits for the first, and another topic does not.
    let batch = outbox.claim().await.unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[1].event_id, other_topic.event_id);
    assert!(batch[0].id < batch[1].id);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_retry_holds_back_only_its_own_key() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        retry_backoff: Duration::from_mins(10),
        ..quick()
    })
    .await;
    let first = add(&pool, "1").await;
    let _second = add(&pool, "1").await;
    let other = add(&pool, "2").await;
    let batch = outbox.claim().await.unwrap();
    assert_eq!(batch.len(), 2);
    outbox.failed(&batch[0], "no").await.unwrap();
    outbox.sent(&batch[1..]).await.unwrap();
    assert_eq!(status(&pool, &first).await, "pending");
    assert_eq!(status(&pool, &other).await, "sent");
    // The first is waiting for its retry, and the second must not pass it.
    assert_eq!(outbox.claim().await.unwrap().len(), 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_recovered_row_goes_before_those_behind_it() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let first = add(&pool, "1").await;
    let second = add(&pool, "1").await;
    outbox.claim().await.unwrap();
    go_quiet(&pool).await;
    let batch = outbox.claim().await.unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].event_id, first.event_id);
    outbox.sent(&batch).await.unwrap();
    let batch = outbox.claim().await.unwrap();
    assert_eq!(batch[0].event_id, second.event_id);
}

// OBX-9: an outage.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_outage_charges_no_row_and_stops_the_round() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 2,
        batch_size: 50,
        ..quick()
    })
    .await;
    let mut events = Vec::new();
    for number in 0..50 {
        events.push(add(&pool, &number.to_string()).await);
    }
    let tries = Arc::new(AtomicUsize::new(0));
    let counted = tries.clone();
    let relay = Relay::start(&outbox, move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { Err(Failure::unavailable("connection refused")) }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    relay.stop().await;
    // Far past `max_attempts`, had the relay been charging rows. Each round tried one
    // row, and it waited longer each time: 10, 20, 40, 80, 160 milliseconds.
    let rounds = tries.load(Ordering::SeqCst);
    assert!((2..=8).contains(&rounds), "{rounds} rounds");
    let stats = outbox.stats().await.unwrap();
    assert_eq!((stats.pending, stats.claimed, stats.failed), (50, 0, 0));
    for event in &events {
        assert_eq!(row(&pool, event).await.1, 0, "charged for an outage");
    }
    assert_eq!(
        row(&pool, &events[0]).await.2.as_deref(),
        Some("connection refused")
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_rows_go_out_once_the_broker_is_back() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    for number in 0..5 {
        add(&pool, &number.to_string()).await;
    }
    let up = Arc::new(AtomicUsize::new(0));
    let broker = Broker::default();
    let (publisher, state) = (broker.clone(), up.clone());
    let relay = Relay::start(&outbox, move |event| {
        let broker = publisher.clone();
        let state = state.clone();
        async move {
            if state.load(Ordering::SeqCst) == 0 {
                return Err(Failure::unavailable("down"));
            }
            broker.take(event);
            Ok(())
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    up.store(1, Ordering::SeqCst);
    drained(&outbox).await;
    relay.stop().await;
    assert_eq!(broker.len(), 5);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_refusal_called_an_outage_holds_up_only_its_key() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        retry_backoff: Duration::from_mins(10),
        ..quick()
    })
    .await;
    let poison = add(&pool, "poison").await;
    let behind = add(&pool, "poison").await;
    let mut others = Vec::new();
    for number in 0..10 {
        others.push(add(&pool, &number.to_string()).await);
    }
    let broker = Broker::default();
    let publisher = broker.clone();
    let poison_id = poison.event_id;
    let relay = Relay::start(&outbox, move |event| {
        let broker = publisher.clone();
        async move {
            if event.event_id == poison_id {
                return Err(Failure::unavailable("always"));
            }
            broker.take(event);
            Ok(())
        }
    });
    let until = Instant::now() + DEADLINE;
    while broker.len() < 10 {
        assert!(Instant::now() < until, "held up behind the poison row");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    relay.stop().await;
    assert_eq!(row(&pool, &poison).await.1, 0, "charged for an outage");
    assert_eq!(status(&pool, &behind).await, "pending");
}

// OBX-10: retries.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_retry_waits_for_its_backoff() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        retry_backoff: Duration::from_mins(10),
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let batch = outbox.claim().await.unwrap();
    outbox.failed(&batch[0], "no").await.unwrap();
    assert_eq!(status(&pool, &event).await, "pending");
    assert_eq!(outbox.claim().await.unwrap().len(), 0);
    let wait: f64 =
        sqlx::query_scalar("SELECT EXTRACT(EPOCH FROM run_at - now())::float8 FROM axa_outbox")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!((590.0..=600.0).contains(&wait), "{wait}");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn long_error_text_is_cut_short() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let batch = outbox.claim().await.unwrap();
    outbox.failed(&batch[0], &"é".repeat(5000)).await.unwrap();
    let kept = row(&pool, &event).await.2.unwrap();
    assert_eq!(kept.chars().count(), 1024);
}

// OBX-11: a panic.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_publisher_that_panics_fails_that_row_and_the_loop_carries_on() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 1,
        ..quick()
    })
    .await;
    let bad = add(&pool, "1").await;
    let good = add(&pool, "2").await;
    let bad_id = bad.event_id;
    let relay = Relay::start(&outbox, move |event| async move {
        assert_ne!(event.event_id, bad_id, "the publisher's own bug");
        Ok(())
    });
    drained(&outbox).await;
    relay.stop().await;
    let (status_of_bad, _, error) = row(&pool, &bad).await;
    assert_eq!(status_of_bad, "failed");
    assert_eq!(error.as_deref(), Some("the publisher panicked"));
    assert_eq!(status(&pool, &good).await, "sent");
}

// OBX-12: a hang.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_publish_past_its_limit_is_charged_and_stops_the_round() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        publish_timeout: Duration::from_millis(50),
        retry_backoff: Duration::from_mins(10),
        ..quick()
    })
    .await;
    let hung = add(&pool, "1").await;
    let next = add(&pool, "2").await;
    let hung_id = hung.event_id;
    let relay = Relay::start(&outbox, move |event| async move {
        if event.event_id == hung_id {
            std::future::pending::<()>().await;
        }
        Ok(())
    });
    drained_of(&outbox, 1).await;
    relay.stop().await;
    let (status_of_hung, attempts, error) = row(&pool, &hung).await;
    assert_eq!((status_of_hung.as_str(), attempts), ("pending", 1));
    assert_eq!(error.as_deref(), Some("the publish timed out"));
    // The round stopped without charging the next row, which then went out. It was
    // claimed twice, and only the claim that published it counts.
    let (status_of_next, attempts, _) = row(&pool, &next).await;
    assert_eq!((status_of_next.as_str(), attempts), ("sent", 1));
}

/// Wait until only `left` rows are pending.
async fn drained_of(outbox: &Outbox, left: i64) {
    let until = Instant::now() + DEADLINE;
    while outbox.stats().await.unwrap().pending != left
        || outbox.stats().await.unwrap().claimed != 0
    {
        assert!(Instant::now() < until, "not drained");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// OBX-13: sweeping.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn only_old_sent_rows_are_swept() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 1,
        ..quick()
    })
    .await;
    let old_sent = add(&pool, "1").await;
    let new_sent = add(&pool, "2").await;
    let failed = add(&pool, "3").await;
    let claimed = add(&pool, "4").await;
    let pending = add(&pool, "5").await;
    let set = async |event: &Event, status: &str| {
        sqlx::query(
            "UPDATE axa_outbox SET status = $2, sent_at = now() - interval '30 days',
             created_at = now() - interval '30 days', updated_at = now()
             WHERE event_id = $1",
        )
        .bind(event.event_id)
        .bind(status)
        .execute(&pool)
        .await
        .unwrap();
    };
    set(&old_sent, "sent").await;
    set(&failed, "failed").await;
    set(&claimed, "claimed").await;
    set(&pending, "pending").await;
    sqlx::query("UPDATE axa_outbox SET status = 'sent', sent_at = now() WHERE event_id = $1")
        .bind(new_sent.event_id)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(outbox.purge_sent().await.unwrap(), 1);
    assert_eq!(rows(&pool).await, 4);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_relay_sweeps_by_itself() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        retention_days: 0,
        maintenance_interval: Duration::from_millis(10),
        ..quick()
    })
    .await;
    add(&pool, "1").await;
    let broker = Broker::default();
    let relay = Relay::start(&outbox, taking(&broker));
    let until = Instant::now() + DEADLINE;
    while rows(&pool).await != 0 {
        assert!(Instant::now() < until, "sent row not swept");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    relay.stop().await;
}

// OBX-15: the table.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn relays_booting_together_all_connect() {
    let _table = TABLE.lock().await;
    let pool = admin().await;
    // Setup only races when the table is really missing.
    for _ in 0..5 {
        sqlx::query("DROP TABLE IF EXISTS axa_outbox")
            .execute(&pool)
            .await
            .unwrap();
        let boots: Vec<_> = (0..8)
            .map(|_| {
                tokio::spawn(async move { Outbox::connect(&uri(), quick()).await.map(|_| ()) })
            })
            .collect();
        for boot in boots {
            boot.await.unwrap().unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn connecting_to_an_existing_table_changes_no_schema() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    assert!(Outbox::connect_existing(&uri(), quick()).await.is_ok());

    sqlx::query("DROP TABLE axa_outbox")
        .execute(&pool)
        .await
        .unwrap();
    assert!(Outbox::connect_existing(&uri(), quick()).await.is_err());
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_name = 'axa_outbox'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tables, 0);

    // A migration running the statements is what `connect_existing` expects.
    for statement in SETUP_STATEMENTS {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    assert!(Outbox::connect_existing(&uri(), quick()).await.is_ok());
}

// OBX-16: the size of the table.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn what_is_owed_is_found_without_reading_sent_rows() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    sqlx::query(
        "INSERT INTO axa_outbox (event_id, topic, kind, key, payload, status, sent_at)
         SELECT gen_random_uuid(), 'person', 'updated', n::text, '{}', 'sent', now()
         FROM generate_series(1, 20000) AS n",
    )
    .execute(&pool)
    .await
    .unwrap();
    add(&pool, "new").await;
    sqlx::query("ANALYZE axa_outbox")
        .execute(&pool)
        .await
        .unwrap();
    let plan = async |query: &str| -> String {
        let lines: Vec<String> =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("EXPLAIN {query}")))
                .fetch_all(&pool)
                .await
                .unwrap();
        lines.join("\n")
    };
    // The check a claim makes for each candidate, and the statistics.
    let earlier = plan(
        "SELECT 1 FROM axa_outbox earlier
         WHERE earlier.topic = 'person' AND earlier.key = 'new' AND earlier.id < 1000000
           AND earlier.status <> 'sent' AND earlier.status <> 'failed'",
    )
    .await;
    assert!(earlier.contains("axa_outbox_owed"), "{earlier}");
    let stats = plan("SELECT count(*) FROM axa_outbox WHERE status <> 'sent'").await;
    assert!(stats.contains("axa_outbox_owed"), "{stats}");
    assert_eq!(outbox.claim().await.unwrap().len(), 1);
}

// OBX-17: shutdown.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_publish_in_progress_finishes_and_the_rest_go_back() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let mut events = Vec::new();
    for number in 0..5 {
        events.push(add(&pool, &number.to_string()).await);
    }
    let started = Arc::new(tokio::sync::Notify::new());
    let told = started.clone();
    let relay = Relay::start(&outbox, move |_| {
        let told = told.clone();
        async move {
            told.notify_one();
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(())
        }
    });
    started.notified().await;
    relay.stop().await;
    assert_eq!(row(&pool, &events[0]).await, ("sent".to_string(), 1, None));
    for event in &events[1..] {
        assert_eq!(row(&pool, event).await, ("pending".to_string(), 0, None));
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_relay_stops_while_waiting_for_work() {
    let _table = TABLE.lock().await;
    let (outbox, _pool) = fresh(OutboxOptions {
        poll_interval: Duration::from_hours(1),
        ..quick()
    })
    .await;
    let broker = Broker::default();
    let relay = Relay::start(&outbox, taking(&broker));
    tokio::time::sleep(Duration::from_millis(50)).await;
    relay.stop().await;
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_relay_nobody_can_stop_stops() {
    let _table = TABLE.lock().await;
    let (outbox, _pool) = fresh(OutboxOptions {
        poll_interval: Duration::from_hours(1),
        ..quick()
    })
    .await;
    let broker = Broker::default();
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(outbox.run(taking(&broker), stopped));
    drop(stop);
    tokio::time::timeout(DEADLINE, task)
        .await
        .expect("spinning with no sender")
        .unwrap();
}

// OBX-18: what is owed.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn stats_say_what_is_owed() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 1,
        batch_size: 2,
        ..quick()
    })
    .await;
    let empty = outbox.stats().await.unwrap();
    assert_eq!(empty.pending + empty.claimed + empty.failed, 0);
    assert_eq!(empty.oldest, None);

    let oldest = add(&pool, "1").await;
    add(&pool, "2").await;
    add(&pool, "3").await;
    add(&pool, "4").await;
    sqlx::query("UPDATE axa_outbox SET created_at = now() - interval '1 hour' WHERE event_id = $1")
        .bind(oldest.event_id)
        .execute(&pool)
        .await
        .unwrap();
    let batch = outbox.claim().await.unwrap();
    outbox.sent(&batch[..1]).await.unwrap();
    outbox.failed(&batch[1], "no").await.unwrap();
    outbox.claim().await.unwrap();
    add(&pool, "5").await;
    let stats = outbox.stats().await.unwrap();
    assert_eq!((stats.pending, stats.claimed, stats.failed), (1, 2, 1));
    // The hour-old row went out, so this is the age of what is left.
    assert!(stats.oldest.unwrap() < Duration::from_mins(1));
}

// OBX-19: a service's own loop.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_service_can_drive_the_outbox_from_its_own_loop() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        batch_size: 3,
        ..quick()
    })
    .await;
    let mut events = Vec::new();
    for number in 0..4 {
        events.push(add(&pool, &number.to_string()).await);
    }

    // A broker client that batches: send them all, flush once, then acknowledge.
    let batch = outbox.claim().await.unwrap();
    assert_eq!(batch.len(), 3);
    assert_eq!(outbox.heartbeat(&batch).await.unwrap(), 3);
    assert_eq!(outbox.sent(&batch).await.unwrap(), 3);

    let batch = outbox.claim().await.unwrap();
    assert_eq!(batch[0].event_id, events[3].event_id);
    assert_eq!(batch[0].payload, json!({ "key": "3" }));
    assert_eq!(batch[0].topic, "person");
    assert_eq!(batch[0].kind, "updated");
    assert_eq!(outbox.release(&batch).await.unwrap(), 1);
    assert_eq!(
        row(&pool, &events[3]).await,
        ("pending".to_string(), 0, None)
    );
    assert_eq!(status(&pool, &events[0]).await, "sent");
    assert!(matches!(outbox.ping().await, Ok(())));
}
