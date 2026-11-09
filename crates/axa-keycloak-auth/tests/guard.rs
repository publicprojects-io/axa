//! A real token through the real guard. A local OIDC provider stands in for
//! Keycloak, so signature, issuer, and audience checks all run.

mod idp;

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axa_keycloak_auth::{
    Allowed, AuthUser, Claims, Guard, GuardConfig, KeycloakSettings, KeycloakVerifier,
    PasetoVerifier, Policy, ReloadablePolicy, VerifyError, middleware,
};
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode, header::AUTHORIZATION},
    middleware::from_fn_with_state,
    routing::{delete, get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use idp::TestIdp;
use serde_json::{Value, json};
use tower::ServiceExt;

/// The one token [`StubPaseto`] accepts.
const GOOD_PASETO: &str = "v4.public.good";

fn settings(idp: &TestIdp) -> KeycloakSettings {
    KeycloakSettings {
        url: idp.base_url.clone(),
        realm: "mxi".to_string(),
        audiences: vec!["mxi-api".to_string()],
        role_map: serde_json::from_value(json!({
            "editor": { "access": ["write"] },
            "root": { "access": ["admin"] },
        }))
        .unwrap(),
        require_verified_email: true,
    }
}

fn config(require_auth: bool) -> GuardConfig {
    GuardConfig::new("place")
        .require_auth(require_auth)
        .destructive_post_suffix("/merge")
}

fn policy() -> Arc<ReloadablePolicy> {
    Arc::new(ReloadablePolicy::new(Policy::default_policy()))
}

fn guard(settings: KeycloakSettings, require_auth: bool) -> Guard {
    let verifier = KeycloakVerifier::new(settings).unwrap();
    Guard::new(config(require_auth), Arc::new(verifier), policy())
}

async fn whoami(AuthUser(claims): AuthUser) -> Json<Value> {
    Json(json!({ "sub": claims.sub, "email": claims.email, "attrs": claims.attrs }))
}

fn app(guard: Guard) -> Router {
    Router::new()
        .route("/_health", get(|| async { "ok" }))
        .route(
            "/api/places",
            get(|| async { "ok" }).post(|| async { "ok" }),
        )
        .route("/api/places/merge", post(|| async { "ok" }))
        .route("/api/places/1", delete(|| async { "ok" }))
        .route("/api/whoami", get(whoami))
        .layer(from_fn_with_state(Arc::new(guard), middleware))
}

async fn send(app: &Router, method: &str, path: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = request.body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn status(app: &Router, method: &str, path: &str, token: Option<&str>) -> StatusCode {
    send(app, method, path, token).await.0
}

fn with_roles(idp: &TestIdp, roles: &[&str]) -> String {
    idp.token(|claims| claims["realm_access"] = json!({ "roles": roles }))
}

/// Stands in for a service's PASETO verifier.
struct StubPaseto;

impl PasetoVerifier for StubPaseto {
    fn verify(&self, token: &str) -> Result<Claims, VerifyError> {
        if token != GOOD_PASETO {
            return Err(VerifyError::Claim("bad signature".to_string()));
        }
        Ok(serde_json::from_value(json!({
            "sub": "11111111-1111-1111-1111-111111111111",
            "email": "alice@example.com", "name": "Alice",
            "iss": "authentication-service", "aud": "main-x-service",
            "exp": 11_700_000_000_i64, "iat": 1_700_000_000_i64,
            "sid": "test-sid", "attrs": { "access": ["write"] },
        }))
        .unwrap())
    }
}

// Authentication.

#[tokio::test]
async fn a_valid_token_reaches_the_handler_with_its_claims() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let token = idp.token(|_| {});
    let (code, body) = send(&app, "GET", "/api/whoami", Some(&token)).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["email"], "ada@example.com");
    assert_eq!(body["attrs"]["access"], json!(["write"]));
}

#[tokio::test]
async fn health_stays_public_and_everything_else_needs_a_token() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    assert_eq!(status(&app, "GET", "/_health", None).await, StatusCode::OK);
    assert_eq!(
        status(&app, "GET", "/api/places", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&app, "GET", "/api/anything-new", None).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn bad_tokens_are_401s() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let cases = [
        ("expired", idp.token(|c| c["exp"] = json!(1_000_000_000))),
        (
            "wrong audience",
            idp.token(|c| c["aud"] = json!(["other-client"])),
        ),
        (
            "wrong issuer",
            idp.token(|c| c["iss"] = json!("https://evil.example/realms/mxi")),
        ),
        (
            "unverified email",
            idp.token(|c| c["email_verified"] = json!(false)),
        ),
        ("garbage", "garbage".to_string()),
    ];
    for (name, token) in cases {
        assert_eq!(
            status(&app, "GET", "/api/places", Some(&token)).await,
            StatusCode::UNAUTHORIZED,
            "{name}"
        );
    }
}

#[tokio::test]
async fn only_a_current_access_token_for_this_audience_passes() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let an_hour_ahead = json!(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600
    );
    let refused = [
        (
            "no audience",
            idp.token(|c| drop(c.as_object_mut().unwrap().remove("aud"))),
        ),
        (
            "no audience, issued to another client",
            idp.token(|c| {
                c.as_object_mut().unwrap().remove("aud");
                c["azp"] = json!("other-client");
            }),
        ),
        ("ID token", idp.token(|c| c["typ"] = json!("ID"))),
        ("refresh token", idp.token(|c| c["typ"] = json!("Refresh"))),
        ("nbf ahead", idp.token(|c| c["nbf"] = an_hour_ahead.clone())),
        ("iat ahead", idp.token(|c| c["iat"] = an_hour_ahead.clone())),
    ];
    for (name, token) in refused {
        assert_eq!(
            status(&app, "GET", "/api/places", Some(&token)).await,
            StatusCode::UNAUTHORIZED,
            "{name}"
        );
    }

    // The checks refuse the token, not everything that carries one of these claims.
    let token = idp.token(|c| c["nbf"] = json!(1_000_000_000));
    assert_eq!(
        status(&app, "GET", "/api/places", Some(&token)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_changed_payload_fails_the_signature_check() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let good = idp.token(|_| {});
    let other = with_roles(&idp, &["root"]);
    let good: Vec<&str> = good.split('.').collect();
    let other: Vec<&str> = other.split('.').collect();
    let forged = format!("{}.{}.{}", good[0], other[1], good[2]);
    assert_eq!(
        status(&app, "GET", "/api/places", Some(&forged)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_unsigned_token_is_a_401() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let token = with_roles(&idp, &["root"]);
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload = token.split('.').nth(1).unwrap();
    let unsigned = format!("{header}.{payload}.");
    assert_eq!(
        status(&app, "GET", "/api/places", Some(&unsigned)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_refusal_tells_the_caller_nothing_about_why() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let wrong_issuer = idp.token(|c| c["iss"] = json!("https://evil.example/realms/mxi"));
    for token in [wrong_issuer.as_str(), "garbage", "v4.public.garbage"] {
        let (code, body) = send(&app, "GET", "/api/places", Some(token)).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
        assert_eq!(
            body,
            r#"{"type":"about:blank","title":"Unauthorized","status":401,"detail":"invalid token","code":"unauthorized"}"#
        );
    }
}

#[tokio::test]
async fn a_forbidden_caller_is_not_told_which_rule_refused_them() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let token = with_roles(&idp, &["editor"]);
    let (code, body) = send(&app, "DELETE", "/api/places/1", Some(&token)).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(
        body,
        r#"{"type":"about:blank","title":"Forbidden","status":403,"detail":"forbidden","code":"forbidden"}"#
    );
}

#[tokio::test]
async fn an_unreachable_realm_refuses_every_token() {
    let idp = TestIdp::start().await;
    let token = idp.token(|_| {});
    let mut settings = settings(&idp);
    settings.url = "http://127.0.0.1:1".to_string();
    let app = app(guard(settings, true));
    let refused = tokio::time::timeout(
        Duration::from_secs(8),
        status(&app, "GET", "/api/places", Some(&token)),
    )
    .await
    .expect("the guard must not hang on an unreachable realm");
    assert_eq!(refused, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ready_says_whether_tokens_can_be_verified() {
    let idp = TestIdp::start().await;
    let verifier = KeycloakVerifier::new(settings(&idp)).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while verifier.ready().is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("keys arrive from a reachable realm");

    let mut unreachable = settings(&idp);
    unreachable.url = "http://127.0.0.1:1".to_string();
    let verifier = KeycloakVerifier::new(unreachable).unwrap();
    assert!(verifier.ready().is_err());
}

// Obligations.

fn with_obligation(idp: &TestIdp) -> Guard {
    let policy = Policy::from_json(
        r#"{ "rules": [{ "effect": "allow", "actions": ["read"], "obligations": ["audit"] }] }"#,
    )
    .unwrap();
    Guard::new(
        config(true),
        Arc::new(KeycloakVerifier::new(settings(idp)).unwrap()),
        Arc::new(ReloadablePolicy::new(policy)),
    )
}

fn obligation_app(guard: Guard) -> Router {
    Router::new()
        .route(
            "/api/places",
            get(|Allowed { decision, .. }: Allowed| async move {
                decision.requires("audit").to_string()
            }),
        )
        .layer(from_fn_with_state(Arc::new(guard), middleware))
}

#[tokio::test]
async fn an_obligation_nobody_carries_out_is_refused() {
    let idp = TestIdp::start().await;
    let token = idp.token(|_| {});
    let app = obligation_app(with_obligation(&idp));
    assert_eq!(
        status(&app, "GET", "/api/places", Some(&token)).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn a_handler_reads_the_obligations_the_service_declared() {
    let idp = TestIdp::start().await;
    let token = idp.token(|_| {});
    let app = obligation_app(with_obligation(&idp).honouring(["audit"]));
    let (code, body) = send(&app, "GET", "/api/places", Some(&token)).await;
    assert_eq!((code, body.as_str()), (StatusCode::OK, "true"));
}

#[tokio::test]
async fn allowed_is_refused_where_nothing_was_decided() {
    let idp = TestIdp::start().await;
    let app = obligation_app(guard(settings(&idp), false));
    assert_eq!(
        status(&app, "GET", "/api/places", None).await,
        StatusCode::UNAUTHORIZED
    );
}

// Authorization.

#[tokio::test]
async fn mapped_roles_decide_what_a_caller_may_do() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let editor = with_roles(&idp, &["editor"]);
    let root = with_roles(&idp, &["root"]);

    for (method, path, expected) in [
        ("GET", "/api/places", StatusCode::OK),
        ("POST", "/api/places", StatusCode::OK),
        ("DELETE", "/api/places/1", StatusCode::FORBIDDEN),
        ("POST", "/api/places/merge", StatusCode::FORBIDDEN),
    ] {
        let got = status(&app, method, path, Some(&editor)).await;
        assert_eq!(got, expected, "{method} {path}");
    }
    assert_eq!(
        status(&app, "POST", "/api/places/merge", Some(&root)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_unmapped_role_grants_nothing() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    let token = with_roles(&idp, &["offline_access", "admin"]);
    assert_eq!(
        status(&app, "GET", "/api/places", Some(&token)).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, "POST", "/api/places", Some(&token)).await,
        StatusCode::FORBIDDEN
    );
}

// Enforcement off.

#[tokio::test]
async fn with_enforcement_off_no_token_is_needed_except_by_auth_user() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), false));
    assert_eq!(
        status(&app, "POST", "/api/places/merge", None).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, "GET", "/api/whoami", None).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn auth_user_fails_closed_without_the_middleware() {
    let idp = TestIdp::start().await;
    let app = Router::new().route("/api/whoami", get(whoami));
    let token = idp.token(|_| {});
    assert_eq!(
        status(&app, "GET", "/api/whoami", Some(&token)).await,
        StatusCode::UNAUTHORIZED
    );
}

// PASETO alongside Keycloak.

#[tokio::test]
async fn a_paseto_is_refused_unless_the_guard_accepts_them() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true));
    assert_eq!(
        status(&app, "GET", "/api/places", Some(GOOD_PASETO)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn with_paseto_both_kinds_of_token_work_under_one_policy() {
    let idp = TestIdp::start().await;
    let app = app(guard(settings(&idp), true).with_paseto(Arc::new(StubPaseto)));

    let keycloak = idp.token(|_| {});
    assert_eq!(
        status(&app, "POST", "/api/places", Some(&keycloak)).await,
        StatusCode::OK
    );

    assert_eq!(
        status(&app, "POST", "/api/places", Some(GOOD_PASETO)).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, "DELETE", "/api/places/1", Some(GOOD_PASETO)).await,
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        status(&app, "GET", "/api/places", Some("v4.public.forged")).await,
        StatusCode::UNAUTHORIZED
    );
}

// Configuration.

fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let pairs: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    move |name| pairs.get(name).cloned()
}

#[tokio::test]
async fn no_url_means_keycloak_is_off() {
    assert!(
        axa_keycloak_auth::verifier_from_lookup("X", &lookup(&[]))
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_full_configuration_builds_a_verifier() {
    let variables = lookup(&[
        ("X_KEYCLOAK_URL", "https://kc.example.com"),
        ("X_KEYCLOAK_REALM", "mxi"),
        ("X_KEYCLOAK_AUDIENCES", "mxi-api"),
    ]);
    assert!(
        axa_keycloak_auth::verifier_from_lookup("X", &variables)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_half_configuration_is_an_error_rather_than_off() {
    let cases = [
        vec![("X_KEYCLOAK_URL", "https://kc.example.com")],
        vec![
            ("X_KEYCLOAK_URL", "https://kc.example.com"),
            ("X_KEYCLOAK_REALM", "mxi"),
        ],
        vec![
            ("X_KEYCLOAK_URL", "http://kc.example.com"),
            ("X_KEYCLOAK_REALM", "mxi"),
            ("X_KEYCLOAK_AUDIENCES", "mxi-api"),
        ],
    ];
    for case in cases {
        assert!(
            axa_keycloak_auth::verifier_from_lookup("X", &lookup(&case)).is_err(),
            "{case:?}"
        );
    }
}
