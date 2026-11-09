//! The outbox against a real Postgres. These are ignored by default. Run them with
//! `AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-transactional-outbox --all-features -- --ignored`.
//!
//! An event's record is the point of most of these, so each one reads the tables rather
//! than trusting what the API says. They share the tables, and each starts by dropping
//! them, as the tables refuse to be emptied.

use std::{
    collections::{HashMap, HashSet},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axa_transactional_outbox::{
    Attempt, Claimed, Decision, Event, Failure, IMPORT_STATEMENTS, Order, Outbox, OutboxOptions,
    Outcome, REBUILD_OWED_STATEMENTS, Refused, SETUP_STATEMENTS, Stats, WriteOptions, Writer,
    decide, erase, inbox, record, release, write,
};
use serde_json::json;
use sqlx::{PgConnection, PgPool, postgres::PgPoolOptions};
use tokio::sync::{Notify, watch};
use uuid::Uuid;

/// Held for a whole test: these tests share the tables.
static TABLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Being set makes a process the relay that gets killed.
const ROLE: &str = "AXA_OUTBOX_TEST_ROLE";

/// How long a test waits for anything, after which it fails.
const DEADLINE: Duration = Duration::from_secs(20);

const TABLES: &str = "DROP TABLE IF EXISTS axa_outbox_attempts, axa_outbox_payloads,
    axa_outbox_owed, axa_outbox_leases, axa_outbox_events, axa_outbox_test_audit CASCADE";

fn uri() -> String {
    std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL")
}

/// Quick to poll, and retrying at once.
fn quick() -> OutboxOptions {
    OutboxOptions {
        poll_interval: Duration::from_millis(10),
        retry_backoff: Duration::ZERO,
        hold_interval: Duration::from_millis(10),
        ..OutboxOptions::default()
    }
}

async fn admin() -> PgPool {
    PgPoolOptions::new().connect(&uri()).await.unwrap()
}

/// An outbox on empty tables, and a pool for looking at them.
async fn fresh(options: OutboxOptions) -> (Outbox, PgPool) {
    let pool = admin().await;
    sqlx::query(TABLES).execute(&pool).await.unwrap();
    let outbox = Outbox::connect(&uri(), options).await.unwrap();
    sqlx::query(
        "CREATE TABLE axa_outbox_test_audit (
            attempt BIGINT NOT NULL, event_id UUID NOT NULL, outcome TEXT NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    (outbox, pool)
}

/// Write an event in its own committed transaction, returning it.
async fn add(pool: &PgPool, key: &str) -> Event {
    add_to(pool, "person", key).await
}

async fn add_to(pool: &PgPool, topic: &str, key: &str) -> Event {
    let event = Event::new(topic, "updated", key, json!({ "key": key }));
    let mut transaction = pool.begin().await.unwrap();
    write(&mut transaction, &event).await.unwrap();
    transaction.commit().await.unwrap();
    event
}

/// Every attempt of an event, in order.
async fn outcomes(pool: &PgPool, event: &Event) -> Vec<String> {
    sqlx::query_scalar("SELECT outcome FROM axa_outbox_attempts WHERE event_id = $1 ORDER BY id")
        .bind(event.event_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

/// An event's latest attempt, and its reason, which together are its state.
async fn latest(pool: &PgPool, event: &Event) -> Option<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT outcome, reason FROM axa_outbox_attempts WHERE event_id = $1
         ORDER BY id DESC LIMIT 1",
    )
    .bind(event.event_id)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn state(pool: &PgPool, event: &Event) -> String {
    latest(pool, event)
        .await
        .map_or_else(|| "written".to_string(), |(outcome, _)| outcome)
}

/// How many times an event has been claimed.
async fn claims_of(pool: &PgPool, event: &Event) -> usize {
    outcomes(pool, event)
        .await
        .iter()
        .filter(|outcome| *outcome == "claimed")
        .count()
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

/// As if every relay had gone quiet an hour ago, which is how `stale_minutes` passes.
async fn go_quiet(pool: &PgPool) {
    sqlx::query("UPDATE axa_outbox_leases SET heard_at = now() - interval '1 hour'")
        .execute(pool)
        .await
        .unwrap();
}

/// Claimed events, and how many of those have gone quiet.
async fn claims(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE NOT EXISTS (
             SELECT 1 FROM axa_outbox_leases l
             WHERE l.claim = o.claim AND l.heard_at >= now() - interval '1 minute'))
         FROM axa_outbox_owed o WHERE o.state = 'claimed'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Wait until a relay holds something.
async fn until_claimed(pool: &PgPool) {
    let until = Instant::now() + DEADLINE;
    while claims(pool).await.0 == 0 {
        assert!(Instant::now() < until, "the relay claimed nothing");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Wait until every claim that [`go_quiet`] aged has been heard from again.
async fn until_reported_in(pool: &PgPool) {
    let until = Instant::now() + DEADLINE;
    loop {
        let (claimed, quiet) = claims(pool).await;
        assert!(claimed > 0, "the claim ended before it reported in");
        if quiet == 0 {
            return;
        }
        assert!(Instant::now() < until, "the relay never reported in");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
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

/// Wait until `done` says so.
async fn until<F: AsyncFn() -> bool>(what: &str, done: F) {
    let until = Instant::now() + DEADLINE;
    while !done().await {
        assert!(Instant::now() < until, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Claim with no rule and no audit.
async fn take(outbox: &Outbox) -> Vec<Claimed> {
    outbox
        .claim(decide::deliver_all, record::nothing)
        .await
        .unwrap()
}

/// Record `events` as delivered, each in a transaction of its own.
async fn deliver(outbox: &Outbox, pool: &PgPool, events: &[Claimed]) -> usize {
    let mut transaction = pool.begin().await.unwrap();
    let written = outbox.delivered(&mut transaction, events).await.unwrap();
    transaction.commit().await.unwrap();
    written.len()
}

async fn fail(outbox: &Outbox, pool: &PgPool, event: &Claimed, reason: &str) -> Option<Attempt> {
    let mut transaction = pool.begin().await.unwrap();
    let written = outbox
        .failed(&mut transaction, event, reason)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    written
}

async fn give_back(outbox: &Outbox, pool: &PgPool, events: &[Claimed]) -> u64 {
    let mut transaction = pool.begin().await.unwrap();
    let returned = outbox.returned(&mut transaction, events).await.unwrap();
    transaction.commit().await.unwrap();
    returned
}

async fn release_as(pool: &PgPool, event: &Event, by: &str) -> bool {
    let mut transaction = pool.begin().await.unwrap();
    let released = release(&mut transaction, event.event_id, by, "cause_fixed")
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    released
}

/// A record closure writing the service's audit row.
async fn audit(attempt: &Attempt, transaction: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO axa_outbox_test_audit (attempt, event_id, outcome) VALUES ($1, $2, $3)",
    )
    .bind(attempt.id)
    .bind(attempt.event_id)
    .bind(attempt.outcome.as_str())
    .execute(&mut *transaction)
    .await?;
    Ok(())
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
    /// A relay with no rule and no audit.
    fn start<P, F>(outbox: &Outbox, publish: P) -> Self
    where
        P: Fn(Claimed) -> F + Send + Sync + 'static,
        F: Future<Output = Result<(), Failure>> + Send + 'static,
    {
        let outbox = outbox.clone();
        Self::spawn(|stopped| outbox.run(decide::deliver_all, publish, record::nothing, stopped))
    }

    /// Any relay. The caller builds the run, so that its closures are known types
    /// where it is spawned.
    fn spawn<G, R>(go: G) -> Self
    where
        G: FnOnce(watch::Receiver<bool>) -> R,
        R: Future<Output = ()> + Send + 'static,
    {
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(go(stopped));
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
    assert_eq!(count(&pool, "axa_outbox_events").await, 0, "seen early");
    transaction.rollback().await.unwrap();
    assert_eq!(count(&pool, "axa_outbox_events").await, 0);
    assert_eq!(count(&pool, "axa_outbox_payloads").await, 0);
    assert_eq!(count(&pool, "axa_outbox_owed").await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_event_commits_with_its_transaction() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    assert_eq!(state(&pool, &event).await, "written");
    assert_eq!(outbox.stats().await.unwrap().pending, 1);
    let payload: serde_json::Value =
        sqlx::query_scalar("SELECT payload FROM axa_outbox_payloads WHERE event_id = $1")
            .bind(event.event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(payload, json!({ "key": "1" }));
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
    axa_transactional_outbox::write_seaorm(&transaction, &dropped)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert_eq!(count(&pool, "axa_outbox_events").await, 0);

    let kept = Event::new("person", "updated", "1", json!({ "name": "Ada" }));
    let transaction = db.begin().await.unwrap();
    axa_transactional_outbox::write_seaorm(&transaction, &kept)
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    // A refused payload fails a SeaORM write too.
    let writer = Writer::new(WriteOptions {
        max_payload_bytes: 4,
        ..WriteOptions::default()
    })
    .unwrap();
    let transaction = db.begin().await.unwrap();
    let large = Event::new("person", "updated", "1", json!({ "name": "Ada" }));
    assert!(writer.write_seaorm(&transaction, &large).await.is_err());
    transaction.rollback().await.unwrap();

    let batch = take(&outbox).await;
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].event_id, kept.event_id);
    assert_eq!(batch[0].payload, json!({ "name": "Ada" }));
    assert_eq!(batch[0].key, "1");
}

// OBX-2 and OBX-3: nothing committed is lost, and delivered means the broker has it.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_event_committed_before_a_crash_is_still_published() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    // The commit happened, and no relay ever heard of it. A later one finds the event.
    let event = add(&pool, "1").await;
    let outbox = Outbox::connect_existing(&uri(), quick()).await.unwrap();
    let broker = Broker::default();
    let relay = Relay::start(&outbox, taking(&broker));
    drained(&outbox).await;
    relay.stop().await;
    assert_eq!(broker.event_ids(), [event.event_id]);
    assert_eq!(outcomes(&pool, &event).await, ["claimed", "delivered"]);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_failed_publish_is_not_delivered() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 5,
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let relay = Relay::start(&outbox, |_| async {
        Err(Failure::rejected("broker_said_no"))
    });
    until("not tried twice", async || {
        claims_of(&pool, &event).await >= 2
    })
    .await;
    relay.stop().await;
    let all = outcomes(&pool, &event).await;
    assert!(!all.contains(&"delivered".to_string()), "{all:?}");
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT reason FROM axa_outbox_attempts WHERE event_id = $1 AND outcome = 'rejected'
         LIMIT 1",
    )
    .bind(event.event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reason.as_deref(), Some("broker_said_no"));
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
            decide::deliver_all,
            move |event| {
                let (pool, seen) = (pool.clone(), seen.clone());
                async move {
                    sqlx::query("INSERT INTO axa_outbox_test_broker (event_id) VALUES ($1)")
                        .bind(event.event_id)
                        .execute(&pool)
                        .await
                        .map_err(|_| Failure::unavailable("broker_down"))?;
                    if seen.fetch_add(1, Ordering::SeqCst) == 2 {
                        std::future::pending::<()>().await;
                    }
                    Ok(())
                }
            },
            record::nothing,
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
    // SIGKILL: no unwinding, no return, and no chance to say anything.
    child.kill().unwrap();
    child.wait().unwrap();

    // Two acknowledged, one taken and not acknowledged, seven claimed and never tried,
    // and ten never claimed.
    let mut states: HashMap<String, usize> = HashMap::new();
    for event in &events {
        *states.entry(state(&pool, event).await).or_default() += 1;
    }
    assert_eq!(states["delivered"], 2, "{states:?}");
    assert_eq!(states["claimed"], 8, "{states:?}");
    assert_eq!(states["written"], 10, "{states:?}");

    // Events in a live claim are left alone until it goes quiet.
    let replacement = Outbox::connect_existing(&uri(), quick()).await.unwrap();
    assert_eq!(replacement.recover(record::nothing).await.unwrap(), 0);
    go_quiet(&pool).await;

    let pool_for_publisher = pool.clone();
    let relay = Relay::start(&replacement, move |event| {
        let pool = pool_for_publisher.clone();
        async move {
            sqlx::query("INSERT INTO axa_outbox_test_broker (event_id) VALUES ($1)")
                .bind(event.event_id)
                .execute(&pool)
                .await
                .map_err(|_| Failure::unavailable("broker_down"))?;
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
        assert_eq!(state(&pool, event).await, "delivered");
        assert!(counts.contains_key(&event.event_id), "lost an event");
    }
    // Exactly the event caught between the broker and the ack came twice.
    assert_eq!(arrived.len(), 21);
    assert_eq!(counts.values().filter(|count| **count == 2).count(), 1);
    // The eight lost with the relay are on record as lost.
    let lost: i64 =
        sqlx::query_scalar("SELECT count(*) FROM axa_outbox_attempts WHERE outcome = 'lost'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(lost, 8);
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
    let claimed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM axa_outbox_attempts WHERE outcome = 'claimed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(claimed, 400, "an event claimed twice");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn two_claims_at_once_share_nothing() {
    let _table = TABLE.lock().await;
    let options = OutboxOptions {
        batch_size: 5,
        ..quick()
    };
    let (_outbox, pool) = fresh(options.clone()).await;
    for number in 0..40 {
        add(&pool, &number.to_string()).await;
    }
    let mut claims = Vec::new();
    for _ in 0..8 {
        let outbox = Outbox::connect_existing(&uri(), options.clone())
            .await
            .unwrap();
        claims.push(tokio::spawn(async move { take(&outbox).await }));
    }
    let mut held = Vec::new();
    for claim in claims {
        held.extend(claim.await.unwrap().into_iter().map(|event| event.id));
    }
    let unique: HashSet<_> = held.iter().collect();
    assert_eq!(held.len(), unique.len(), "one event in two claims");
}

// OBX-5: a claim whose relay died is recovered from its attempts.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_claim_not_heard_from_comes_back() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    assert_eq!(take(&outbox).await.len(), 1);
    assert_eq!(take(&outbox).await.len(), 0, "taken twice");
    go_quiet(&pool).await;
    // Nothing else has to run: the next claim recovers it, and edits nothing.
    let again = take(&outbox).await;
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].event_id, event.event_id);
    assert_eq!(again[0].attempts, 2);
    assert_eq!(
        outcomes(&pool, &event).await,
        ["claimed", "lost", "claimed"]
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_event_that_keeps_killing_relays_is_abandoned() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 2,
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    take(&outbox).await;
    go_quiet(&pool).await;
    assert_eq!(take(&outbox).await.len(), 1);
    go_quiet(&pool).await;
    assert_eq!(take(&outbox).await.len(), 0);
    assert_eq!(
        outcomes(&pool, &event).await,
        ["claimed", "lost", "claimed", "abandoned"]
    );
    assert_eq!(outbox.stats().await.unwrap().failed, 1);
    // No lease is left behind once its claims have ended.
    assert_eq!(count(&pool, "axa_outbox_leases").await, 0);
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
    // The publish lasts until the test lets it go, however slow the machine is.
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    let relay = Relay::start(&outbox, move |_| {
        let held = held.clone();
        async move {
            held.notified().await;
            Ok(())
        }
    });
    until_claimed(&pool).await;
    go_quiet(&pool).await;
    until_reported_in(&pool).await;
    assert_eq!(outbox.recover(record::nothing).await.unwrap(), 0);
    gate.notify_one();
    drained(&outbox).await;
    relay.stop().await;
    assert_eq!(outcomes(&pool, &event).await, ["claimed", "delivered"]);
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
    // The last waits for the test, so that the batch is still held when it looks.
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    let relay = Relay::start(&outbox, move |event| {
        let held = held.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if event.key == "29" {
                held.notified().await;
            }
            Ok(())
        }
    });
    until_claimed(&pool).await;
    go_quiet(&pool).await;
    until_reported_in(&pool).await;
    assert_eq!(outbox.recover(record::nothing).await.unwrap(), 0);
    gate.notify_one();
    drained(&outbox).await;
    relay.stop().await;
    for event in &events {
        assert_eq!(claims_of(&pool, event).await, 1, "published twice");
    }
}

// OBX-6: stale work.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_late_failure_changes_nothing() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let first = take(&outbox).await;
    go_quiet(&pool).await;
    let second = take(&outbox).await;
    assert!(
        fail(&outbox, &pool, &first[0], "from_the_past")
            .await
            .is_none()
    );
    assert_eq!(state(&pool, &event).await, "claimed");
    assert_eq!(deliver(&outbox, &pool, &second).await, 1);
    assert_eq!(state(&pool, &event).await, "delivered");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_late_return_changes_nothing() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let first = take(&outbox).await;
    go_quiet(&pool).await;
    let _second = take(&outbox).await;
    assert_eq!(give_back(&outbox, &pool, &first).await, 0);
    assert_eq!(state(&pool, &event).await, "claimed");
}

// OBX-29: every attempt names its relay and its claim.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_late_success_is_kept_as_its_own_attempt() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let other = Outbox::connect_existing(&uri(), quick()).await.unwrap();
    let event = add(&pool, "1").await;
    let first = take(&outbox).await;
    go_quiet(&pool).await;
    // Another relay takes over, and both then succeed.
    let second = take(&other).await;
    assert_eq!(second.len(), 1);
    assert_eq!(deliver(&outbox, &pool, &first).await, 1, "a late success");
    assert_eq!(deliver(&other, &pool, &second).await, 1);
    assert_eq!(
        outcomes(&pool, &event).await,
        ["claimed", "lost", "claimed", "delivered", "delivered"]
    );
    // The duplicate is on record: two deliveries, each naming its own claim.
    let deliveries: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT relay, claim FROM axa_outbox_attempts
         WHERE event_id = $1 AND outcome = 'delivered' ORDER BY id",
    )
    .bind(event.event_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        deliveries,
        [
            (outbox.relay(), first[0].claim),
            (other.relay(), second[0].claim)
        ]
    );
    // Each success counts once.
    assert_eq!(deliver(&outbox, &pool, &first).await, 0);
    assert_eq!(take(&outbox).await.len(), 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn each_attempt_names_its_relay_and_claim() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        retry_backoff: Duration::ZERO,
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let first = take(&outbox).await;
    fail(&outbox, &pool, &first[0], "broker_rejected")
        .await
        .unwrap();
    let second = take(&outbox).await;
    assert_ne!(first[0].claim, second[0].claim);
    deliver(&outbox, &pool, &second).await;
    let rows: Vec<(String, Uuid, Uuid)> = sqlx::query_as(
        "SELECT outcome, relay, claim FROM axa_outbox_attempts WHERE event_id = $1 ORDER BY id",
    )
    .bind(event.event_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let relay = outbox.relay();
    assert_eq!(
        rows,
        [
            ("claimed".to_string(), relay, first[0].claim),
            ("rejected".to_string(), relay, first[0].claim),
            ("claimed".to_string(), relay, second[0].claim),
            ("delivered".to_string(), relay, second[0].claim),
        ]
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_outcome_must_match_its_claim() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let other = Outbox::connect_existing(&uri(), quick()).await.unwrap();
    let event = add(&pool, "1").await;
    let batch = take(&outbox).await;
    // A made-up claim, or another relay's report of this one, writes nothing.
    let forged = Claimed {
        claim: Uuid::new_v4(),
        ..batch[0].clone()
    };
    assert_eq!(
        deliver(&outbox, &pool, std::slice::from_ref(&forged)).await,
        0
    );
    assert!(fail(&outbox, &pool, &forged, "forged").await.is_none());
    assert_eq!(give_back(&outbox, &pool, &[forged]).await, 0);
    let borrowed = Claimed {
        relay: other.relay(),
        ..batch[0].clone()
    };
    assert_eq!(deliver(&other, &pool, &[borrowed]).await, 0);
    assert_eq!(outcomes(&pool, &event).await, ["claimed"]);
    // The real claim still can.
    assert_eq!(deliver(&outbox, &pool, &batch).await, 1);
}

// OBX-7: a poison event.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_poison_event_does_not_hold_up_other_keys() {
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
                return Err(Failure::rejected("too_large"));
            }
            broker.take(event);
            Ok(())
        }
    });
    let until = Instant::now() + DEADLINE;
    while broker.len() < 10 {
        assert!(Instant::now() < until, "held up behind the poison event");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    relay.stop().await;
    for event in &others {
        assert_eq!(state(&pool, event).await, "delivered");
    }
    assert_eq!(
        latest(&pool, &poison).await,
        Some(("rejected".to_string(), Some("too_large".to_string())))
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_event_out_of_attempts_is_kept_and_frees_its_key() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 3,
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
                return Err(Failure::rejected("too_large"));
            }
            broker.take(event);
            Ok(())
        }
    });
    let stats = drained(&outbox).await;
    relay.stop().await;
    assert_eq!(tries.load(Ordering::SeqCst), 3);
    assert_eq!(stats.failed, 1);
    assert_eq!(
        outcomes(&pool, &poison).await,
        [
            "claimed", "rejected", "claimed", "rejected", "claimed", "failed"
        ]
    );
    assert_eq!(broker.event_ids(), [behind.event_id]);
    // Its payload is kept.
    assert_eq!(count(&pool, "axa_outbox_payloads").await, 2);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_failed_event_can_be_released_by_a_named_person() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 1,
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let batch = take(&outbox).await;
    let failed = fail(&outbox, &pool, &batch[0], "too_large").await.unwrap();
    assert_eq!(failed.outcome, Outcome::Failed);
    assert_eq!(take(&outbox).await.len(), 0);

    // Who and why must be codes, and a refused release changes nothing.
    let mut transaction = pool.begin().await.unwrap();
    let error = release(&mut transaction, event.event_id, "", "cause_fixed")
        .await
        .unwrap_err();
    assert!(matches!(&error, sqlx::Error::Encode(inner)
        if matches!(inner.downcast_ref::<Refused>(), Some(Refused::NotACode { what: "by" }))));
    drop(transaction);

    // A release rolled back with its transaction leaves the event failed.
    let mut transaction = pool.begin().await.unwrap();
    assert!(
        release(&mut transaction, event.event_id, "ops@example.com", "x")
            .await
            .unwrap()
    );
    transaction.rollback().await.unwrap();
    assert_eq!(state(&pool, &event).await, "failed");

    assert!(release_as(&pool, &event, "ops@example.com").await);
    assert!(!release_as(&pool, &event, "ops@example.com").await, "twice");
    let (by, reason): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT by, reason FROM axa_outbox_attempts WHERE event_id = $1 AND outcome = 'released'",
    )
    .bind(event.event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(by.as_deref(), Some("ops@example.com"));
    assert_eq!(reason.as_deref(), Some("cause_fixed"));
    let again = take(&outbox).await;
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].attempts, 1, "attempts are cleared");
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
    let other_topic = add_to(&pool, "place", "1").await;
    // The second event waits for the first, and another topic does not.
    let batch = take(&outbox).await;
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
    let batch = take(&outbox).await;
    assert_eq!(batch.len(), 2);
    fail(&outbox, &pool, &batch[0], "no").await.unwrap();
    deliver(&outbox, &pool, &batch[1..]).await;
    assert_eq!(state(&pool, &first).await, "rejected");
    assert_eq!(state(&pool, &other).await, "delivered");
    // The first is waiting for its retry, and the second must not pass it.
    assert_eq!(take(&outbox).await.len(), 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_recovered_event_goes_before_those_behind_it() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let first = add(&pool, "1").await;
    let second = add(&pool, "1").await;
    take(&outbox).await;
    go_quiet(&pool).await;
    let batch = take(&outbox).await;
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].event_id, first.event_id);
    deliver(&outbox, &pool, &batch).await;
    let batch = take(&outbox).await;
    assert_eq!(batch[0].event_id, second.event_id);
}

// OBX-37: a topic with no order.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_topic_with_no_order_is_not_held_back() {
    let _table = TABLE.lock().await;
    let mut options = OutboxOptions {
        retry_backoff: Duration::from_mins(10),
        ..quick()
    };
    options.topic_order.insert("audit".into(), Order::None);
    let (outbox, pool) = fresh(options).await;
    let stuck = add_to(&pool, "audit", "1").await;
    let behind = add_to(&pool, "audit", "1").await;
    let ordered = add_to(&pool, "person", "1").await;
    let _waits = add_to(&pool, "person", "1").await;
    // Both audit events are taken at once, and only the first person event.
    let batch = take(&outbox).await;
    let taken: HashSet<Uuid> = batch.iter().map(|event| event.event_id).collect();
    assert_eq!(
        taken,
        HashSet::from([stuck.event_id, behind.event_id, ordered.event_id])
    );
    // One stuck audit event holds back nothing.
    let stuck_claim = batch.iter().find(|e| e.event_id == stuck.event_id).unwrap();
    fail(&outbox, &pool, stuck_claim, "no").await.unwrap();
    let rest: Vec<Claimed> = batch
        .iter()
        .filter(|e| e.event_id != stuck.event_id)
        .cloned()
        .collect();
    deliver(&outbox, &pool, &rest).await;
    let audit = add_to(&pool, "audit", "1").await;
    let next = take(&outbox).await;
    assert!(
        next.iter().any(|e| e.event_id == audit.event_id),
        "held back"
    );
}

// OBX-9: an outage.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_outage_charges_nothing_and_stops_the_round() {
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
        async { Err(Failure::unavailable("connection_refused")) }
    });
    until("too few rounds", async || tries.load(Ordering::SeqCst) >= 3).await;
    relay.stop().await;
    // Each round tried one event, and nothing was charged however many rounds ran.
    let stats = outbox.stats().await.unwrap();
    assert_eq!((stats.pending, stats.claimed, stats.failed), (50, 0, 0));
    let charged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM axa_outbox_attempts
         WHERE outcome NOT IN ('claimed', 'returned')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(charged, 0, "charged for an outage");
    let rounds: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM axa_outbox_attempts WHERE reason = 'connection_refused'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        usize::try_from(rounds).unwrap(),
        tries.load(Ordering::SeqCst)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_events_go_out_once_the_broker_is_back() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    for number in 0..5 {
        add(&pool, &number.to_string()).await;
    }
    let up = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = Broker::default();
    let (publisher, state, counted) = (broker.clone(), up.clone(), calls.clone());
    let relay = Relay::start(&outbox, move |event| {
        let (broker, state, counted) = (publisher.clone(), state.clone(), counted.clone());
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            if state.load(Ordering::SeqCst) == 0 {
                return Err(Failure::unavailable("down"));
            }
            broker.take(event);
            Ok(())
        }
    });
    until("never tried", async || calls.load(Ordering::SeqCst) >= 2).await;
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
        assert!(Instant::now() < until, "held up behind the poison event");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    relay.stop().await;
    assert_eq!(
        state(&pool, &poison).await,
        "returned",
        "charged for an outage"
    );
    assert_eq!(state(&pool, &behind).await, "written");
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
    let batch = take(&outbox).await;
    fail(&outbox, &pool, &batch[0], "no").await.unwrap();
    assert_eq!(state(&pool, &event).await, "rejected");
    assert_eq!(take(&outbox).await.len(), 0);
    let wait: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM retry_at - now())::float8 FROM axa_outbox_attempts
         WHERE outcome = 'rejected'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((590.0..=600.0).contains(&wait), "{wait}");
}

// OBX-32: a reason is a code.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_long_reason_is_kept_as_a_code() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let batch = take(&outbox).await;
    fail(&outbox, &pool, &batch[0], &"é".repeat(5000))
        .await
        .unwrap();
    assert_eq!(
        latest(&pool, &event).await.unwrap().1.as_deref(),
        Some("reason_too_long")
    );
    let batch = take(&outbox).await;
    fail(&outbox, &pool, &batch[0], "the broker said:\nno")
        .await
        .unwrap();
    assert_eq!(
        latest(&pool, &event).await.unwrap().1.as_deref(),
        Some("reason_not_a_code")
    );
    // The table refuses one too, whoever writes it.
    for reason in ["x".repeat(201), "a\nb".to_string()] {
        let refused = sqlx::query(
            "INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim, reason)
             VALUES ($1, 'returned', gen_random_uuid(), gen_random_uuid(), $2)",
        )
        .bind(event.event_id)
        .bind(reason)
        .execute(&pool)
        .await;
        assert!(refused.is_err());
    }
}

// OBX-11: a panic.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_publisher_that_panics_fails_that_event_and_the_loop_carries_on() {
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
    assert_eq!(
        latest(&pool, &bad).await,
        Some(("failed".to_string(), Some("publisher_panicked".to_string())))
    );
    assert_eq!(state(&pool, &good).await, "delivered");
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
    until("the next event never went", async || {
        state(&pool, &next).await == "delivered"
    })
    .await;
    relay.stop().await;
    assert_eq!(
        latest(&pool, &hung).await,
        Some(("rejected".to_string(), Some("timed_out".to_string())))
    );
    // The round stopped and gave the next event back uncharged, and it then went out.
    assert_eq!(
        outcomes(&pool, &next).await,
        ["claimed", "returned", "claimed", "delivered"]
    );
}

// OBX-15 and OBX-22: the tables.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn relays_booting_together_all_connect() {
    let _table = TABLE.lock().await;
    let pool = admin().await;
    // Setup only races when the tables are really missing.
    for _ in 0..5 {
        sqlx::query(TABLES).execute(&pool).await.unwrap();
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
async fn connecting_to_existing_tables_changes_no_schema() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    assert!(Outbox::connect_existing(&uri(), quick()).await.is_ok());

    sqlx::query(TABLES).execute(&pool).await.unwrap();
    assert!(Outbox::connect_existing(&uri(), quick()).await.is_err());
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_name LIKE 'axa_outbox_%'",
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

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn connect_makes_the_triggers() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    let triggers: Vec<(String, String)> = sqlx::query_as(
        "SELECT tgname::text, tgenabled::text FROM pg_trigger
         WHERE NOT tgisinternal AND tgrelid IN (
             'axa_outbox_events'::regclass, 'axa_outbox_attempts'::regclass,
             'axa_outbox_payloads'::regclass, 'axa_outbox_owed'::regclass,
             'axa_outbox_leases'::regclass)
         ORDER BY tgname",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let expected: Vec<(String, String)> = [
        ("axa_outbox_attempts_follow", "O"),
        ("axa_outbox_attempts_refuse", "A"),
        ("axa_outbox_attempts_refuse_truncate", "A"),
        ("axa_outbox_events_owe", "O"),
        ("axa_outbox_events_refuse", "A"),
        ("axa_outbox_events_refuse_truncate", "A"),
        ("axa_outbox_leases_guard", "A"),
        ("axa_outbox_owed_guard", "A"),
        ("axa_outbox_owed_refuse_truncate", "A"),
        ("axa_outbox_payloads_guard", "A"),
        ("axa_outbox_payloads_refuse_truncate", "A"),
    ]
    .iter()
    .map(|(name, enabled)| ((*name).to_string(), (*enabled).to_string()))
    .collect();
    assert_eq!(triggers, expected);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_missing_trigger_stops_the_relay() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    let setup = async || {
        for statement in SETUP_STATEMENTS {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
    };
    let refused = async |change: &str, named: &str| {
        sqlx::query(sqlx::AssertSqlSafe(change.to_string()))
            .execute(&pool)
            .await
            .unwrap();
        let error = Outbox::connect_existing(&uri(), quick())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(named), "{change}: {error}");
    };
    refused(
        "DROP TRIGGER axa_outbox_attempts_refuse ON axa_outbox_attempts",
        "axa_outbox_attempts_refuse",
    )
    .await;
    setup().await;
    assert!(Outbox::connect_existing(&uri(), quick()).await.is_ok());
    refused(
        "ALTER TABLE axa_outbox_events DISABLE TRIGGER axa_outbox_events_refuse",
        "axa_outbox_events_refuse",
    )
    .await;
    // On, but not always: `session_replication_role` could turn it off.
    refused(
        "ALTER TABLE axa_outbox_payloads ENABLE TRIGGER axa_outbox_payloads_guard",
        "axa_outbox_payloads_guard",
    )
    .await;
    setup().await;
    refused(
        "DROP TRIGGER axa_outbox_attempts_follow ON axa_outbox_attempts",
        "axa_outbox_attempts_follow",
    )
    .await;
    setup().await;
    refused(
        "DROP TRIGGER axa_outbox_owed_guard ON axa_outbox_owed",
        "axa_outbox_owed_guard",
    )
    .await;
    setup().await;
    refused(
        "ALTER TABLE axa_outbox_leases DISABLE TRIGGER axa_outbox_leases_guard",
        "axa_outbox_leases_guard",
    )
    .await;
    setup().await;
    refused("DROP INDEX axa_outbox_owed_line", "axa_outbox_owed_line").await;
    setup().await;
    assert!(Outbox::connect_existing(&uri(), quick()).await.is_ok());
}

// OBX-23: the record is only ever added to.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_tables_refuse_every_change() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    add(&pool, "1").await;
    let batch = take(&outbox).await;
    deliver(&outbox, &pool, &batch).await;
    let changes = [
        "UPDATE axa_outbox_events SET kind = 'changed'",
        "DELETE FROM axa_outbox_events",
        "TRUNCATE axa_outbox_events CASCADE",
        "UPDATE axa_outbox_attempts SET outcome = 'refused'",
        "DELETE FROM axa_outbox_attempts",
        "TRUNCATE axa_outbox_attempts",
        "UPDATE axa_outbox_payloads SET payload = '{}'",
        "DELETE FROM axa_outbox_payloads",
        "TRUNCATE axa_outbox_payloads",
    ];
    // Whoever sends them, and with triggers for replication turned off too.
    for role in ["origin", "replica"] {
        let mut connection = pool.acquire().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SET session_replication_role = {role}"
        )))
        .execute(&mut *connection)
        .await
        .unwrap();
        for change in changes {
            let result = sqlx::query(change).execute(&mut *connection).await;
            let error = result.expect_err(change).to_string();
            assert!(error.contains("refused"), "{change}: {error}");
        }
        sqlx::query("RESET session_replication_role")
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    assert_eq!(count(&pool, "axa_outbox_events").await, 1);
    assert_eq!(count(&pool, "axa_outbox_attempts").await, 2);
    assert_eq!(count(&pool, "axa_outbox_payloads").await, 1);
}

// OBX-39: the working state follows the record, and nothing else.

/// A row of `axa_outbox_owed`, leaving out its holds and times.
type OwedRow = (Uuid, String, Option<Uuid>, i32, Option<String>);

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_owed_table_changes_only_with_the_record() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        hold_interval: Duration::from_mins(10),
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let other = add(&pool, "2").await;
    let changes = [
        "DELETE FROM axa_outbox_owed WHERE event_id = $1",
        "UPDATE axa_outbox_owed SET state = 'failed' WHERE event_id = $1",
        "UPDATE axa_outbox_owed SET claim = gen_random_uuid() WHERE event_id = $1",
        "UPDATE axa_outbox_owed SET charges = 99 WHERE event_id = $1",
        "UPDATE axa_outbox_owed SET retry_at = now() + interval '1 year' WHERE event_id = $1",
        // A hold set by hand may not stop an event for good.
        "UPDATE axa_outbox_owed SET recheck_at = now() + interval '1 year' WHERE event_id = $1",
        "UPDATE axa_outbox_owed SET held_since = now() + interval '1 hour' WHERE event_id = $1",
        "INSERT INTO axa_outbox_owed (event_id, id, topic, key)
         SELECT event_id, id + 1000, topic, key || 'x' FROM axa_outbox_events
         WHERE event_id = $1",
        "TRUNCATE axa_outbox_owed",
    ];
    for role in ["origin", "replica"] {
        let mut connection = pool.acquire().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SET session_replication_role = {role}"
        )))
        .execute(&mut *connection)
        .await
        .unwrap();
        for change in changes {
            let error = sqlx::query(change)
                .bind(event.event_id)
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
    assert_eq!(count(&pool, "axa_outbox_owed").await, 2);
    // What the relay does still works: a hold, and the wait after a failed decision.
    let event_id = event.event_id;
    let decide = async move |claimed: &Claimed, tx: &mut PgConnection| {
        if claimed.event_id == event_id {
            return Ok(Decision::Hold);
        }
        sqlx::query("SELECT * FROM no_such_table")
            .execute(&mut *tx)
            .await?;
        Ok(Decision::Deliver)
    };
    assert_eq!(
        outbox.claim(decide, record::nothing).await.unwrap().len(),
        0
    );
    let waits: Vec<(Uuid, bool, bool)> = sqlx::query_as(
        "SELECT event_id, held_since IS NOT NULL, recheck_at > now()
         FROM axa_outbox_owed ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        waits,
        [(event.event_id, true, true), (other.event_id, false, true)]
    );
    // And so does everything the record's own triggers do.
    sqlx::query("UPDATE axa_outbox_owed SET recheck_at = NULL")
        .execute(&pool)
        .await
        .unwrap();
    let batch = take(&outbox).await;
    assert_eq!(batch.len(), 2);
    deliver(&outbox, &pool, &batch).await;
    assert_eq!(count(&pool, "axa_outbox_owed").await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_changed_lease_can_only_bring_a_recovery_which_is_recorded() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let batch = take(&outbox).await;
    // A lease cannot be dated ahead, to keep a dead relay's claim alive, nor moved.
    for change in [
        "UPDATE axa_outbox_leases SET heard_at = now() + interval '1 day'",
        "UPDATE axa_outbox_leases SET claim = gen_random_uuid()",
        "UPDATE axa_outbox_leases SET relay = gen_random_uuid()",
        "INSERT INTO axa_outbox_leases (claim, relay, heard_at)
         VALUES (gen_random_uuid(), gen_random_uuid(), now() + interval '1 day')",
    ] {
        assert!(
            sqlx::query(change).execute(&pool).await.is_err(),
            "{change}"
        );
    }
    // Dating one back, or deleting it, recovers its claim, and the record says so.
    sqlx::query("DELETE FROM axa_outbox_leases")
        .execute(&pool)
        .await
        .unwrap();
    let again = take(&outbox).await;
    assert_eq!(again.len(), 1);
    assert_eq!(
        outcomes(&pool, &event).await,
        ["claimed", "lost", "claimed"]
    );
    // The first claim's late success is still kept.
    assert_eq!(deliver(&outbox, &pool, &batch).await, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn rebuilding_owed_restores_it_from_the_record() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 2,
        retry_backoff: Duration::ZERO,
        hold_interval: Duration::from_mins(10),
        ..quick()
    })
    .await;
    // An event in each state the record leaves owed, and some it does not.
    let mut events = HashMap::new();
    for key in [
        "held",
        "claimed",
        "rejected",
        "failed",
        "released",
        "returned",
        "delivered",
        "refused",
        "erased",
    ] {
        events.insert(key, add(&pool, key).await);
    }
    let decide = async |event: &Claimed, _: &mut PgConnection| {
        Ok(match event.key.as_str() {
            "refused" => Decision::refuse("no_consent"),
            "held" => Decision::Hold,
            _ => Decision::Deliver,
        })
    };
    let first = outbox.claim(decide, record::nothing).await.unwrap();
    let pick = |batch: &[Claimed], key: &str| batch.iter().find(|e| e.key == key).unwrap().clone();
    for key in ["rejected", "failed", "released"] {
        fail(&outbox, &pool, &pick(&first, key), "no")
            .await
            .unwrap();
    }
    give_back(&outbox, &pool, &[pick(&first, "returned")]).await;
    deliver(&outbox, &pool, &[pick(&first, "delivered")]).await;
    let mut transaction = pool.begin().await.unwrap();
    erase(
        &mut transaction,
        events["erased"].event_id,
        "dpo",
        "erasure_request",
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let second = take(&outbox).await;
    for key in ["failed", "released"] {
        fail(&outbox, &pool, &pick(&second, key), "no")
            .await
            .unwrap();
    }
    give_back(
        &outbox,
        &pool,
        &[pick(&second, "rejected"), pick(&second, "returned")],
    )
    .await;
    assert!(release_as(&pool, &events["released"], "ops@example.com").await);

    let snapshot = async || -> Vec<OwedRow> {
        sqlx::query_as(
            "SELECT event_id, state, claim, charges, reason FROM axa_outbox_owed ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let before = snapshot().await;
    assert_eq!(before.len(), 6, "{before:?}");

    // Someone with the schema empties what is owed, leaving no trace in the record.
    let mut transaction = pool.begin().await.unwrap();
    for statement in [
        "ALTER TABLE axa_outbox_owed DISABLE TRIGGER axa_outbox_owed_guard",
        "DELETE FROM axa_outbox_owed",
        "ALTER TABLE axa_outbox_owed ENABLE ALWAYS TRIGGER axa_outbox_owed_guard",
    ] {
        sqlx::query(statement)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();
    assert_eq!(outbox.stats().await.unwrap().pending, 0, "nothing is owed");

    let mut transaction = pool.begin().await.unwrap();
    for statement in REBUILD_OWED_STATEMENTS {
        sqlx::query(statement)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();
    assert_eq!(snapshot().await, before);
    assert!(Outbox::connect_existing(&uri(), quick()).await.is_ok());
    // Holds are not kept, so the held event is decided again, with the rest owed.
    let again: HashSet<Uuid> = take(&outbox).await.iter().map(|e| e.event_id).collect();
    let owed: HashSet<Uuid> = ["held", "rejected", "released", "returned"]
        .iter()
        .map(|key| events[key].event_id)
        .collect();
    assert_eq!(again, owed);
}

// OBX-24: state is read from the attempts.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn state_is_read_from_the_attempts() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        retry_backoff: Duration::ZERO,
        ..quick()
    })
    .await;
    // The event has no state of its own.
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
         WHERE table_name = 'axa_outbox_events' ORDER BY ordinal_position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        columns,
        [
            "id",
            "event_id",
            "topic",
            "kind",
            "key",
            "fingerprint",
            "created_at"
        ]
    );
    let event = add(&pool, "1").await;
    let counts = async || {
        let stats = outbox.stats().await.unwrap();
        (stats.pending, stats.claimed, stats.failed)
    };
    assert_eq!(counts().await, (1, 0, 0));
    let batch = take(&outbox).await;
    assert_eq!(counts().await, (0, 1, 0));
    fail(&outbox, &pool, &batch[0], "no").await.unwrap();
    assert_eq!(counts().await, (1, 0, 0));
    // An attempt added by hand is what the state then is.
    let batch = take(&outbox).await;
    sqlx::query(
        "INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim)
         VALUES ($1, 'delivered', $2, $3)",
    )
    .bind(event.event_id)
    .bind(batch[0].relay)
    .bind(batch[0].claim)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(counts().await, (0, 0, 0));
    assert_eq!(take(&outbox).await.len(), 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn stats_count_every_outcome() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 1,
        batch_size: 10,
        hold_interval: Duration::from_mins(10),
        ..quick()
    })
    .await;
    let empty = outbox.stats().await.unwrap();
    assert_eq!(
        empty.pending + empty.claimed + empty.held + empty.failed + empty.delivered,
        0
    );
    assert_eq!(
        (empty.oldest, empty.oldest_held, empty.oldest_failed),
        (None, None, None)
    );

    // An hour-old event, written as a migration would.
    let old = Uuid::new_v4();
    sqlx::query(
        "WITH e AS (
             INSERT INTO axa_outbox_events (event_id, topic, kind, key, fingerprint, created_at)
             VALUES ($1, 'person', 'updated', 'old',
                 axa_outbox_fingerprint($1, 'person', 'updated', 'old', '{}', NULL),
                 now() - interval '1 hour')
             RETURNING event_id)
         INSERT INTO axa_outbox_payloads (event_id, payload) SELECT event_id, '{}' FROM e",
    )
    .bind(old)
    .execute(&pool)
    .await
    .unwrap();
    for key in ["deliver", "refuse", "fail", "hold", "claim"] {
        add(&pool, key).await;
    }
    let decide = async |event: &Claimed, _: &mut PgConnection| {
        Ok(match event.key.as_str() {
            "refuse" => Decision::refuse("no_consent"),
            "hold" => Decision::Hold,
            _ => Decision::Deliver,
        })
    };
    let batch = outbox.claim(decide, record::nothing).await.unwrap();
    let by_key = |key: &str| batch.iter().find(|e| e.key == key).unwrap().clone();
    deliver(&outbox, &pool, &[by_key("old"), by_key("deliver")]).await;
    fail(&outbox, &pool, &by_key("fail"), "no").await.unwrap();
    add(&pool, "new").await;
    let stats = outbox.stats().await.unwrap();
    assert_eq!(
        (
            stats.pending,
            stats.claimed,
            stats.held,
            stats.failed,
            stats.delivered,
            stats.refused
        ),
        (1, 1, 1, 1, 2, 1)
    );
    assert_eq!(stats.tampered, 0);
    // The hour-old event went out, so this is the age of what is left.
    assert!(stats.oldest.unwrap() < Duration::from_mins(1));
    assert!(stats.oldest_held.is_some());
    assert!(stats.oldest_failed.is_some());
}

// OBX-25: a refusal is final.

/// A decision that refuses whatever has the key `no`.
async fn refuse_no(event: &Claimed, _: &mut PgConnection) -> Result<Decision, sqlx::Error> {
    Ok(if event.key == "no" {
        Decision::refuse("no_consent")
    } else {
        Decision::Deliver
    })
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_refusal_is_final() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let refused = add(&pool, "no").await;
    assert_eq!(outbox.claim(refuse_no, audit).await.unwrap().len(), 0);
    assert_eq!(
        latest(&pool, &refused).await,
        Some(("refused".to_string(), Some("no_consent".to_string())))
    );
    // Asked again, even with a decision that would deliver it, it is never claimed.
    assert_eq!(take(&outbox).await.len(), 0);
    go_quiet(&pool).await;
    assert_eq!(take(&outbox).await.len(), 0);
    assert_eq!(outcomes(&pool, &refused).await, ["refused"]);
    let stats = outbox.stats().await.unwrap();
    assert_eq!((stats.pending, stats.failed, stats.refused), (0, 0, 1));
    // The refusal was recorded, in the claim's transaction.
    assert_eq!(count(&pool, "axa_outbox_test_audit").await, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_refusal_cannot_be_released() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let refused = add(&pool, "no").await;
    outbox.claim(refuse_no, record::nothing).await.unwrap();
    assert!(!release_as(&pool, &refused, "ops@example.com").await);
    assert_eq!(outcomes(&pool, &refused).await, ["refused"]);
    assert_eq!(take(&outbox).await.len(), 0);
    // Nor can a refusal after the claim, from a service's own loop.
    let later = add(&pool, "later").await;
    let batch = take(&outbox).await;
    let mut transaction = pool.begin().await.unwrap();
    let attempt = outbox
        .refused(&mut transaction, &batch[0], "no_consent")
        .await
        .unwrap()
        .unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(attempt.outcome, Outcome::Refused);
    assert!(!release_as(&pool, &later, "ops@example.com").await);
    assert_eq!(take(&outbox).await.len(), 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_refusal_frees_its_key() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let refused = add(&pool, "1").await;
    let behind = add(&pool, "1").await;
    // The first is refused, and the second, behind it on the same key, goes next.
    let refused_id = refused.event_id;
    let decide_first = async move |event: &Claimed, _: &mut PgConnection| {
        Ok(if event.event_id == refused_id {
            Decision::refuse("no_consent")
        } else {
            Decision::Deliver
        })
    };
    assert_eq!(
        outbox
            .claim(decide_first, record::nothing)
            .await
            .unwrap()
            .len(),
        0
    );
    let batch = take(&outbox).await;
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].event_id, behind.event_id);
}

// OBX-26: a hold.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_hold_charges_nothing() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        hold_interval: Duration::from_millis(1),
        ..quick()
    })
    .await;
    let event = add(&pool, "1").await;
    let hold = async |_: &Claimed, _: &mut PgConnection| Ok(Decision::Hold);
    for _ in 0..5 {
        assert_eq!(outbox.claim(hold, audit).await.unwrap().len(), 0);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Nothing written, nothing recorded, and the stats say it is held.
    assert_eq!(outcomes(&pool, &event).await, Vec::<String>::new());
    assert_eq!(count(&pool, "axa_outbox_test_audit").await, 0);
    let stats = outbox.stats().await.unwrap();
    assert_eq!((stats.held, stats.pending), (1, 0));
    // Once it goes, it is on its first attempt, with no backoff grown.
    let batch = take(&outbox).await;
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].attempts, 1);
    assert_eq!(outbox.stats().await.unwrap().held, 0);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_hold_lasts_as_long_as_it_is_given() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    // A standing hold, such as a report of death, that the service lifts.
    let standing = Arc::new(AtomicUsize::new(1));
    let asked = Arc::new(AtomicUsize::new(0));
    let (holding, asking) = (standing.clone(), asked.clone());
    let broker = Broker::default();
    let publish = taking(&broker);
    let relay = Relay::spawn(|stopped| {
        outbox.clone().run(
            async move |_: &Claimed, _: &mut PgConnection| {
                asking.fetch_add(1, Ordering::SeqCst);
                Ok(if holding.load(Ordering::SeqCst) == 1 {
                    Decision::Hold
                } else {
                    Decision::Deliver
                })
            },
            publish,
            record::nothing,
            stopped,
        )
    });
    until("not decided again", async || {
        asked.load(Ordering::SeqCst) >= 5
    })
    .await;
    assert_eq!(broker.len(), 0);
    assert_eq!(outbox.stats().await.unwrap().held, 1);
    standing.store(0, Ordering::SeqCst);
    until("not delivered once the hold was lifted", async || {
        state(&pool, &event).await == "delivered"
    })
    .await;
    relay.stop().await;
    assert_eq!(broker.event_ids(), [event.event_id]);
    assert_eq!(outcomes(&pool, &event).await, ["claimed", "delivered"]);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_hold_holds_back_only_its_key() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        hold_interval: Duration::from_mins(10),
        ..quick()
    })
    .await;
    let held = add(&pool, "1").await;
    let _behind = add(&pool, "1").await;
    let other = add(&pool, "2").await;
    let held_id = held.event_id;
    let decide = async move |event: &Claimed, _: &mut PgConnection| {
        Ok(if event.event_id == held_id {
            Decision::Hold
        } else {
            Decision::Deliver
        })
    };
    let batch = outbox.claim(decide, record::nothing).await.unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].event_id, other.event_id);
    deliver(&outbox, &pool, &batch).await;
    // The held event waits, and the one behind it on its key waits too.
    assert_eq!(take(&outbox).await.len(), 0);
}

// OBX-27: the decision comes before the publisher.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_refused_event_never_reaches_the_publisher() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let refused = add(&pool, "no").await;
    let allowed = add(&pool, "yes").await;
    let broker = Broker::default();
    let publish = taking(&broker);
    let relay = Relay::spawn(|stopped| outbox.clone().run(refuse_no, publish, audit, stopped));
    drained(&outbox).await;
    relay.stop().await;
    assert_eq!(broker.event_ids(), [allowed.event_id]);
    assert_eq!(outcomes(&pool, &refused).await, ["refused"]);
    // The decision ran in the claim's transaction, and could read and write in it.
    let audited: Vec<String> =
        sqlx::query_scalar("SELECT outcome FROM axa_outbox_test_audit ORDER BY attempt")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(audited, ["refused", "delivered"]);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_failed_decision_leaves_the_event_as_it_was() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        hold_interval: Duration::from_mins(10),
        ..quick()
    })
    .await;
    let broken = add(&pool, "1").await;
    let fine = add(&pool, "2").await;
    let broken_id = broken.event_id;
    let decide = async move |event: &Claimed, tx: &mut PgConnection| {
        if event.event_id == broken_id {
            // A query that fails aborts the savepoint, and only that.
            sqlx::query("SELECT * FROM no_such_table")
                .execute(&mut *tx)
                .await?;
        }
        Ok(Decision::Deliver)
    };
    let batch = outbox.claim(decide, record::nothing).await.unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].event_id, fine.event_id);
    assert_eq!(outcomes(&pool, &broken).await, Vec::<String>::new());
    // It is not decided again at once.
    assert_eq!(take(&outbox).await.len(), 0);
}

// OBX-28: an outcome commits with its audit.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_outcome_and_its_audit_commit_together() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(OutboxOptions {
        max_attempts: 2,
        ..quick()
    })
    .await;
    let delivered = add(&pool, "1").await;
    let rejected = add(&pool, "2").await;
    let rejected_id = rejected.event_id;
    let relay = Relay::spawn(|stopped| {
        outbox.clone().run(
            decide::deliver_all,
            move |event| async move {
                if event.event_id == rejected_id {
                    return Err(Failure::rejected("no"));
                }
                Ok(())
            },
            audit,
            stopped,
        )
    });
    drained(&outbox).await;
    relay.stop().await;
    // Every outcome has its audit row, naming the same attempt.
    let unaudited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM axa_outbox_attempts a
         WHERE a.outcome NOT IN ('claimed', 'returned')
           AND NOT EXISTS (SELECT 1 FROM axa_outbox_test_audit t
                           WHERE t.attempt = a.id AND t.outcome = a.outcome)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unaudited, 0);
    assert_eq!(count(&pool, "axa_outbox_test_audit").await, 3);
    assert_eq!(state(&pool, &delivered).await, "delivered");
    assert_eq!(state(&pool, &rejected).await, "failed");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_failed_record_rolls_back_the_outcome() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let behind = add(&pool, "2").await;
    let published = Arc::new(AtomicUsize::new(0));
    let told = published.clone();
    let relay = Relay::spawn(|stopped| {
        outbox.clone().run(
            decide::deliver_all,
            move |_| {
                told.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(()))
            },
            // The audit row is written, and then the audit fails.
            async |attempt: &Attempt, tx: &mut PgConnection| {
                audit(attempt, tx).await?;
                Err(sqlx::Error::Protocol("the audit is down".into()))
            },
            stopped,
        )
    });
    until("never published", async || {
        published.load(Ordering::SeqCst) >= 1
    })
    .await;
    relay.stop().await;
    // Neither the outcome nor the audit row is there, and the event stays claimed, to
    // be recovered. The rest of the batch went back.
    assert_eq!(outcomes(&pool, &event).await, ["claimed"]);
    assert_eq!(count(&pool, "axa_outbox_test_audit").await, 0);
    assert_eq!(outbox.stats().await.unwrap().claimed, 1);
    assert_eq!(outcomes(&pool, &behind).await, ["claimed", "returned"]);
    // The same in a service's own loop: the outcome and the audit roll back together.
    go_quiet(&pool).await;
    let batch = take(&outbox).await;
    let mut transaction = pool.begin().await.unwrap();
    let written = outbox.delivered(&mut transaction, &batch).await.unwrap();
    for attempt in &written {
        audit(attempt, &mut transaction).await.unwrap();
    }
    transaction.rollback().await.unwrap();
    assert_eq!(state(&pool, &event).await, "claimed");
    assert_eq!(count(&pool, "axa_outbox_test_audit").await, 0);
}

// OBX-30: nothing is swept, and an erasure is a recorded act.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn nothing_is_swept() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let broker = Broker::default();
    let relay = Relay::start(&outbox, taking(&broker));
    drained(&outbox).await;
    // Many empty rounds later, the delivered event and its payload are all still there.
    for _ in 0..20 {
        take(&outbox).await;
    }
    relay.stop().await;
    assert_eq!(count(&pool, "axa_outbox_events").await, 1);
    assert_eq!(count(&pool, "axa_outbox_payloads").await, 1);
    assert_eq!(outcomes(&pool, &event).await, ["claimed", "delivered"]);
    // And the code has no way to: no statement here deletes from the record.
    for source in [
        include_str!("../src/lib.rs"),
        include_str!("../src/relay.rs"),
    ] {
        for table in [
            "DELETE FROM axa_outbox_events",
            "DELETE FROM axa_outbox_attempts",
            "DELETE FROM axa_outbox_payloads",
        ] {
            assert!(!source.contains(table), "{table}");
        }
    }
}

async fn payload_of(pool: &PgPool, event: &Event) -> (Option<serde_json::Value>, Option<Vec<u8>>) {
    sqlx::query_as("SELECT payload, sealed FROM axa_outbox_payloads WHERE event_id = $1")
        .bind(event.event_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn fingerprint_of(pool: &PgPool, event: &Event) -> String {
    sqlx::query_scalar("SELECT fingerprint FROM axa_outbox_events WHERE event_id = $1")
        .bind(event.event_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_erasure_blanks_only_the_payload_and_is_kept() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let batch = take(&outbox).await;
    deliver(&outbox, &pool, &batch).await;
    let before: (String, String, String, String) = sqlx::query_as(
        "SELECT topic, kind, key, fingerprint FROM axa_outbox_events WHERE event_id = $1",
    )
    .bind(event.event_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    // An erasure rolled back with its transaction changes nothing.
    let mut transaction = pool.begin().await.unwrap();
    assert!(
        erase(
            &mut transaction,
            event.event_id,
            "dpo@example.com",
            "erasure_request"
        )
        .await
        .unwrap()
    );
    transaction.rollback().await.unwrap();
    assert!(payload_of(&pool, &event).await.0.is_some());

    let mut transaction = pool.begin().await.unwrap();
    assert!(
        erase(
            &mut transaction,
            event.event_id,
            "dpo@example.com",
            "erasure_request"
        )
        .await
        .unwrap()
    );
    transaction.commit().await.unwrap();
    assert_eq!(payload_of(&pool, &event).await, (None, None));
    let after: (String, String, String, String) = sqlx::query_as(
        "SELECT topic, kind, key, fingerprint FROM axa_outbox_events WHERE event_id = $1",
    )
    .bind(event.event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        outcomes(&pool, &event).await,
        ["claimed", "delivered", "erased"]
    );
    let (by, reason): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT by, reason FROM axa_outbox_attempts WHERE event_id = $1 AND outcome = 'erased'",
    )
    .bind(event.event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(by.as_deref(), Some("dpo@example.com"));
    assert_eq!(reason.as_deref(), Some("erasure_request"));
    // Once is enough, and nothing more is written.
    let mut transaction = pool.begin().await.unwrap();
    assert!(
        !erase(&mut transaction, event.event_id, "dpo@example.com", "again")
            .await
            .unwrap()
    );
    transaction.commit().await.unwrap();
    assert_eq!(outcomes(&pool, &event).await.len(), 3);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn nothing_else_can_blank_a_payload() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    let event = add(&pool, "1").await;
    let other = add(&pool, "2").await;
    let try_change = async |change: &str| {
        sqlx::query(sqlx::AssertSqlSafe(change.to_string()))
            .bind(event.event_id)
            .bind(other.event_id)
            .execute(&pool)
            .await
    };
    // No erased attempt yet.
    let blank = "UPDATE axa_outbox_payloads SET payload = NULL
                 WHERE event_id = $1 AND $2::uuid IS NOT NULL";
    assert!(try_change(blank).await.is_err());
    // An erased attempt makes blanking possible, and nothing else.
    sqlx::query(
        "INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim, reason, by)
         VALUES ($1, 'erased', gen_random_uuid(), gen_random_uuid(), 'test', 'test')",
    )
    .bind(event.event_id)
    .execute(&pool)
    .await
    .unwrap();
    for change in [
        "UPDATE axa_outbox_payloads SET payload = '{}' WHERE event_id = $1 AND $2::uuid IS NOT NULL",
        "UPDATE axa_outbox_payloads SET payload = NULL, sealed = '\\x00'
         WHERE event_id = $1 AND $2::uuid IS NOT NULL",
        "UPDATE axa_outbox_payloads SET payload = NULL, event_id = $2 WHERE event_id = $1",
        // Another event's payload has no erased attempt.
        "UPDATE axa_outbox_payloads SET payload = NULL WHERE event_id = $2 AND $1::uuid IS NOT NULL",
        "DELETE FROM axa_outbox_payloads WHERE event_id = $1 AND $2::uuid IS NOT NULL",
    ] {
        assert!(try_change(change).await.is_err(), "{change}");
    }
    assert!(payload_of(&pool, &event).await.0.is_some());
    assert!(try_change(blank).await.is_ok());
    // A payload already blank cannot be changed again, even to blank.
    assert!(try_change(blank).await.is_err());
    assert!(payload_of(&pool, &other).await.0.is_some());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_erased_event_is_not_published_and_is_not_tampered() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let erased = add(&pool, "1").await;
    let behind = add(&pool, "1").await;
    let fingerprint = fingerprint_of(&pool, &erased).await;
    let mut transaction = pool.begin().await.unwrap();
    erase(
        &mut transaction,
        erased.event_id,
        "dpo@example.com",
        "erasure_request",
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let broker = Broker::default();
    let relay = Relay::start(&outbox, taking(&broker));
    let stats = drained(&outbox).await;
    relay.stop().await;
    // The erased event is no longer owed: it is never claimed, never checked, and
    // frees its key. Its fingerprint stays, as the record of what was there.
    assert_eq!(broker.event_ids(), [behind.event_id]);
    assert_eq!(outcomes(&pool, &erased).await, ["erased"]);
    assert_eq!((stats.failed, stats.tampered), (0, 0));
    assert_eq!(fingerprint_of(&pool, &erased).await, fingerprint);
    // A release cannot bring it back.
    assert!(!release_as(&pool, &erased, "ops@example.com").await);
}

// OBX-31: a changed row is never published.

/// Change a row as only someone who turned the triggers off could.
async fn tamper(pool: &PgPool, table: &str, trigger: &str, change: &str, event: &Event) {
    let mut transaction = pool.begin().await.unwrap();
    for statement in [
        format!("ALTER TABLE {table} DISABLE TRIGGER {trigger}"),
        change.to_string(),
        format!("ALTER TABLE {table} ENABLE ALWAYS TRIGGER {trigger}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .bind(event.event_id)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_changed_row_is_never_published() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    let changed_payload = add(&pool, "1").await;
    let changed_topic = add(&pool, "2").await;
    let blanked = add(&pool, "3").await;
    let untouched = add(&pool, "4").await;
    tamper(
        &pool,
        "axa_outbox_payloads",
        "axa_outbox_payloads_guard",
        "UPDATE axa_outbox_payloads SET payload = '{\"key\": \"other\"}' WHERE event_id = $1",
        &changed_payload,
    )
    .await;
    tamper(
        &pool,
        "axa_outbox_events",
        "axa_outbox_events_refuse",
        "UPDATE axa_outbox_events SET topic = 'place' WHERE event_id = $1",
        &changed_topic,
    )
    .await;
    tamper(
        &pool,
        "axa_outbox_payloads",
        "axa_outbox_payloads_guard",
        "UPDATE axa_outbox_payloads SET payload = NULL WHERE event_id = $1",
        &blanked,
    )
    .await;
    let broker = Broker::default();
    let publish = taking(&broker);
    let relay = Relay::spawn(|stopped| {
        outbox
            .clone()
            .run(decide::deliver_all, publish, audit, stopped)
    });
    let stats = drained(&outbox).await;
    relay.stop().await;
    assert_eq!(broker.event_ids(), [untouched.event_id]);
    for event in [&changed_payload, &changed_topic, &blanked] {
        assert_eq!(
            latest(&pool, event).await,
            Some(("failed".to_string(), Some("tampered".to_string())))
        );
    }
    assert_eq!((stats.failed, stats.tampered), (3, 3));
    // Each was recorded, with its audit.
    assert_eq!(count(&pool, "axa_outbox_test_audit").await, 4);
}

// OBX-33 and OBX-35: what a payload may be.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_large_payload_fails_the_write() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    let large = Event::new(
        "person",
        "updated",
        "1",
        json!({ "photo": "x".repeat(64 * 1024) }),
    );
    let mut transaction = pool.begin().await.unwrap();
    let error = write(&mut transaction, &large).await.unwrap_err();
    assert!(matches!(&error, sqlx::Error::Encode(inner)
        if matches!(inner.downcast_ref::<Refused>(), Some(Refused::TooLarge { limit: 65536, .. }))));
    transaction.rollback().await.unwrap();
    assert_eq!(count(&pool, "axa_outbox_events").await, 0);
    // A service may raise the limit, knowing what it stores.
    let writer = Writer::new(WriteOptions {
        max_payload_bytes: 128 * 1024,
        ..WriteOptions::default()
    })
    .unwrap();
    let mut transaction = pool.begin().await.unwrap();
    writer.write(&mut transaction, &large).await.unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(count(&pool, "axa_outbox_events").await, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_denied_field_fails_the_write() {
    let _table = TABLE.lock().await;
    let (_outbox, pool) = fresh(quick()).await;
    let writer = Writer::new(WriteOptions {
        deny: vec!["nhs_number".to_string()],
        ..WriteOptions::default()
    })
    .unwrap();
    let event = Event::new(
        "person",
        "updated",
        "1",
        json!({ "patient": { "ids": [{ "nhs_number": "943 476 5919" }] } }),
    );
    let mut transaction = pool.begin().await.unwrap();
    let error = writer.write(&mut transaction, &event).await.unwrap_err();
    assert!(error.to_string().contains("nhs_number"), "{error}");
    assert!(!error.to_string().contains("943"), "{error}");
    transaction.rollback().await.unwrap();
    assert_eq!(count(&pool, "axa_outbox_events").await, 0);
    // With no names, as by default, nothing is denied.
    let mut transaction = pool.begin().await.unwrap();
    write(&mut transaction, &event).await.unwrap();
    transaction.commit().await.unwrap();
}

// OBX-34: payloads sealed at rest.

#[cfg(feature = "encryption")]
mod sealed {
    use axa_transactional_outbox::{Key, Keys};

    use super::*;

    fn keys(id: u8) -> Keys {
        Keys::new(Key::new(id, &[id; 32]).unwrap())
    }

    async fn sealed_add(pool: &PgPool, writer: &Writer, key: &str) -> Event {
        let event = Event::new("person", "updated", key, json!({ "name": "Ada Lovelace" }));
        let mut transaction = pool.begin().await.unwrap();
        writer.write(&mut transaction, &event).await.unwrap();
        transaction.commit().await.unwrap();
        event
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_sealed_payload_is_stored_encrypted_and_published_plain() {
        let _table = TABLE.lock().await;
        let (outbox, pool) = fresh(quick()).await;
        let writer = Writer::default().sealing(keys(1));
        let event = sealed_add(&pool, &writer, "1").await;
        let (payload, sealed) = payload_of(&pool, &event).await;
        assert!(payload.is_none());
        let sealed = sealed.unwrap();
        assert!(!sealed.windows(3).any(|part| part == b"Ada"));
        let outbox = outbox.opening(keys(1));
        let broker = Broker::default();
        let relay = Relay::start(&outbox, taking(&broker));
        drained(&outbox).await;
        relay.stop().await;
        let arrived = broker.0.lock().unwrap().clone();
        assert_eq!(arrived.len(), 1);
        assert_eq!(arrived[0].payload, json!({ "name": "Ada Lovelace" }));
        // An erased sealed payload is blanked like a plain one.
        let mut transaction = pool.begin().await.unwrap();
        assert!(
            erase(
                &mut transaction,
                event.event_id,
                "dpo@example.com",
                "erasure_request"
            )
            .await
            .unwrap()
        );
        transaction.commit().await.unwrap();
        assert_eq!(payload_of(&pool, &event).await, (None, None));
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_relay_without_the_key_leaves_sealed_events_alone() {
        let _table = TABLE.lock().await;
        let (outbox, pool) = fresh(quick()).await;
        let writer = Writer::default().sealing(keys(1));
        let event = sealed_add(&pool, &writer, "1").await;
        // No key, or the wrong one: nothing is written, charged, or published.
        assert_eq!(take(&outbox).await.len(), 0);
        assert_eq!(take(&outbox.clone().opening(keys(2))).await.len(), 0);
        assert_eq!(outcomes(&pool, &event).await, Vec::<String>::new());
        assert_eq!(outbox.stats().await.unwrap().pending, 1);
        // Rotated: the old key still opens it.
        let rotated =
            Keys::new(Key::new(2, &[2; 32]).unwrap()).and_open_with(Key::new(1, &[1; 32]).unwrap());
        let batch = take(&outbox.opening(rotated)).await;
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].payload, json!({ "name": "Ada Lovelace" }));
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_sealed_payload_moved_to_another_event_is_tampered() {
        let _table = TABLE.lock().await;
        let (outbox, pool) = fresh(quick()).await;
        let writer = Writer::default().sealing(keys(1));
        let first = sealed_add(&pool, &writer, "1").await;
        let second = sealed_add(&pool, &writer, "2").await;
        // Moved, and the fingerprint made again, as someone with the schema could.
        let mut transaction = pool.begin().await.unwrap();
        for statement in [
            "ALTER TABLE axa_outbox_payloads DISABLE TRIGGER axa_outbox_payloads_guard",
            "ALTER TABLE axa_outbox_events DISABLE TRIGGER axa_outbox_events_refuse",
            "UPDATE axa_outbox_payloads SET sealed =
                 (SELECT sealed FROM axa_outbox_payloads WHERE event_id = $1)
             WHERE event_id = $2",
            "UPDATE axa_outbox_events e SET fingerprint = axa_outbox_fingerprint(
                 e.event_id, e.topic, e.kind, e.key, p.payload, p.sealed)
             FROM axa_outbox_payloads p WHERE p.event_id = e.event_id AND e.event_id = $2
               AND $1::uuid IS NOT NULL",
            "ALTER TABLE axa_outbox_payloads ENABLE ALWAYS TRIGGER axa_outbox_payloads_guard",
            "ALTER TABLE axa_outbox_events ENABLE ALWAYS TRIGGER axa_outbox_events_refuse",
        ] {
            sqlx::query(statement)
                .bind(first.event_id)
                .bind(second.event_id)
                .execute(&mut *transaction)
                .await
                .unwrap();
        }
        transaction.commit().await.unwrap();
        let outbox = outbox.opening(keys(1));
        let batch = take(&outbox).await;
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].event_id, first.event_id);
        assert_eq!(
            latest(&pool, &second).await,
            Some(("failed".to_string(), Some("tampered".to_string())))
        );
    }
}

// OBX-36: the consumer's inbox.

async fn fresh_inbox(pool: &PgPool) {
    sqlx::query("DROP TABLE IF EXISTS axa_inbox")
        .execute(pool)
        .await
        .unwrap();
    for statement in inbox::SETUP_STATEMENTS {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_inbox_says_what_was_seen() {
    let _table = TABLE.lock().await;
    let pool = admin().await;
    fresh_inbox(&pool).await;
    let event_id = Uuid::new_v4();
    let mut transaction = pool.begin().await.unwrap();
    assert!(!inbox::seen(&mut transaction, event_id).await.unwrap());
    transaction.commit().await.unwrap();
    let mut transaction = pool.begin().await.unwrap();
    assert!(inbox::seen(&mut transaction, event_id).await.unwrap());
    transaction.commit().await.unwrap();
    assert!(
        !inbox::seen(&mut pool.begin().await.unwrap(), Uuid::new_v4())
            .await
            .unwrap()
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_rolled_back_note_is_not_seen() {
    let _table = TABLE.lock().await;
    let pool = admin().await;
    fresh_inbox(&pool).await;
    let event_id = Uuid::new_v4();
    let mut transaction = pool.begin().await.unwrap();
    assert!(!inbox::seen(&mut transaction, event_id).await.unwrap());
    // Handling the event failed, so it was not applied.
    transaction.rollback().await.unwrap();
    let mut transaction = pool.begin().await.unwrap();
    assert!(!inbox::seen(&mut transaction, event_id).await.unwrap());
    transaction.commit().await.unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_inbox_refuses_every_change() {
    let _table = TABLE.lock().await;
    let pool = admin().await;
    fresh_inbox(&pool).await;
    let mut transaction = pool.begin().await.unwrap();
    inbox::seen(&mut transaction, Uuid::new_v4()).await.unwrap();
    transaction.commit().await.unwrap();
    for change in [
        "UPDATE axa_inbox SET seen_at = now()",
        "DELETE FROM axa_inbox",
        "TRUNCATE axa_inbox",
    ] {
        let error = sqlx::query(change)
            .execute(&pool)
            .await
            .expect_err(change)
            .to_string();
        assert!(error.contains("refused"), "{change}: {error}");
    }
    assert_eq!(count(&pool, "axa_inbox").await, 1);
}

// OBX-38: moving from 0.1.0.

/// The 0.1.0 table, as its `SETUP_STATEMENTS` made it.
const OLD_TABLE: &str = "CREATE TABLE axa_outbox (
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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_old_table_imports_with_its_states() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    sqlx::query("DROP TABLE IF EXISTS axa_outbox")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(OLD_TABLE).execute(&pool).await.unwrap();
    let mut old = HashMap::new();
    for (status, attempts) in [("sent", 1), ("failed", 10), ("pending", 2), ("claimed", 1)] {
        let event_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO axa_outbox (event_id, topic, kind, key, payload, status, attempts,
                 error, sent_at, created_at)
             VALUES ($1, 'person', 'updated', $2, '{\"id\": 7}', $2, $3, 'free text: Ada',
                 CASE WHEN $2 = 'sent' THEN now() END, now() - interval '1 day')",
        )
        .bind(event_id)
        .bind(status)
        .bind(attempts)
        .execute(&pool)
        .await
        .unwrap();
        old.insert(status, event_id);
    }
    // Safe to run twice.
    for _ in 0..2 {
        let mut transaction = pool.begin().await.unwrap();
        for statement in IMPORT_STATEMENTS {
            sqlx::query(statement)
                .execute(&mut *transaction)
                .await
                .unwrap();
        }
        transaction.commit().await.unwrap();
    }
    assert_eq!(count(&pool, "axa_outbox_events").await, 4);
    assert_eq!(count(&pool, "axa_outbox_attempts").await, 4);
    let imported = async |status: &str| -> (String, Option<String>) {
        sqlx::query_as("SELECT outcome, reason FROM axa_outbox_attempts WHERE event_id = $1")
            .bind(old[status])
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(
        imported("sent").await,
        (
            "delivered".to_string(),
            Some("imported: sent, 1 attempts".to_string())
        )
    );
    assert_eq!(imported("failed").await.0, "failed");
    assert_eq!(
        imported("pending").await,
        (
            "imported".to_string(),
            Some("imported: pending, 2 attempts".to_string())
        )
    );
    assert_eq!(imported("claimed").await.0, "imported");
    let errors: i64 =
        sqlx::query_scalar("SELECT count(*) FROM axa_outbox_attempts WHERE reason LIKE '%Ada%'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(errors, 0, "free error text was imported");
    // The owed ones go out, the old ones are intact, and the failed one waits.
    let broker = Broker::default();
    let relay = Relay::start(&outbox, taking(&broker));
    let stats = drained(&outbox).await;
    relay.stop().await;
    let sent: HashSet<Uuid> = broker.event_ids().into_iter().collect();
    assert_eq!(sent, HashSet::from([old["pending"], old["claimed"]]));
    assert_eq!((stats.failed, stats.tampered), (1, 0));
    sqlx::query("DROP TABLE axa_outbox")
        .execute(&pool)
        .await
        .unwrap();
}

// OBX-16: the size of the record.

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn what_is_owed_is_found_without_reading_delivered_events() {
    let _table = TABLE.lock().await;
    let (outbox, pool) = fresh(quick()).await;
    sqlx::query(
        "WITH e AS (
             INSERT INTO axa_outbox_events (event_id, topic, kind, key, fingerprint)
             SELECT gen_random_uuid(), 'person', 'updated', n::text, 'x'
             FROM generate_series(1, 20000) AS n
             RETURNING event_id)
         INSERT INTO axa_outbox_payloads (event_id, payload) SELECT event_id, '{}' FROM e",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO axa_outbox_attempts (event_id, outcome, relay, claim, at)
         SELECT event_id, 'delivered', gen_random_uuid(), gen_random_uuid(),
             now() - interval '2 days'
         FROM axa_outbox_events",
    )
    .execute(&pool)
    .await
    .unwrap();
    add(&pool, "new").await;
    assert_eq!(count(&pool, "axa_outbox_owed").await, 1);
    for table in [
        "axa_outbox_events",
        "axa_outbox_attempts",
        "axa_outbox_owed",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("ANALYZE {table}")))
            .execute(&pool)
            .await
            .unwrap();
    }
    let plan = async |query: &str| -> String {
        let lines: Vec<String> =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("EXPLAIN {query}")))
                .fetch_all(&pool)
                .await
                .unwrap();
        lines.join("\n")
    };
    // What `stats` reads for the last hour.
    let recent = plan(
        "SELECT count(*) FROM axa_outbox_attempts a
         WHERE a.outcome = 'delivered' AND a.at > now() - interval '1 hour'",
    )
    .await;
    assert!(recent.contains("axa_outbox_attempts_recent"), "{recent}");
    // What a claim reads: the owed, and each one's event by its id.
    let owed = plan(
        "SELECT e.topic FROM axa_outbox_owed o
         JOIN axa_outbox_events e ON e.event_id = o.event_id
         WHERE o.state = 'pending'",
    )
    .await;
    assert!(!owed.contains("Seq Scan on axa_outbox_events"), "{owed}");
    assert_eq!(take(&outbox).await.len(), 1);
    // Deliveries from days ago are not counted, nor read.
    assert_eq!(outbox.stats().await.unwrap().delivered, 0);
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
    assert_eq!(outcomes(&pool, &events[0]).await, ["claimed", "delivered"]);
    for event in &events[1..] {
        assert_eq!(outcomes(&pool, event).await, ["claimed", "returned"]);
    }
    assert_eq!(outbox.stats().await.unwrap().pending, 4);
    assert_eq!(count(&pool, "axa_outbox_leases").await, 0);
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
    let task = tokio::spawn(outbox.run(
        decide::deliver_all,
        taking(&broker),
        record::nothing,
        stopped,
    ));
    drop(stop);
    tokio::time::timeout(DEADLINE, task)
        .await
        .expect("spinning with no sender")
        .unwrap();
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

    // A broker client that batches: send them all, flush once, then record all three
    // with the service's audit rows in one transaction.
    let batch = take(&outbox).await;
    assert_eq!(batch.len(), 3);
    assert_eq!(outbox.heartbeat(&batch).await.unwrap(), 1);
    let mut transaction = pool.begin().await.unwrap();
    let written = outbox.delivered(&mut transaction, &batch).await.unwrap();
    assert_eq!(written.len(), 3);
    for attempt in &written {
        audit(attempt, &mut transaction).await.unwrap();
    }
    transaction.commit().await.unwrap();
    assert_eq!(count(&pool, "axa_outbox_test_audit").await, 3);

    let batch = take(&outbox).await;
    assert_eq!(batch[0].event_id, events[3].event_id);
    assert_eq!(batch[0].payload, json!({ "key": "3" }));
    assert_eq!(batch[0].topic, "person");
    assert_eq!(batch[0].kind, "updated");
    assert_eq!(give_back(&outbox, &pool, &batch).await, 1);
    assert_eq!(outcomes(&pool, &events[3]).await, ["claimed", "returned"]);
    assert_eq!(state(&pool, &events[0]).await, "delivered");
    assert!(matches!(outbox.ping().await, Ok(())));
}
