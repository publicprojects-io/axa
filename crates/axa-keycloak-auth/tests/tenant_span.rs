//! The tenant on the request span, which the guard records where the span has a
//! `tenant` field.
//!
//! This has its own test binary, so its own process. `tracing` caches per process
//! whether each log site is enabled, so a parallel test reaching these sites first,
//! with no subscriber, would turn them off here.

mod idp;

use std::sync::{Arc, Mutex};

use axa_keycloak_auth::{
    Guard, GuardConfig, KeycloakSettings, KeycloakVerifier, Policy, ReloadablePolicy, middleware,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header::AUTHORIZATION},
    middleware::{Next, from_fn, from_fn_with_state},
    response::Response,
    routing::get,
};
use idp::TestIdp;
use serde_json::json;
use tower::ServiceExt;
use tracing::Instrument;

fn guarded(idp: &TestIdp) -> Router {
    let settings = KeycloakSettings {
        url: idp.base_url.clone(),
        realm: "mxi".to_string(),
        audiences: vec!["mxi-api".to_string()],
        role_map: serde_json::from_value(json!({ "acme-member": { "tenant": ["acme"] } })).unwrap(),
        require_verified_email: true,
    };
    let guard = Guard::new(
        GuardConfig::new("place"),
        Arc::new(KeycloakVerifier::new(settings).unwrap()),
        Arc::new(ReloadablePolicy::new(Policy::default_policy())),
    );
    places().layer(from_fn_with_state(Arc::new(guard), middleware))
}

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

fn capture() -> Captured {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();
    captured
}

/// A request span as Axa makes one, with an empty `tenant` field when `log_tenant`
/// is on, as with `HTTP_LOG_TENANT`.
async fn request_span(log_tenant: bool, request: Request<Body>, next: Next) -> Response {
    let span = if log_tenant {
        tracing::info_span!("http-request", tenant = tracing::field::Empty)
    } else {
        tracing::info_span!("http-request")
    };
    next.run(request).instrument(span).await
}

async fn logged(app: Router, token: &str) -> StatusCode {
    let request = Request::builder()
        .uri("/api/places")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    app.oneshot(request).await.unwrap().status()
}

fn with_span(guarded: Router, log_tenant: bool) -> Router {
    guarded.layer(from_fn(move |request, next| {
        request_span(log_tenant, request, next)
    }))
}

fn places() -> Router {
    Router::new().route(
        "/api/places",
        get(|| async {
            tracing::info!("inside the handler");
            "ok"
        }),
    )
}

#[tokio::test]
async fn the_guard_records_the_tenant_only_where_the_span_asks() {
    let log = capture();
    let idp = TestIdp::start().await;
    let acme = idp.token(|claims| claims["realm_access"] = json!({ "roles": ["acme-member"] }));

    let app = guarded(&idp);
    assert_eq!(
        logged(with_span(app.clone(), true), &acme).await,
        StatusCode::OK
    );
    let lines = log.take();
    let line = lines
        .lines()
        .find(|l| l.contains("inside the handler"))
        .unwrap();
    assert!(line.contains(r#"tenant="acme""#), "{line}");

    assert_eq!(logged(with_span(app, false), &acme).await, StatusCode::OK);
    let lines = log.take();
    assert!(lines.contains("inside the handler"), "{lines}");
    assert!(!lines.contains("tenant="), "{lines}");
}
