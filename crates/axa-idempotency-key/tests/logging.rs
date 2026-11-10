//! What the layer writes to the log. In its own binary, as `tracing` caches per process
//! whether a log site is on.
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test -p axa-idempotency-key -- --ignored --test-threads=1
//! ```

use std::sync::{Arc, Mutex};

use axa_idempotency_key::{HEADER, Idempotency, idempotent, setup};
use axum::{
    Router, body::Body, extract::Request, http::header, middleware::from_fn_with_state,
    routing::post,
};
use sqlx::PgPool;
use tower::ServiceExt;

/// A log writer that can be read afterwards.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn keys(pool: &PgPool) -> Idempotency {
    Idempotency::new(pool.clone(), |_| Some("caller-ana".to_string()))
}

fn app(keys: Idempotency) -> Router {
    Router::new()
        .route(
            "/encoded",
            post(|| async { ([(header::CONTENT_ENCODING, "gzip")], "answer-text") }),
        )
        .layer(from_fn_with_state(keys, idempotent))
}

fn write() -> Request {
    Request::post("/encoded")
        .header(HEADER, "key-to-a-secret")
        .body(Body::from("body-text"))
        .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_log_line_holds_no_key_caller_or_body() {
    let url = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let pool = PgPool::connect(&url).await.expect("connect");
    setup(&pool).await.expect("setup");
    sqlx::query("TRUNCATE axa_idempotency")
        .execute(&pool)
        .await
        .expect("truncate");

    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    app(keys(&pool)).oneshot(write()).await.unwrap();
    let closed = PgPool::connect(&url).await.expect("connect");
    closed.close().await;
    app(keys(&closed)).oneshot(write()).await.unwrap();

    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("idempotency_answer_not_kept"), "{text}");
    assert!(text.contains("idempotency_store_failed"), "{text}");
    for secret in ["key-to-a-secret", "caller-ana", "body-text", "answer-text"] {
        assert!(!text.contains(secret), "{text}");
    }
}
