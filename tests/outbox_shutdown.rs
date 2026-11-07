//! A relay under `server::run`, written as a service's `main` would write it, and a
//! real `SIGTERM`.
//!
//! The test runs twice. Run by `cargo test` it is the parent, and it starts this same
//! binary again with [`ROLE`] set as the child that serves, because a signal has to
//! reach a real process, whose exit a test can then read. These need a real Postgres,
//! so they are ignored by default.

#![cfg(all(unix, feature = "outbox"))]

use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use axa::{
    config::Server,
    outbox::{Event, Failure, Outbox, OutboxOptions, write},
    server,
};
use axum::{Router, routing::get};
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tokio::sync::watch;

/// Being set is what makes a process the child.
const ROLE: &str = "AXA_OUTBOX_SHUTDOWN_TEST_ROLE";
/// How long the child has to do anything, after which the test fails.
const DEADLINE: Duration = Duration::from_secs(20);
/// How long a publish takes, well past the settling time below.
const WORK: Duration = Duration::from_millis(1500);
/// `shutdown_signal` installs its handlers when first polled, and until then a
/// `SIGTERM` takes the default action. This is for the moment in between.
const SETTLE: Duration = Duration::from_millis(300);

fn uri() -> String {
    std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL")
}

fn config() -> Server {
    Server {
        port: 0,
        binding: "127.0.0.1".to_string(),
        host: String::new(),
        shutdown_timeout_seconds: 10,
        header_read_timeout_seconds: 0,
        keep_alive_timeout_seconds: 0,
        send_stall_timeout_seconds: 0,
        max_connections: 1,
    }
}

/// What a service's `main` does: start the relay, serve until told to stop, and then
/// stop the relay. Requests in flight can still write events until the server is done,
/// so the relay hears last.
async fn service() {
    let outbox = Outbox::connect_existing(&uri(), OutboxOptions::default())
        .await
        .unwrap();
    let (stop, stopped) = watch::channel(false);
    let relay = tokio::spawn(outbox.run(
        |_event| async {
            tokio::time::sleep(WORK).await;
            Ok::<(), Failure>(())
        },
        stopped,
    ));
    let router = Router::new().route("/", get(|| async { "ok" }));
    let served = server::run(Some(router), None, &config()).await;
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), relay)
        .await
        .expect("the relay did not stop")
        .unwrap();
    served.unwrap();
}

async fn status_of(pool: &PgPool, event: &Event) -> (String, i32) {
    let row = sqlx::query("SELECT status, attempts FROM axa_outbox WHERE event_id = $1")
        .bind(event.event_id)
        .fetch_one(pool)
        .await
        .unwrap();
    (row.get("status"), row.get("attempts"))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn sigterm_lets_a_publish_in_progress_finish_beside_a_server() {
    const TEST: &str = "sigterm_lets_a_publish_in_progress_finish_beside_a_server";
    if std::env::var_os(ROLE).is_some() {
        service().await;
        return;
    }
    // Only for its `connect`, which makes the table.
    Outbox::connect(&uri(), OutboxOptions::default())
        .await
        .unwrap();
    let pool = PgPoolOptions::new().connect(&uri()).await.unwrap();
    sqlx::query("DELETE FROM axa_outbox")
        .execute(&pool)
        .await
        .unwrap();
    let first = Event::new("person", "updated", "1", json!({}));
    let second = Event::new("person", "updated", "2", json!({}));
    let mut transaction = pool.begin().await.unwrap();
    write(&mut transaction, &first).await.unwrap();
    write(&mut transaction, &second).await.unwrap();
    transaction.commit().await.unwrap();

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([TEST, "--exact", "--nocapture", "--include-ignored"])
        .env(ROLE, "service")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + DEADLINE;
    while status_of(&pool, &first).await.0 != "claimed" {
        assert!(Instant::now() < until, "the relay claimed nothing");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(SETTLE).await;
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < until, "the child did not stop");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(status.success(), "{status:?}");
    // The publish in progress finished, and the row it had not reached went back.
    assert_eq!(status_of(&pool, &first).await, ("sent".to_string(), 1));
    assert_eq!(status_of(&pool, &second).await, ("pending".to_string(), 0));
}
