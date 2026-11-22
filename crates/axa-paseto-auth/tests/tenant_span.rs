#![allow(clippy::unwrap_used, reason = "RCA-SCOPE-4: tests may unwrap")]
//! The tenant on the request span, which the guard records where the span has a
//! `tenant` field.
//!
//! This has its own test binary, so its own process. `tracing` caches per process
//! whether each log site is enabled, so a parallel test reaching these sites first,
//! with no subscriber, would turn them off here.

use std::sync::{Arc, Mutex};

use authentication_verifier::{Policy, ReloadablePolicy, ReloadableVerifier, Verifier};
use axa_paseto_auth::{Guard, GuardConfig, middleware};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header::AUTHORIZATION},
    middleware::{Next, from_fn, from_fn_with_state},
    response::Response,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::SigningKey;
use rusty_paseto::core::{Footer, Key, Paseto, PasetoAsymmetricPrivateKey, Payload, Public, V4};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use tracing::Instrument;

/// Throwaway seed for test tokens. Not a secret.
const SEED: [u8; 32] = [17; 32];
const ISSUER: &str = "authentication-service";
const AUDIENCE: &str = "main-x-service";

fn kid() -> String {
    let public = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
    URL_SAFE_NO_PAD.encode(Sha256::digest(public))
}

fn sign(attrs: &Value) -> String {
    let payload = json!({
        "sub": "11111111-1111-1111-1111-111111111111",
        "email": "alice@example.com", "name": "Alice",
        "iss": ISSUER, "aud": AUDIENCE,
        "exp": 11_700_000_000_i64, "iat": 1_700_000_000, "sid": "test-sid", "attrs": attrs,
    })
    .to_string();
    let key = Key::<64>::from(SigningKey::from_bytes(&SEED).to_keypair_bytes());
    let private = PasetoAsymmetricPrivateKey::<V4, Public>::from(&key);
    let footer = format!(r#"{{"kid":"{}"}}"#, kid());
    let mut builder = Paseto::<V4, Public>::builder();
    builder.set_payload(Payload::from(payload.as_str()));
    builder.set_footer(Footer::from(footer.as_str()));
    builder.try_sign(&private).unwrap()
}

fn guarded() -> Router {
    let public = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
    let keys = json!({
        "keys": [{ "kty": "OKP", "crv": "Ed25519", "use": "sig",
                   "kid": kid(), "x": URL_SAFE_NO_PAD.encode(public) }]
    });
    let verifier = Verifier::from_paseto_keys_value(&keys, ISSUER, AUDIENCE).unwrap();
    let guard = Guard::new(
        GuardConfig::new("place"),
        Arc::new(ReloadableVerifier::new(verifier)),
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
    let acme = sign(&json!({ "tenant": ["acme"] }));

    assert_eq!(
        logged(with_span(guarded(), true), &acme).await,
        StatusCode::OK
    );
    let line = log.take();
    let line = line
        .lines()
        .find(|l| l.contains("inside the handler"))
        .unwrap();
    assert!(line.contains(r#"tenant="acme""#), "{line}");

    assert_eq!(
        logged(with_span(guarded(), false), &acme).await,
        StatusCode::OK
    );
    let lines = log.take();
    assert!(lines.contains("inside the handler"), "{lines}");
    assert!(!lines.contains("acme"), "{lines}");

    // A token with no tenant leaves the field empty, and the request goes on.
    let none = sign(&json!({}));
    assert_eq!(
        logged(with_span(guarded(), true), &none).await,
        StatusCode::OK
    );
    let lines = log.take();
    assert!(lines.contains("inside the handler"), "{lines}");
    assert!(!lines.contains("tenant="), "{lines}");
}
