#![allow(clippy::unwrap_used, reason = "RCA-SCOPE-4: tests may unwrap")]
//! Captured logs, and the outbox of a service's memory mailer.

use axa_test::logs;

#[tokio::test]
async fn logs_are_captured_for_the_test_and_a_secret_can_be_checked_absent() {
    let logs = logs::capture();
    tracing::info!(user = "ada", "sign-in refused");
    tracing::trace!("below the captured level");
    let lines = logs.lines();
    assert_eq!(lines.len(), 1, "{}", logs.text());
    assert_eq!(lines[0]["level"], "INFO");
    assert_eq!(lines[0]["fields"]["message"], "sign-in refused");
    assert_eq!(lines[0]["fields"]["user"], "ada");
    assert!(logs.contains("sign-in refused"));
    logs.assert_absent("hunter2");
    logs.clear();
    assert_eq!(logs.lines(), Vec::<serde_json::Value>::new());
}

#[tokio::test]
async fn a_request_through_the_test_server_is_captured() {
    let logs = logs::capture();
    let router = axum::Router::new().route(
        "/",
        axum::routing::get(|| async {
            tracing::warn!("handled");
            "ok"
        }),
    );
    let server = axa_test::TestServer::new(router);
    server.get("/").await.assert_status_ok();
    assert!(logs.contains("handled"), "{}", logs.text());
}

#[tokio::test]
#[should_panic(expected = "captured log line 1 holds what must not be logged")]
async fn a_secret_in_the_logs_fails_the_test_without_repeating_it() {
    let logs = logs::capture();
    tracing::info!(password = "hunter2", "oops");
    logs.assert_absent("hunter2");
}

#[tokio::test]
async fn capture_ends_when_the_logs_are_dropped() {
    let first = logs::capture();
    drop(first);
    let second = logs::capture();
    tracing::info!("only the second sees this");
    assert_eq!(second.lines().len(), 1);
}

#[cfg(feature = "mail")]
mod mail {
    use axa::mail::Message;
    use axa::store::Store;
    use axa_test::mail::{Mailer, outbox, sent_to};

    fn message(to: &str) -> Message {
        Message::builder()
            .from("accounts@example.com".parse().unwrap())
            .to(to.parse().unwrap())
            .subject("Your link")
            .body(String::from("Follow it to sign in."))
            .unwrap()
    }

    #[tokio::test]
    async fn the_outbox_of_the_services_memory_mailer_shows_what_it_sent() {
        let store = Store::default();
        store.insert(Mailer::from_url("memory:", false).unwrap());
        let mailer = store.get_ref::<Mailer>().unwrap();
        mailer.send(&message("ada@example.org")).await.unwrap();
        mailer.send(&message("bob@example.org")).await.unwrap();
        let outbox = outbox(&store);
        let to_ada = sent_to(&outbox, "ADA@example.org");
        assert_eq!(to_ada.len(), 1);
        assert!(to_ada[0].text().contains("Follow it to sign in."));
        assert_eq!(outbox.take().len(), 2);
    }

    #[test]
    #[should_panic(expected = "give it memory: in tests")]
    fn a_mailer_that_would_send_real_mail_fails_the_test() {
        let store = Store::default();
        let dir = std::env::temp_dir();
        store.insert(Mailer::file(dir));
        let _ = outbox(&store);
    }

    #[test]
    #[should_panic(expected = "keeps its Mailer in ctx.shared_store")]
    fn no_mailer_in_the_store_fails_the_test() {
        let _ = outbox(&Store::default());
    }
}
