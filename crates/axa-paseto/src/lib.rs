//! PASETO bearer authentication and ABAC enforcement for Axum services.
//!
//! Signature checks and policy evaluation live in
//! [`authentication_verifier`]. This crate adds the Axum layer: a [`Guard`]
//! that decides each request, its [`middleware`], and the [`AuthUser`]
//! extractor.
//!
//! The decision, when enforcement is on:
//!
//! | Request | Result |
//! |---|---|
//! | A public path | Allowed |
//! | No token, or a bad one | `401` |
//! | Valid token, policy denies | `403` |
//! | Valid token, policy allows | Allowed |
//!
//! A refused caller is told nothing about why. The reason goes to the log.
//!
//! Every path is protected unless it is listed as public, so a new route can
//! never be exposed by forgetting to add it to a list.
//!
//! Enforcement is **on by default**, so a service that forgets it refuses
//! requests rather than serving them. [`GuardConfig::require_auth`] turns it off,
//! for a service still integrating or behind another gate, and [`Guard::new`]
//! logs a warning when it does. With it off there is no token check and no policy
//! at all, although [`AuthUser`] still verifies on the handlers that use it.
//!
//! ```
//! use axa_paseto::{GuardConfig, derive_action};
//! use authentication_verifier::Action;
//! use axum::http::Method;
//!
//! let config = GuardConfig::new("place")
//!     .public_path("/metrics.prom")
//!     .public_prefix("/swagger-ui")
//!     .destructive_post_suffix("/merge");
//!
//! assert!(config.is_public("/_health"));
//! assert!(config.is_public("/swagger-ui/index.html"));
//! assert!(!config.is_public("/api/places"));
//!
//! // A trailing slash cannot downgrade a destructive action.
//! let action = derive_action(&Method::POST, "/api/places/merge/", &config);
//! assert_eq!(action, Action::Destructive);
//! ```

use std::sync::Arc;

use authentication_verifier::{Action, Claims, Decision, ReloadablePolicy, ReloadableVerifier};
use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, Method, StatusCode, header::AUTHORIZATION, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// What a refused caller is told, whatever the cause.
const INVALID_TOKEN: &str = "invalid token";

/// What a caller the policy refuses is told. The reason goes to the log.
const FORBIDDEN: &str = "forbidden";

/// A refusal: the status and the text sent to the caller.
pub type Rejection = (StatusCode, String);

fn invalid_token() -> Rejection {
    (StatusCode::UNAUTHORIZED, INVALID_TOKEN.to_string())
}

/// What a service declares about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardConfig {
    /// The entity name policies see.
    pub entity: String,
    /// Whether requests must carry a token. On by default.
    pub require_auth: bool,
    /// Paths open to anyone, matched exactly.
    pub public_paths: Vec<String>,
    /// Path prefixes open to anyone, matched on whole path segments.
    pub public_prefixes: Vec<String>,
    /// `POST` path suffixes that count as destructive rather than a write.
    pub destructive_post_suffixes: Vec<String>,
}

impl GuardConfig {
    /// A config for `entity`, with enforcement on and only the health routes
    /// public. Everything else stays protected until listed.
    #[must_use]
    pub fn new(entity: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            require_auth: true,
            public_paths: ["/_ping", "/_health", "/_readiness"]
                .map(str::to_string)
                .to_vec(),
            public_prefixes: Vec::new(),
            destructive_post_suffixes: Vec::new(),
        }
    }

    /// Set whether requests must carry a token.
    #[must_use]
    pub fn require_auth(mut self, require_auth: bool) -> Self {
        self.require_auth = require_auth;
        self
    }

    /// Add a public path.
    #[must_use]
    pub fn public_path(mut self, path: impl Into<String>) -> Self {
        self.public_paths.push(path.into());
        self
    }

    /// Add a public prefix.
    #[must_use]
    pub fn public_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.public_prefixes.push(prefix.into());
        self
    }

    /// Add a `POST` suffix that counts as destructive.
    #[must_use]
    pub fn destructive_post_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.destructive_post_suffixes.push(suffix.into());
        self
    }

    /// Whether `path` needs no token.
    ///
    /// A prefix matches only on a whole path segment, so a public
    /// `/swagger-ui` does not also open `/swagger-uiX`.
    #[must_use]
    pub fn is_public(&self, path: &str) -> bool {
        self.public_paths.iter().any(|public| public == path)
            || self.public_prefixes.iter().any(|prefix| {
                path.strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
    }
}

/// Parse an on/off setting: `1`, `true`, `yes`, and `on` mean on, and `0`, `false`,
/// `no`, and `off` mean off, in any case.
///
/// Anything else is `None`, for the caller to refuse. A typo must not read as off,
/// because off can be what turns enforcement off.
#[must_use]
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// The ABAC action a request performs.
#[must_use]
pub fn derive_action(method: &Method, path: &str, config: &GuardConfig) -> Action {
    // SEC-G6: normalise a trailing slash before the suffix check, so that
    // `POST .../merge/` stays destructive. Otherwise an `access=write` caller
    // could reach a destructive operation by adding one character.
    let path = path.trim_end_matches('/');
    match *method {
        Method::GET | Method::HEAD | Method::OPTIONS => Action::Read,
        Method::DELETE => Action::Delete,
        Method::POST
            if config
                .destructive_post_suffixes
                .iter()
                .any(|suffix| path.ends_with(suffix.as_str())) =>
        {
            Action::Destructive
        }
        _ => Action::Write,
    }
}

/// The bearer token in `headers`.
///
/// # Errors
///
/// Fails with `401` when the header is missing, is not text, or is not a
/// bearer token.
pub fn bearer_token(headers: &HeaderMap) -> Result<&str, Rejection> {
    let header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(invalid_token)?;
    header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .map(str::trim)
        .ok_or_else(invalid_token)
}

/// A service's guard: its config plus its live verifier and policy.
///
/// The verifier and policy are the reloadable holders, so a rotated key set or
/// an edited policy takes effect on the next request.
pub struct Guard {
    config: GuardConfig,
    verifier: Arc<ReloadableVerifier>,
    policy: Arc<ReloadablePolicy>,
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The config only: nothing about keys belongs in a log.
        f.debug_struct("Guard")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Guard {
    /// Build a guard.
    #[must_use]
    pub fn new(
        config: GuardConfig,
        verifier: Arc<ReloadableVerifier>,
        policy: Arc<ReloadablePolicy>,
    ) -> Self {
        if !config.require_auth {
            tracing::warn!(entity = %config.entity, "authentication enforcement is off");
        }
        Self {
            config,
            verifier,
            policy,
        }
    }

    /// The guard's config.
    #[must_use]
    pub fn config(&self) -> &GuardConfig {
        &self.config
    }

    /// Verify the bearer token in `headers`.
    ///
    /// # Errors
    ///
    /// Fails with `401` when the token is missing or does not verify.
    pub fn claims(&self, headers: &HeaderMap) -> Result<Claims, Rejection> {
        let token = bearer_token(headers)?;
        self.verifier.current().verify(token).map_err(|error| {
            tracing::debug!(%error, "bearer token refused");
            invalid_token()
        })
    }

    /// The policy's decision for `claims` performing `action`.
    #[must_use]
    pub fn decide(&self, claims: &Claims, action: Action) -> Decision {
        self.policy
            .current()
            .evaluate(claims, action, &self.config.entity)
    }

    /// Decide one request.
    ///
    /// # Errors
    ///
    /// Fails with `401` for a missing or bad credential, and `403` when the
    /// credential is valid and the policy denies it.
    pub fn enforce(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
    ) -> Result<(), Rejection> {
        if !self.config.require_auth || self.config.is_public(path) {
            return Ok(());
        }
        let claims = self.claims(headers)?;
        let decision = self.decide(&claims, derive_action(method, path, &self.config));
        if decision.allowed {
            Ok(())
        } else {
            tracing::info!(reason = %decision.reason, "paseto_forbidden");
            Err((StatusCode::FORBIDDEN, FORBIDDEN.to_string()))
        }
    }
}

/// Middleware applying a [`Guard`] to every request. It also makes the guard
/// available to [`AuthUser`].
pub async fn middleware(
    State(guard): State<Arc<Guard>>,
    mut request: Request,
    next: Next,
) -> Response {
    let decision = guard.enforce(request.method(), request.uri().path(), request.headers());
    match decision {
        Ok(()) => {
            request.extensions_mut().insert(guard);
            next.run(request).await
        }
        Err(rejection) => rejection.into_response(),
    }
}

/// Extractor for a verified caller. A handler taking this requires a valid
/// token, even with enforcement off.
#[derive(Debug, Clone)]
pub struct AuthUser(pub Claims);

impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = Rejection;

    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Fail closed: without the middleware there is nothing to verify
        // against, so this must never let the request through.
        let guard = parts
            .extensions
            .get::<Arc<Guard>>()
            .ok_or_else(invalid_token)?;
        guard.claims(&parts.headers).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use authentication_verifier::{Policy, Verifier};
    use axum::{
        Json, Router,
        body::Body,
        http::Request as HttpRequest,
        middleware::from_fn_with_state,
        routing::{get, post},
    };
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::SigningKey;
    use rusty_paseto::core::{
        Footer, Key, Paseto, PasetoAsymmetricPrivateKey, Payload, Public, V4,
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    /// Throwaway seed for test tokens. Not a secret.
    const SEED: [u8; 32] = [17; 32];
    const OTHER_SEED: [u8; 32] = [99; 32];
    const ISSUER: &str = "authentication-service";
    const AUDIENCE: &str = "main-x-service";
    /// `iat` in 2023, and an `exp` thousands of years later.
    const IAT: i64 = 1_700_000_000;
    const FAR_FUTURE: i64 = IAT + 10_000_000_000;

    fn kid(seed: &[u8; 32]) -> String {
        let public = SigningKey::from_bytes(seed).verifying_key().to_bytes();
        URL_SAFE_NO_PAD.encode(Sha256::digest(public))
    }

    fn verifier() -> Arc<ReloadableVerifier> {
        let public = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
        let keys = json!({
            "keys": [{ "kty": "OKP", "crv": "Ed25519", "use": "sig",
                       "kid": kid(&SEED), "x": URL_SAFE_NO_PAD.encode(public) }]
        });
        let verifier = Verifier::from_paseto_keys_value(&keys, ISSUER, AUDIENCE).unwrap();
        Arc::new(ReloadableVerifier::new(verifier))
    }

    /// Sign a token with `seed`, naming the real key's `kid` so a wrong seed
    /// reaches the signature check rather than failing on an unknown key.
    fn sign(seed: &[u8; 32], exp: i64, attrs: &Value) -> String {
        let payload = json!({
            "sub": "11111111-1111-1111-1111-111111111111",
            "email": "alice@example.com", "name": "Alice",
            "iss": ISSUER, "aud": AUDIENCE,
            "exp": exp, "iat": IAT, "sid": "test-sid", "attrs": attrs,
        })
        .to_string();
        let keypair = SigningKey::from_bytes(seed).to_keypair_bytes();
        let key = Key::<64>::from(keypair);
        let private = PasetoAsymmetricPrivateKey::<V4, Public>::from(&key);
        let footer = format!(r#"{{"kid":"{}"}}"#, kid(&SEED));
        let mut builder = Paseto::<V4, Public>::builder();
        builder.set_payload(Payload::from(payload.as_str()));
        builder.set_footer(Footer::from(footer.as_str()));
        builder.try_sign(&private).unwrap()
    }

    fn token(attrs: &Value) -> String {
        sign(&SEED, FAR_FUTURE, attrs)
    }

    fn config(require_auth: bool) -> GuardConfig {
        GuardConfig::new("place")
            .require_auth(require_auth)
            .public_path("/metrics.prom")
            .public_prefix("/swagger-ui")
            .destructive_post_suffix("/merge")
    }

    fn guard(require_auth: bool) -> Arc<Guard> {
        Arc::new(Guard::new(
            config(require_auth),
            verifier(),
            Arc::new(ReloadablePolicy::new(Policy::default_policy())),
        ))
    }

    async fn whoami(AuthUser(claims): AuthUser) -> Json<String> {
        Json(claims.email)
    }

    fn app(require_auth: bool) -> Router {
        Router::new()
            .route("/_health", get(|| async { "ok" }))
            .route(
                "/api/places",
                get(|| async { "ok" }).post(|| async { "ok" }),
            )
            .route("/api/places/merge", post(|| async { "ok" }))
            .route("/api/places/merge/", post(|| async { "ok" }))
            .route("/api/places/1", axum::routing::delete(|| async { "ok" }))
            .route("/api/whoami", get(whoami))
            .layer(from_fn_with_state(guard(require_auth), middleware))
    }

    async fn status(app: Router, method: &str, path: &str, token: Option<&str>) -> StatusCode {
        let mut request = HttpRequest::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = request.body(Body::empty()).unwrap();
        app.oneshot(request).await.unwrap().status()
    }

    // Public paths.

    #[test]
    fn health_routes_are_public_by_default() {
        let config = GuardConfig::new("x");
        assert!(config.is_public("/_health"));
        assert!(!config.is_public("/metrics.prom"));
    }

    #[test]
    fn enforcement_is_on_unless_turned_off() {
        assert!(GuardConfig::new("x").require_auth);
        assert!(!GuardConfig::new("x").require_auth(false).require_auth);
    }

    #[test]
    fn prefixes_match_whole_segments_only() {
        let config = config(true);
        assert!(config.is_public("/swagger-ui"));
        assert!(config.is_public("/swagger-ui/index.html"));
        assert!(!config.is_public("/swagger-uiX"));
        assert!(!config.is_public("/api/swagger-ui"));
    }

    #[test]
    fn public_paths_match_exactly() {
        assert!(!config(true).is_public("/metrics.prom/secret"));
    }

    // Actions.

    #[test]
    fn methods_map_to_actions() {
        let config = config(true);
        assert_eq!(derive_action(&Method::GET, "/a", &config), Action::Read);
        assert_eq!(derive_action(&Method::HEAD, "/a", &config), Action::Read);
        assert_eq!(derive_action(&Method::OPTIONS, "/a", &config), Action::Read);
        assert_eq!(derive_action(&Method::POST, "/a", &config), Action::Write);
        assert_eq!(derive_action(&Method::PUT, "/a", &config), Action::Write);
        assert_eq!(derive_action(&Method::PATCH, "/a", &config), Action::Write);
        assert_eq!(
            derive_action(&Method::DELETE, "/a", &config),
            Action::Delete
        );
    }

    #[test]
    fn a_trailing_slash_cannot_downgrade_a_destructive_post() {
        let config = config(true);
        for path in [
            "/api/places/merge",
            "/api/places/merge/",
            "/api/places/merge//",
        ] {
            assert_eq!(
                derive_action(&Method::POST, path, &config),
                Action::Destructive,
                "{path}"
            );
        }
    }

    // Parsing.

    #[test]
    fn on_off_settings_parse() {
        for value in ["1", "true", "TRUE", " yes ", "on"] {
            assert_eq!(parse_bool(value), Some(true), "{value}");
        }
        for value in ["0", "false", "FALSE", " no ", "off"] {
            assert_eq!(parse_bool(value), Some(false), "{value}");
        }
        for value in ["", "junk", "ture", "of"] {
            assert_eq!(parse_bool(value), None, "{value}");
        }
    }

    #[test]
    fn bearer_tokens_are_extracted() {
        let mut headers = HeaderMap::new();
        assert_eq!(
            bearer_token(&headers).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        headers.insert(AUTHORIZATION, "Basic abc".parse().unwrap());
        assert_eq!(
            bearer_token(&headers).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        headers.insert(AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(bearer_token(&headers).unwrap(), "abc");
        headers.insert(AUTHORIZATION, "bearer abc".parse().unwrap());
        assert_eq!(bearer_token(&headers).unwrap(), "abc");
    }

    // Enforcement off.

    #[tokio::test]
    async fn with_enforcement_off_no_token_is_needed() {
        let status = status(app(false), "POST", "/api/places/merge", None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_user_verifies_even_with_enforcement_off() {
        assert_eq!(
            status(app(false), "GET", "/api/whoami", None).await,
            StatusCode::UNAUTHORIZED
        );
        let token = token(&json!({}));
        assert_eq!(
            status(app(false), "GET", "/api/whoami", Some(&token)).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn auth_user_fails_closed_without_the_middleware() {
        let app = Router::new().route("/api/whoami", get(whoami));
        let token = token(&json!({}));
        assert_eq!(
            status(app, "GET", "/api/whoami", Some(&token)).await,
            StatusCode::UNAUTHORIZED
        );
    }

    // Enforcement on: authentication.

    #[tokio::test]
    async fn public_paths_need_no_token() {
        assert_eq!(
            status(app(true), "GET", "/_health", None).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_refusal_tells_the_caller_nothing_about_why() {
        async fn answer(method: &str, header: Option<String>) -> (StatusCode, String) {
            let mut request = HttpRequest::builder().method(method).uri("/api/places");
            if let Some(header) = header {
                request = request.header(AUTHORIZATION, header);
            }
            let request = request.body(Body::empty()).unwrap();
            let response = app(true).oneshot(request).await.unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            (status, String::from_utf8(body.to_vec()).unwrap())
        }

        let forged = sign(&[9; 32], FAR_FUTURE, &json!({}));
        for header in [
            None,
            Some("Basic abc".to_string()),
            Some("Bearer garbage".to_string()),
            Some(format!("Bearer {forged}")),
        ] {
            let refused = (StatusCode::UNAUTHORIZED, "invalid token".to_string());
            assert_eq!(answer("GET", header).await, refused);
        }

        let token = token(&json!({}));
        let refused = (StatusCode::FORBIDDEN, "forbidden".to_string());
        assert_eq!(
            answer("POST", Some(format!("Bearer {token}"))).await,
            refused
        );
    }

    #[tokio::test]
    async fn a_missing_token_is_a_401() {
        assert_eq!(
            status(app(true), "GET", "/api/places", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn an_unlisted_path_is_protected_too() {
        assert_eq!(
            status(app(true), "GET", "/api/anything-new", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn garbage_is_a_401() {
        assert_eq!(
            status(app(true), "GET", "/api/places", Some("v4.public.garbage")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn a_token_signed_by_another_key_is_a_401() {
        let forged = sign(&OTHER_SEED, FAR_FUTURE, &json!({ "access": ["admin"] }));
        assert_eq!(
            status(app(true), "GET", "/api/places", Some(&forged)).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn an_expired_token_is_a_401() {
        let expired = sign(&SEED, IAT + 1, &json!({ "access": ["admin"] }));
        assert_eq!(
            status(app(true), "GET", "/api/places", Some(&expired)).await,
            StatusCode::UNAUTHORIZED
        );
    }

    // Enforcement on: authorization under the default policy.

    #[tokio::test]
    async fn any_valid_token_may_read() {
        let token = token(&json!({}));
        assert_eq!(
            status(app(true), "GET", "/api/places", Some(&token)).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn no_attributes_means_no_mutation() {
        let token = token(&json!({}));
        assert_eq!(
            status(app(true), "POST", "/api/places", Some(&token)).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn write_access_writes_but_cannot_delete_or_merge() {
        let token = token(&json!({ "access": ["write"] }));
        assert_eq!(
            status(app(true), "POST", "/api/places", Some(&token)).await,
            StatusCode::OK
        );
        assert_eq!(
            status(app(true), "DELETE", "/api/places/1", Some(&token)).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(app(true), "POST", "/api/places/merge", Some(&token)).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn a_trailing_slash_does_not_reach_a_destructive_route_with_write_access() {
        let token = token(&json!({ "access": ["write"] }));
        assert_eq!(
            status(app(true), "POST", "/api/places/merge/", Some(&token)).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn admin_access_may_delete_and_merge() {
        let token = token(&json!({ "access": ["admin"] }));
        assert_eq!(
            status(app(true), "DELETE", "/api/places/1", Some(&token)).await,
            StatusCode::OK
        );
        assert_eq!(
            status(app(true), "POST", "/api/places/merge", Some(&token)).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_machine_peer_may_do_everything() {
        let token = token(&json!({ "svc": ["true"] }));
        assert_eq!(
            status(app(true), "POST", "/api/places/merge", Some(&token)).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn an_edited_policy_applies_to_the_next_request() {
        let policy = Arc::new(ReloadablePolicy::new(Policy::default_policy()));
        let guard = Arc::new(Guard::new(config(true), verifier(), policy.clone()));
        let app = Router::new()
            .route("/api/places", get(|| async { "ok" }))
            .layer(from_fn_with_state(guard, middleware));
        let token = token(&json!({}));
        assert_eq!(
            status(app.clone(), "GET", "/api/places", Some(&token)).await,
            StatusCode::OK
        );
        policy.store(
            Policy::from_json(r#"{"rules":[{"effect":"deny","actions":["read"],"when":{}}]}"#)
                .unwrap(),
        );
        assert_eq!(
            status(app, "GET", "/api/places", Some(&token)).await,
            StatusCode::FORBIDDEN
        );
    }
}
