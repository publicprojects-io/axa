//! The caller's tenant, read from the verified token and from nowhere else.
//!
//! A service that many tenants share keeps them apart by the tenant each request runs
//! in. Taking that from a header, a path, a host, or a body would let any caller name
//! any tenant, so [`Tenant`] reads only what the guard verified and the policy allowed.

use std::{fmt, sync::Arc};

use authentication_verifier::Claims;
use axa_problem::Problem;
use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
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
    // Only what the guard allowed. With no guard there is nothing here, and this fails
    // closed.
    let allowed = parts
        .extensions
        .get::<Allowed>()
        .ok_or(TenantRejection::Unauthenticated)?;
    let attribute = parts
        .extensions
        .get::<Arc<Guard>>()
        .map_or(DEFAULT_TENANT_ATTRIBUTE, |guard| {
            guard.config().tenant_attribute.as_str()
        });
    Tenant::from_claims(&allowed.claims, attribute).ok_or_else(|| {
        // An audit line, as the guard writes for its own refusals. The subject is the
        // caller's id, and the attribute's values are not written, as they came from
        // the token.
        tracing::warn!(
            status = 403,
            subject = allowed.claims.sub.as_str(),
            reason = "no_tenant",
            "auth_refused",
        );
        TenantRejection::NoTenant
    })
}
