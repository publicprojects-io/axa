#![allow(clippy::unwrap_used, reason = "RCA-SCOPE-4: tests may unwrap")]
//! The audit line a refusal leaves.
//!
//! This has its own test binary, so its own process. `tracing` caches per process
//! whether each log site is enabled, so a parallel test reaching these sites first,
//! with no subscriber, would turn them off here.

mod idp;

use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};

use axa_keycloak_auth::{
    Guard, GuardConfig, KeycloakSettings, KeycloakVerifier, Policy, ReloadablePolicy, Tenant,
    middleware,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header::AUTHORIZATION},
    middleware::from_fn_with_state,
    routing::get,
};
use idp::TestIdp;
use serde_json::json;
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

fn with_client(app: Router) -> Router {
    let client: IpAddr = "203.0.113.9".parse().unwrap();
    app.layer(axum::Extension(client))
}

async fn call(app: &Router, method: &str, token: Option<&str>) -> StatusCode {
    send(app, method, "/api/places", token).await
}

async fn call_path(app: &Router, path: &str, token: Option<&str>) -> StatusCode {
    send(app, "GET", path, token).await
}

async fn send(app: &Router, method: &str, path: &str, token: Option<&str>) -> StatusCode {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = request.body(Body::empty()).unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn a_refusal_leaves_one_audit_line_and_an_allowed_request_none() {
    let log = capture();
    let idp = TestIdp::start().await;
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
    let app = with_client(
        Router::new()
            .route(
                "/api/places",
                get(|| async { "ok" }).post(|| async { "ok" }),
            )
            .route(
                "/api/tenant",
                get(|tenant: Tenant| async move { tenant.to_string() }),
            )
            .layer(from_fn_with_state(Arc::new(guard), middleware)),
    );
    let good = idp.token(|_| {});
    let bad = idp.token(|c| drop(c.as_object_mut().unwrap().remove("aud")));

    // The realm's keys load in the background, so wait for a token to pass.
    for _ in 0_i32..200_i32 {
        if call(&app, "GET", Some(&good)).await == StatusCode::OK {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    log.take();

    // A refused token: the line says so, at a level production keeps, and never
    // holds the token.
    assert_eq!(
        call(&app, "GET", Some(&bad)).await,
        StatusCode::UNAUTHORIZED
    );
    let line = log.take();
    assert!(line.contains(" WARN "), "{line}");
    assert!(line.contains("auth_refused"), "{line}");
    assert!(line.contains("status=401"), "{line}");
    assert!(line.contains("client=\"203.0.113.9\""), "{line}");
    assert!(!line.contains(&bad), "{line}");

    // A policy refusal names who, and why, and not their email.
    assert_eq!(call(&app, "POST", Some(&good)).await, StatusCode::FORBIDDEN);
    let line = log.take();
    assert!(line.contains("status=403"), "{line}");
    assert!(line.contains("action=Write"), "{line}");
    assert!(line.contains("subject=\"0c4f1e2a"), "{line}");
    assert!(line.contains("reason="), "{line}");
    assert!(!line.contains("ada@example.com"), "{line}");

    // An allowed request leaves no such line.
    assert_eq!(call(&app, "GET", Some(&good)).await, StatusCode::OK);
    let line = log.take();
    assert!(!line.contains("auth_refused"), "{line}");

    // A caller with no tenant, refused by the extractor after the guard let them in:
    // the line names who and why. A caller with one leaves none.
    assert_eq!(
        call_path(&app, "/api/tenant", Some(&good)).await,
        StatusCode::FORBIDDEN
    );
    let line = log.take();
    assert!(line.contains("auth_refused"), "{line}");
    assert!(line.contains("status=403"), "{line}");
    assert!(line.contains("reason=\"no_tenant\""), "{line}");
    assert!(line.contains("subject=\"0c4f1e2a"), "{line}");
    let member = idp.token(|claims| claims["realm_access"] = json!({ "roles": ["acme-member"] }));
    assert_eq!(
        call_path(&app, "/api/tenant", Some(&member)).await,
        StatusCode::OK
    );
    let line = log.take();
    assert!(!line.contains("auth_refused"), "{line}");
}
