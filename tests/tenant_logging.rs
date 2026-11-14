//! The tenant on the request span, with `log_tenant`.
//!
//! This has its own test binary, so its own process. `tracing` caches per process
//! whether each log site is enabled, so a parallel test reaching these sites first,
//! with no subscriber, would turn them off here.

use std::sync::{Arc, Mutex};

use axa::{config::Http, middleware};
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::StatusCode,
    middleware::{Next, from_fn},
    routing::get,
};
use tower::ServiceExt;

/// A writer the test can read back.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    /// What was written since the last call.
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn http(log_tenant: bool) -> Http {
    Http {
        body_limit: 1_000,
        compression: false,
        timeout_seconds: 0,
        timeout_overrides: Vec::new(),
        cross_site_paths: Vec::new(),
        cors_origins: Vec::new(),
        hsts: false,
        hsts_max_age: 0,
        csp: None,
        permissions_policy: None,
        log_tenant,
        trusted_proxies: Vec::new(),
    }
}

/// What an auth crate's guard does once it has verified a caller of tenant `acme`.
async fn guard_stand_in(request: Request, next: Next) -> axum::response::Response {
    tracing::Span::current().record("tenant", "acme");
    next.run(request).await
}

async fn handler_line(log_tenant: bool, captured: &Captured) -> String {
    let router = Router::new()
        .route(
            "/",
            get(|| async {
                tracing::info!("inside the handler");
                "ok"
            }),
        )
        .layer(from_fn(guard_stand_in));
    let app = middleware::apply(router, &http(log_tenant)).unwrap();
    let request = Request::get("/").body(Body::empty()).unwrap();
    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
    let logs = captured.take();
    logs.lines()
        .find(|line| line.contains("inside the handler"))
        .expect("the handler's line")
        .to_string()
}

#[tokio::test]
async fn the_tenant_is_on_the_span_only_when_asked_for() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();

    let line = handler_line(true, &captured).await;
    assert!(line.contains(r#"tenant="acme""#), "{line}");
    let line = handler_line(false, &captured).await;
    assert!(!line.contains("acme"), "{line}");
}
