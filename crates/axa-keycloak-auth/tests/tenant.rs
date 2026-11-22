#![allow(clippy::unwrap_used, reason = "RCA-SCOPE-4: tests may unwrap")]
//! The tenant from a real Keycloak token, through the real guard. A local OIDC provider
//! stands in for Keycloak, and the role map turns a realm role into a tenant. The role
//! map already refuses a value that is not a short lower-case token, so a malformed
//! tenant cannot reach a token here, and the unit test covers the id rule.

use crate::idp;

use std::sync::Arc;

use axa_keycloak_auth::{
    Guard, GuardConfig, KeycloakSettings, KeycloakVerifier, Policy, ReloadablePolicy,
    SuspendedCheck, Tenant, middleware, refuse_suspended,
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

fn app(idp: &TestIdp, config: GuardConfig) -> Router {
    checked_app(idp, config, None)
}

/// The app, with `check` inside the guard, where the tenant is known.
fn checked_app(idp: &TestIdp, config: GuardConfig, check: Option<SuspendedCheck>) -> Router {
    let settings = KeycloakSettings {
        url: idp.base_url.clone(),
        realm: "mxi".to_string(),
        audiences: vec!["mxi-api".to_string()],
        role_map: serde_json::from_value(json!({
            "acme-member": { "tenant": ["acme"] },
            "globex-member": { "tenant": ["globex"] },
            "acme-org": { "org": ["acme"] },
        }))
        .unwrap(),
        require_verified_email: true,
    };
    let guard = Guard::new(
        config,
        Arc::new(KeycloakVerifier::new(settings).unwrap()),
        Arc::new(ReloadablePolicy::new(Policy::default_policy())),
    );
    let router = Router::new()
        .route(
            "/api/tenant",
            get(|tenant: Tenant| async move { tenant.to_string() }),
        )
        .route(
            "/api/as-ref",
            get(|tenant: Tenant| async move { AsRef::<str>::as_ref(&tenant).to_string() }),
        )
        .route(
            "/api/from-extensions",
            get(|request: axum::extract::Request| async move {
                match Tenant::from_extensions(request.extensions()) {
                    Ok(tenant) => tenant.to_string(),
                    Err(rejection) => format!("{rejection:?}"),
                }
            }),
        );
    let router = match check {
        Some(check) => router.layer(from_fn_with_state(check, refuse_suspended)),
        None => router,
    };
    router.layer(from_fn_with_state(Arc::new(guard), middleware))
}

async fn send(app: &Router, token: Option<&str>) -> (StatusCode, String) {
    send_to(app, "/api/tenant", token).await
}

async fn send_to(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().uri(path);
    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn with_roles(idp: &TestIdp, roles: &[&str]) -> String {
    idp.token(|claims| claims["realm_access"] = json!({ "roles": roles }))
}

#[tokio::test]
async fn a_tenant_is_exactly_one_valid_id_from_the_mapped_roles() {
    let idp = TestIdp::start().await;
    let app = app(&idp, GuardConfig::new("place"));
    let (status, body) = send(&app, Some(&with_roles(&idp, &["acme-member"]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, "acme");

    for roles in [
        &[][..],
        &["acme-member", "globex-member"][..],
        &["acme-org"][..],
    ] {
        let (status, body) = send(&app, Some(&with_roles(&idp, roles))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{roles:?}");
        assert!(body.contains(r#""code":"no_tenant""#), "{body}");
    }
    let (status, _) = send(&app, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_guard_can_name_the_tenant_attribute() {
    let idp = TestIdp::start().await;
    let app = app(&idp, GuardConfig::new("place").tenant_attribute("org"));
    let (status, body) = send(&app, Some(&with_roles(&idp, &["acme-org"]))).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "acme"));
    let (status, _) = send(&app, Some(&with_roles(&idp, &["acme-member"]))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn from_extensions_finds_the_tenant_the_extractor_finds() {
    let idp = TestIdp::start().await;
    let app = app(&idp, GuardConfig::new("place").tenant_attribute("org"));
    let path = "/api/from-extensions";
    let (_, body) = send_to(&app, path, Some(&with_roles(&idp, &["acme-org"]))).await;
    assert_eq!(body, "acme");
    let (_, body) = send_to(&app, path, Some(&with_roles(&idp, &["acme-member"]))).await;
    assert_eq!(body, "NoTenant");
    let bare = Router::new().route(
        path,
        get(|request: axum::extract::Request| async move {
            format!("{:?}", Tenant::from_extensions(request.extensions()))
        }),
    );
    let (_, body) = send_to(&bare, path, None).await;
    assert_eq!(body, "Err(Unauthenticated)");
}

#[tokio::test]
async fn a_suspended_tenant_is_refused_from_its_next_request() {
    use std::{collections::HashSet, sync::RwLock};

    let idp = TestIdp::start().await;
    let suspended: Arc<RwLock<HashSet<String>>> = Arc::default();
    let list = Arc::clone(&suspended);
    let check = SuspendedCheck::new(move |tenant| {
        list.read()
            .map_or(true, |list| list.contains(tenant.as_str()))
    });
    let app = checked_app(&idp, GuardConfig::new("place"), Some(check));
    let acme = with_roles(&idp, &["acme-member"]);
    let globex = with_roles(&idp, &["globex-member"]);

    assert_eq!(send(&app, Some(&acme)).await.0, StatusCode::OK);
    suspended.write().unwrap().insert("acme".to_string());
    let (status, body) = send(&app, Some(&acme)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains(r#""code":"tenant_suspended""#), "{body}");
    assert_eq!(
        send(&app, Some(&globex)).await,
        (StatusCode::OK, "globex".into())
    );
    suspended.write().unwrap().remove("acme");
    assert_eq!(send(&app, Some(&acme)).await.0, StatusCode::OK);

    let (status, body) = send(&app, Some(&with_roles(&idp, &[]))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains(r#""code":"no_tenant""#), "{body}");
}

#[test]
fn a_tenant_id_is_a_dns_label_in_lower_case() {
    use axa_keycloak_auth::is_tenant_id;
    assert!(is_tenant_id("acme-2"));
    for id in ["", "Acme", "-acme", "acme-", "acme.corp", &"a".repeat(64)] {
        assert!(!is_tenant_id(id), "{id}");
    }
}

#[tokio::test]
async fn a_tenant_with_no_caller_is_a_401_and_one_with_a_caller_reads_as_text() {
    let bare = Router::new().route(
        "/api/whoami",
        get(|tenant: Tenant| async move { tenant.to_string() }),
    );
    let response = bare
        .oneshot(Request::get("/api/whoami").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["www-authenticate"], "Bearer");

    let idp = TestIdp::start().await;
    let app = app(&idp, GuardConfig::new("place").tenant_attribute("org"));
    let token = with_roles(&idp, &["acme-org"]);
    assert_eq!(
        send_to(&app, "/api/as-ref", Some(&token)).await,
        (StatusCode::OK, "acme".to_string())
    );
    assert_eq!(
        format!("{:?}", SuspendedCheck::new(|_| false)),
        "SuspendedCheck { .. }"
    );
}
