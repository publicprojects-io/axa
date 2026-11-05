//! Logging from the standard middleware stack.
//!
//! This has its own test binary, so its own process. `tracing` caches per
//! process whether each log site is enabled, so a parallel test reaching these
//! sites first, with no subscriber, would turn them off here.

use std::sync::{Arc, Mutex};

use axa::{config::Http, middleware};
use axum::{Router, body::Body, http::Request, routing::get};
use tower::ServiceExt;

/// A writer the test can read back.
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
async fn log_lines_carry_the_request_id_and_not_the_query() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();

    let router = Router::new().route(
        "/",
        get(|| async {
            tracing::info!("inside the handler");
            "ok"
        }),
    );
    let config = Http {
        body_limit: 1_000,
        compression: false,
        timeout_seconds: 0,
        timeout_overrides: Vec::new(),
        cors_origins: Vec::new(),
        hsts: false,
    };
    let request = Request::builder()
        .uri("/?email=alice@example.org")
        .header("x-request-id", "trace-me")
        .body(Body::empty())
        .unwrap();
    middleware::apply(router, &config)
        .unwrap()
        .oneshot(request)
        .await
        .unwrap();

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|line| line.contains("inside the handler"))
        .expect("the handler's line");
    assert!(line.contains("request_id=trace-me"), "{line}");
    assert!(line.contains("path=/"), "{line}");
    assert!(!logs.contains("alice"), "{logs}");
    // Each finished request is logged at info, so it appears in production.
    assert!(logs.contains("finished processing request"), "{logs}");
}
