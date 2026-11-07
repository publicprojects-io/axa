//! What reaches a log, against a real Postgres. It has its own binary because `tracing`
//! caches per process whether a log site is on. Ignored by default: run it with
//! `AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-outbox -- --ignored`.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axa_outbox::{Event, Failure, Outbox, OutboxOptions, decide, record, write};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::watch;

fn outbox_options() -> OutboxOptions {
    OutboxOptions {
        max_attempts: 2,
        poll_interval: Duration::from_millis(10),
        retry_backoff: Duration::ZERO,
        ..OutboxOptions::default()
    }
}

/// Text a test can read, written by a `tracing` subscriber.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn no_log_line_holds_a_payload_a_key_or_a_failure_text() {
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
    let pool = PgPoolOptions::new().connect(&uri).await.unwrap();
    // The record refuses to be emptied, so this starts from new tables.
    sqlx::query(
        "DROP TABLE IF EXISTS axa_outbox_attempts, axa_outbox_payloads, axa_outbox_owed,
         axa_outbox_leases, axa_outbox_events CASCADE",
    )
    .execute(&pool)
    .await
    .unwrap();
    let outbox = Outbox::connect(&uri, outbox_options()).await.unwrap();

    let event = Event::new(
        "person",
        "updated",
        "ada@example.com",
        json!({ "name": "Ada Lovelace" }),
    );
    let mut transaction = pool.begin().await.unwrap();
    write(&mut transaction, &event).await.unwrap();
    transaction.commit().await.unwrap();

    let (stop, stopped) = watch::channel(false);
    let relay = tokio::spawn(outbox.clone().run(
        decide::deliver_all,
        // A publisher that wrongly quotes the payload in its reason.
        |event| async move { Err(Failure::rejected(format!("quoted {}", event.payload))) },
        record::nothing,
        stopped,
    ));
    let until = Instant::now() + Duration::from_secs(20);
    while outbox.stats().await.unwrap().failed != 1 {
        assert!(Instant::now() < until, "the row did not fail");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.send(true).unwrap();
    relay.await.unwrap();

    let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("failed for good"), "{log}");
    assert!(log.contains("will be retried"), "{log}");
    assert!(!log.contains("Ada"), "{log}");
    assert!(!log.contains("ada@example.com"), "{log}");
    assert!(!log.contains("quoted"), "{log}");
}
