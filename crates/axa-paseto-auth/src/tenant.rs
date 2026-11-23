//! The caller's tenant, read from the verified token and from nowhere else.
//!
//! A service that many tenants share keeps them apart by the tenant each request runs
//! in. Taking that from a header, a path, a host, or a body would let any caller name
//! any tenant, so [`Tenant`] reads only what the guard verified and the policy allowed.

use std::{fmt, sync::Arc};

use authentication_verifier::Claims;
use axa_problem::Problem;
use axum::{
    extract::{FromRequestParts, Request, State},
    http::{Extensions, StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{Allowed, Guard, invalid_token};

/// The attribute in a token's `attrs` that names the tenant, unless
/// [`GuardConfig::tenant_attribute`](crate::GuardConfig::tenant_attribute) names another.
pub const DEFAULT_TENANT_ATTRIBUTE: &str = "tenant";

/// The longest tenant id, which a DNS label also allows.
pub const MAX_TENANT_ID_LEN: usize = 63;

/// The tenant a verified, allowed caller belongs to.
///
/// As a handler argument, it refuses a request with no verified caller as `401`, and a
/// caller whose token names no tenant, more than one, or one that is not a tenant id as
/// `403`, code `no_tenant`, before the handler runs:
///
/// ```
/// use axa_paseto_auth::Tenant;
///
/// async fn list(tenant: Tenant) -> String {
///     format!("the todos of {tenant}")
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tenant(String);

impl Tenant {
    /// The one tenant `claims` name under `attribute`, if it is a tenant id.
    #[must_use]
    pub fn from_claims(claims: &Claims, attribute: &str) -> Option<Self> {
        match claims.attrs.get(attribute).map(Vec::as_slice) {
            Some([id]) if is_tenant_id(id) => Some(Self(id.clone())),
            _ => None,
        }
    }

    /// The tenant of the caller the guard verified and allowed, read from a request's
    /// extensions as the extractor reads it, but writing no audit line: for code that
    /// runs inside the guard before the handler, such as the key of a limit per tenant.
    ///
    /// # Errors
    ///
    /// As the extractor: [`TenantRejection::Unauthenticated`] when no caller was
    /// verified and allowed, and [`TenantRejection::NoTenant`] when the caller's token
    /// names no tenant, more than one, or one that is not a tenant id.
    pub fn from_extensions(extensions: &Extensions) -> Result<Self, TenantRejection> {
        // Only what the guard allowed. With no guard there is nothing here, and this
        // fails closed.
        let allowed = extensions
            .get::<Allowed>()
            .ok_or(TenantRejection::Unauthenticated)?;
        let attribute = extensions
            .get::<Arc<Guard>>()
            .map_or(DEFAULT_TENANT_ATTRIBUTE, |guard| {
                guard.config().tenant_attribute.as_str()
            });
        Self::from_claims(&allowed.claims, attribute).ok_or(TenantRejection::NoTenant)
    }

    /// The tenant id, to bind in a query or a key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Tenant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Tenant {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Whether `id` is a tenant id: lower-case ASCII letters, digits, and hyphens, not
/// starting or ending with a hyphen, from 1 to 63 characters, as a DNS label is. So it
/// is safe in a key, a log line, a subdomain, or a Postgres setting.
#[must_use]
pub fn is_tenant_id(id: &str) -> bool {
    (1..=MAX_TENANT_ID_LEN).contains(&id.len())
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Why a request has no [`Tenant`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantRejection {
    /// No caller was verified and allowed: no guard, a public path, or enforcement off.
    /// A `401`, with `WWW-Authenticate: Bearer`.
    Unauthenticated,
    /// The caller's token names no tenant, more than one, or one that is not a tenant
    /// id. A `403`, code `no_tenant`.
    NoTenant,
}

impl IntoResponse for TenantRejection {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthenticated => invalid_token().into_response(),
            Self::NoTenant => Problem::new(StatusCode::FORBIDDEN)
                .with_detail("forbidden")
                .with_extension("code", "no_tenant".into())
                .into_response(),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Tenant {
    type Rejection = TenantRejection;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(tenant_of(parts))
    }
}

fn tenant_of(parts: &Parts) -> Result<Tenant, TenantRejection> {
    let tenant = Tenant::from_extensions(&parts.extensions);
    if tenant == Err(TenantRejection::NoTenant)
        && let Some(allowed) = parts.extensions.get::<Allowed>()
    {
        // An audit line, as the guard writes for its own refusals. The subject is the
        // caller's id, and the attribute's values are not written, as they came from
        // the token.
        tracing::warn!(
            status = 403_i32,
            subject = allowed.claims.sub.as_str(),
            reason = "no_tenant",
            "auth_refused",
        );
    }
    tenant
}

/// The check [`refuse_suspended`] asks of each verified tenant: `true` when the tenant
/// is suspended. The service passes it in, as suspension belongs to the service that
/// issues tokens, while a token already issued lives until it expires.
///
/// The check runs on every request, so it should answer from memory, such as a set
/// the service keeps fresh from its issuer's events or by polling, and never wait on
/// the network.
///
/// ```
/// use std::{collections::HashSet, sync::{Arc, RwLock}};
///
/// use axa_paseto_auth::SuspendedCheck;
///
/// let suspended: Arc<RwLock<HashSet<String>>> = Arc::default();
/// let list = Arc::clone(&suspended);
/// let check = SuspendedCheck::new(move |tenant| {
///     // A poisoned lock fails closed: every tenant is refused until it is fixed.
///     list.read().map_or(true, |list| list.contains(tenant.as_str()))
/// });
/// suspended.write().unwrap().insert("acme".to_string());
/// ```
#[derive(Clone)]
pub struct SuspendedCheck(Arc<dyn Fn(&Tenant) -> bool + Send + Sync>);

impl SuspendedCheck {
    /// A check that calls `is_suspended` for each verified tenant.
    pub fn new(is_suspended: impl Fn(&Tenant) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(is_suspended))
    }

    /// Whether `tenant` is suspended.
    #[must_use]
    pub fn is_suspended(&self, tenant: &Tenant) -> bool {
        (self.0)(tenant)
    }
}

impl fmt::Debug for SuspendedCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SuspendedCheck").finish_non_exhaustive()
    }
}

/// Middleware refusing a caller whose tenant [`SuspendedCheck`] says is suspended, as
/// `403`, code `tenant_suspended`, with an audit line naming the subject and tenant.
/// Add it inside the guard, as the tenant is known only once the guard has verified
/// the token:
///
/// ```ignore
/// routes
///     .layer(from_fn_with_state(check, axa_paseto_auth::refuse_suspended))
///     .layer(from_fn_with_state(Arc::new(guard), axa_paseto_auth::middleware))
/// ```
///
/// A request with no tenant, as on a public path, passes on, for the handler's
/// [`Tenant`] to refuse as it would.
pub async fn refuse_suspended(
    State(check): State<SuspendedCheck>,
    request: Request,
    next: Next,
) -> Response {
    if let Ok(tenant) = Tenant::from_extensions(request.extensions())
        && check.is_suspended(&tenant)
    {
        let subject = request
            .extensions()
            .get::<Allowed>()
            .map(|allowed| allowed.claims.sub.clone());
        tracing::warn!(
            status = 403_i32,
            subject = subject.as_deref(),
            tenant = tenant.as_str(),
            reason = "tenant_suspended",
            "auth_refused",
        );
        return Problem::new(StatusCode::FORBIDDEN)
            .with_detail("forbidden")
            .with_extension("code", "tenant_suspended".into())
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_reads_as_its_id_every_way() {
        let tenant = Tenant("acme".to_string());
        assert_eq!(tenant.as_ref(), "acme");
        assert_eq!(tenant.as_str(), "acme");
        assert_eq!(tenant.to_string(), "acme");
    }

    #[test]
    fn a_suspended_check_shows_nothing_of_its_closure() {
        let check = SuspendedCheck::new(|_| false);
        assert_eq!(format!("{check:?}"), "SuspendedCheck { .. }");
    }
}
