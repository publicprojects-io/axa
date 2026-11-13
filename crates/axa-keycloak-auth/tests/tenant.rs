//! The tenant from a real Keycloak token, through the real guard. A local OIDC provider
//! stands in for Keycloak, and the role map turns a realm role into a tenant. The role
//! map already refuses a value that is not a short lower-case token, so a malformed
//! tenant cannot reach a token here, and the unit test covers the id rule.

mod idp;

use std::sync::Arc;

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

fn app(idp: &TestIdp, config: GuardConfig) -> Router {
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
    Router::new()
        .route(
            "/api/tenant",
            get(|tenant: Tenant| async move { tenant.to_string() }),
        )
        .layer(from_fn_with_state(Arc::new(guard), middleware))
}

async fn send(app: &Router, token: Option<&str>) -> (StatusCode, String) {
    let mut request = Request::builder().uri("/api/tenant");
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

#[test]
fn a_tenant_id_is_a_dns_label_in_lower_case() {
    use axa_keycloak_auth::is_tenant_id;
    assert!(is_tenant_id("acme-2"));
    for id in ["", "Acme", "-acme", "acme-", "acme.corp", &"a".repeat(64)] {
        assert!(!is_tenant_id(id), "{id}");
    }
}
