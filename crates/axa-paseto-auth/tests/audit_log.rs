//! The audit line a refusal leaves.
//!
//! This has its own test binary, so its own process. `tracing` caches per process
//! whether each log site is enabled, so a parallel test reaching these sites first,
//! with no subscriber, would turn them off here.

use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};

use authentication_verifier::{Policy, ReloadablePolicy, ReloadableVerifier, Verifier};
use axa_paseto_auth::{Guard, GuardConfig, Tenant, middleware};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header::AUTHORIZATION},
    middleware::from_fn_with_state,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::SigningKey;
use rusty_paseto::core::{Footer, Key, Paseto, PasetoAsymmetricPrivateKey, Payload, Public, V4};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

/// Throwaway seed for test tokens. Not a secret.
const SEED: [u8; 32] = [17; 32];
const ISSUER: &str = "authentication-service";
const AUDIENCE: &str = "main-x-service";

fn kid() -> String {
    let public = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
    URL_SAFE_NO_PAD.encode(Sha256::digest(public))
}

/// Sign with `seed`, naming the real key, so that a wrong seed reaches the signature.
fn sign(seed: &[u8; 32]) -> String {
    let payload = json!({
        "sub": "11111111-1111-1111-1111-111111111111",
        "email": "alice@example.com", "name": "Alice",
        "iss": ISSUER, "aud": AUDIENCE,
        "exp": 11_700_000_000_i64, "iat": 1_700_000_000, "sid": "test-sid", "attrs": {},
    })
    .to_string();
    let key = Key::<64>::from(SigningKey::from_bytes(seed).to_keypair_bytes());
    let private = PasetoAsymmetricPrivateKey::<V4, Public>::from(&key);
    let footer = format!(r#"{{"kid":"{}"}}"#, kid());
    let mut builder = Paseto::<V4, Public>::builder();
    builder.set_payload(Payload::from(payload.as_str()));
    builder.set_footer(Footer::from(footer.as_str()));
    builder.try_sign(&private).unwrap()
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

async fn call(app: &Router, method: &str, token: Option<&str>) -> StatusCode {
    let mut request = Request::builder().method(method).uri("/api/places");
    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = request.body(Body::empty()).unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn a_refusal_leaves_one_audit_line_and_an_allowed_request_none() {
    let log = capture();
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
    let client: IpAddr = "203.0.113.9".parse().unwrap();
    let app = Router::new()
        .route(
            "/api/places",
            get(|| async { "ok" }).post(|| async { "ok" }),
        )
        .route(
            "/api/tenant",
            get(|tenant: Tenant| async move { tenant.to_string() }),
        )
        .layer(from_fn_with_state(Arc::new(guard), middleware))
        .layer(axum::Extension(client));
    let good = sign(&SEED);
    let bad = sign(&[9; 32]);

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
    assert!(line.contains("subject=\"11111111-1111"), "{line}");
    assert!(line.contains("reason="), "{line}");
    assert!(!line.contains("alice@example.com"), "{line}");

    // An allowed request leaves no such line.
    assert_eq!(call(&app, "GET", Some(&good)).await, StatusCode::OK);
    let line = log.take();
    assert!(!line.contains("auth_refused"), "{line}");

    // A token with no tenant, where a handler needs one, is refused with a line too.
    let request = Request::builder()
        .uri("/api/tenant")
        .header(AUTHORIZATION, format!("Bearer {good}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let line = log.take();
    assert!(line.contains("auth_refused"), "{line}");
    assert!(line.contains("reason=\"no_tenant\""), "{line}");
    assert!(line.contains("subject=\"11111111-1111"), "{line}");
    assert!(!line.contains("alice@example.com"), "{line}");

    // Turning enforcement off says so.
    let keys = Verifier::from_paseto_keys_value(&keys, ISSUER, AUDIENCE).unwrap();
    let _ = Guard::new(
        GuardConfig::new("place").require_auth(false),
        Arc::new(ReloadableVerifier::new(keys)),
        Arc::new(ReloadablePolicy::new(Policy::default_policy())),
    );
    let line = log.take();
    assert!(line.contains(" WARN "), "{line}");
    assert!(line.contains("authentication enforcement is off"), "{line}");
}
