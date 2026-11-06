//! The Axum layer: a [`Guard`], its [`middleware`], and the [`AuthUser`] extractor.
//!
//! [`GuardConfig`], [`derive_action`], and [`bearer_token`] are copied from
//! `axa-paseto` 0.2.0, so that this crate does not depend on it.

use std::{net::IpAddr, sync::Arc};

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, Method, StatusCode, header::AUTHORIZATION, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{
    Claims, VerifyError,
    abac::{Action, Decision, ReloadablePolicy},
    keycloak::KeycloakVerifier,
};

/// The start of every PASETO this family issues.
const PASETO_PREFIX: &str = "v4.";

/// What a refused caller is told, whatever the cause.
const INVALID_TOKEN: &str = "invalid token";

/// What a caller the policy refuses is told. The reason goes to the log.
const FORBIDDEN: &str = "forbidden";

/// A refusal: the status and the reason sent to the caller.
pub type Rejection = (StatusCode, String);

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
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "missing authorization header".to_string(),
        ))?;
    header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .map(str::trim)
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "expected bearer token".to_string(),
        ))
}

/// Verifies PASETO tokens for a [`Guard`], which holds no PASETO code of its own.
///
/// A service accepting both kinds of token implements this over its PASETO
/// verifier, and converts that verifier's claims into [`Claims`].
pub trait PasetoVerifier: Send + Sync {
    /// Verify `token`.
    ///
    /// # Errors
    ///
    /// Fails when the token does not verify.
    fn verify(&self, token: &str) -> Result<Claims, VerifyError>;
}

/// A service's guard: its config plus its live verifiers and policy.
///
/// The policy is the reloadable holder, so an edited policy takes effect on the
/// next request. Keycloak rotates its own keys, and the verifier follows them.
pub struct Guard {
    config: GuardConfig,
    keycloak: Arc<KeycloakVerifier>,
    paseto: Option<Arc<dyn PasetoVerifier>>,
    policy: Arc<ReloadablePolicy>,
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The config only: nothing about keys belongs in a log.
        f.debug_struct("Guard")
            .field("config", &self.config)
            .field("accepts_paseto", &self.paseto.is_some())
            .finish_non_exhaustive()
    }
}

impl Guard {
    /// Build a guard that accepts Keycloak tokens.
    #[must_use]
    pub fn new(
        config: GuardConfig,
        keycloak: Arc<KeycloakVerifier>,
        policy: Arc<ReloadablePolicy>,
    ) -> Self {
        if !config.require_auth {
            tracing::warn!(entity = %config.entity, "authentication enforcement is off");
        }
        Self {
            config,
            keycloak,
            paseto: None,
            policy,
        }
    }

    /// Also accept PASETO tokens.
    #[must_use]
    pub fn with_paseto(mut self, paseto: Arc<dyn PasetoVerifier>) -> Self {
        self.paseto = Some(paseto);
        self
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
    pub async fn claims(&self, headers: &HeaderMap) -> Result<Claims, Rejection> {
        let token = bearer_token(headers)?;
        let verified = if token.starts_with(PASETO_PREFIX) {
            // A PASETO never reaches Keycloak, and is refused when not accepted.
            let Some(paseto) = &self.paseto else {
                return Err(invalid_token());
            };
            paseto.verify(token)
        } else {
            self.keycloak.verify(token).await
        };
        verified.map_err(|error| {
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

    /// Decide one request. A refusal is logged, as `auth_refused`.
    ///
    /// # Errors
    ///
    /// Fails with `401` for a missing or bad credential, and `403` when the
    /// credential is valid and the policy denies it.
    pub async fn enforce(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
    ) -> Result<(), Rejection> {
        self.check(method, path, headers)
            .await
            .map_err(|refused| refused.logged(None))
    }

    async fn check(&self, method: &Method, path: &str, headers: &HeaderMap) -> Result<(), Refused> {
        if !self.config.require_auth || self.config.is_public(path) {
            return Ok(());
        }
        let action = derive_action(method, path, &self.config);
        let claims = self.claims(headers).await.map_err(|rejection| Refused {
            rejection,
            subject: None,
            reason: None,
            action,
        })?;
        let decision = self.decide(&claims, action);
        if decision.allowed {
            Ok(())
        } else {
            Err(Refused {
                rejection: (StatusCode::FORBIDDEN, FORBIDDEN.to_string()),
                subject: Some(claims.sub),
                reason: Some(decision.reason),
                action,
            })
        }
    }
}

/// A request a guard turned away, and what the audit log needs to know of it.
struct Refused {
    rejection: Rejection,
    subject: Option<String>,
    reason: Option<String>,
    action: Action,
}

impl Refused {
    /// Write the audit line, and give back what the caller is told. The line is a
    /// warning, so that a production log level keeps it. It runs inside the request's
    /// span, which holds the request id, method, and route. It never holds the token,
    /// and `subject` is the caller's id, not their email.
    fn logged(self, client: Option<IpAddr>) -> Rejection {
        tracing::warn!(
            status = self.rejection.0.as_u16(),
            action = ?self.action,
            subject = self.subject.as_deref(),
            reason = self.reason.as_deref(),
            client = client.map(|client| client.to_string()).as_deref(),
            "auth_refused",
        );
        self.rejection
    }
}

fn invalid_token() -> Rejection {
    (StatusCode::UNAUTHORIZED, INVALID_TOKEN.to_string())
}

/// Middleware applying a [`Guard`] to every request. It also makes the guard
/// available to [`AuthUser`].
pub async fn middleware(
    State(guard): State<Arc<Guard>>,
    mut request: Request,
    next: Next,
) -> Response {
    let decision = guard
        .check(request.method(), request.uri().path(), request.headers())
        .await
        .map_err(|refused| refused.logged(request.extensions().get::<IpAddr>().copied()));
    match decision {
        Ok(()) => {
            request.extensions_mut().insert(guard);
            next.run(request).await
        }
        Err(rejection) => rejection.into_response(),
    }
}

/// Extractor for a verified caller. A handler taking this requires a valid token,
/// even with enforcement off.
#[derive(Debug, Clone)]
pub struct AuthUser(pub Claims);

impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Fail closed: without the middleware there is nothing to verify against,
        // so this must never let the request through.
        let guard = parts.extensions.get::<Arc<Guard>>().cloned().ok_or((
            StatusCode::UNAUTHORIZED,
            "authentication is not configured".to_string(),
        ))?;
        guard.claims(&parts.headers).await.map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GuardConfig {
        GuardConfig::new("place")
            .public_path("/metrics.prom")
            .public_prefix("/swagger-ui")
            .destructive_post_suffix("/merge")
    }

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
        let config = config();
        assert!(config.is_public("/swagger-ui"));
        assert!(config.is_public("/swagger-ui/index.html"));
        assert!(!config.is_public("/swagger-uiX"));
        assert!(!config.is_public("/api/swagger-ui"));
    }

    #[test]
    fn public_paths_match_exactly() {
        assert!(!config().is_public("/metrics.prom/secret"));
    }

    #[test]
    fn methods_map_to_actions() {
        let config = config();
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
        let config = config();
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
}
