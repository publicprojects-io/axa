//! Keycloak bearer authentication and ABAC enforcement for Axum services.
//!
//! This crate stands alone, so that it can be run and audited by itself. It depends
//! on neither `authentication-verifier` nor `axa-paseto-auth`, and the code it needs from
//! them is copied in. See [Where the code came from](#where-the-code-came-from).
//!
//! A [`Guard`] decides each request, and comes with a [`middleware`] and the
//! [`AuthUser`] extractor. The [`keycloak`] module verifies tokens, and the [`abac`]
//! module evaluates policy.
//!
//! The decision, when enforcement is on:
//!
//! | Request | Result |
//! |---|---|
//! | A public path | Allowed |
//! | No token, or a bad one | `401` |
//! | Valid token, policy denies | `403`, with the reason |
//! | Valid token, policy allows | Allowed |
//!
//! A refusal is an RFC 9457 problem. A `401`'s `detail` says only `invalid token`. The cause goes to the log at `debug`, because
//! it can name the realm's issuer and its keys, and a caller has no use for either.
//!
//! # What a token must satisfy
//!
//! - A valid signature, from the realm's published keys.
//! - An issuer of `{url}/realms/{realm}`.
//! - One of the configured audiences. At least one is required, because an empty
//!   list would accept a token minted for any client of the realm.
//! - A verified email, when it carries an email and
//!   [`KeycloakSettings::require_verified_email`] is on, which is the default.
//!
//! Keycloak roles become ABAC attributes only through
//! [`KeycloakSettings::role_map`]. An unmapped role grants nothing, so a realm with
//! no role map yields read-only callers under the default policy.
//!
//! Until the realm's discovery document has been fetched, every token is refused,
//! and the service still boots.
//!
//! # Use
//!
//! Build the verifier inside a Tokio runtime, because it starts discovery in the
//! background. `<PREFIX>_KEYCLOAK_URL` being unset means Keycloak is off.
//!
//! ```no_run
//! use std::sync::Arc;
//! use axa_keycloak_auth::{
//!     AuthUser, Guard, GuardConfig, Policy, ReloadablePolicy, middleware, verifier_from_env,
//! };
//! use axum::{Router, middleware::from_fn_with_state, routing::get};
//!
//! async fn whoami(AuthUser(claims): AuthUser) -> String {
//!     claims.sub
//! }
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let verifier = verifier_from_env("PLACE")?.expect("PLACE_KEYCLOAK_URL is not set");
//! let policy = Arc::new(ReloadablePolicy::new(Policy::default_policy()));
//! let guard = Arc::new(Guard::new(
//!     GuardConfig::new("place").destructive_post_suffix("/merge"),
//!     Arc::new(verifier),
//!     policy,
//! ));
//!
//! let app: Router = Router::new()
//!     .route("/api/whoami", get(whoami))
//!     .layer(from_fn_with_state(guard, middleware));
//! # let _ = app;
//! # Ok(())
//! # }
//! ```
//!
//! # Obligations
//!
//! A policy rule can allow a request on a condition, such as `"audit"`, that this crate
//! cannot carry out. A decision with an obligation the service has not declared through
//! [`Guard::honouring`] is therefore a `403`. A handler reads the decision through the
//! [`Allowed`] extractor and acts on it.
//!
//! # Readiness
//!
//! Until the realm's keys arrive, the service runs and refuses every token. Add
//! [`KeycloakVerifier::ready`] to `/_health/ready` so that traffic waits for them.
//!
//! # Keycloak and PASETO together
//!
//! [`Guard::with_paseto`] accepts both. A token beginning `v4.` goes to the service's
//! [`PasetoVerifier`] and anything else goes to Keycloak, so the two share claims
//! and policy. Without it, a PASETO token is refused. This crate holds no
//! PASETO code, so the service supplies that verifier.
//!
//! # What this crate leaves out
//!
//! Browser sign-in, single logout, and the Keycloak admin API.
//!
//! # Where the code came from
//!
//! These are copies, and a fix to the original does not reach them. Review them
//! together whenever the original changes.
//!
//! The copies cite `authorization-attributes.md`, the policy model behind ABAC. It sits
//! beside `authentication-verifier`, and is not part of this crate.
//!
//! | Here | Copied from |
//! |---|---|
//! | [`keycloak`] | `authentication-verifier` 0.9.1, `src/keycloak.rs` |
//! | [`abac`] | `authentication-verifier` 0.9.1, `src/abac.rs`, less one test of a cookbook kept in that repository |
//! | [`Claims`] | `authentication-verifier` 0.9.1, `src/lib.rs` |
//! | [`VerifyError`] | `authentication-verifier` 0.9.1, `src/lib.rs`, with only the two variants used here |
//! | [`GuardConfig`], [`derive_action`], [`bearer_token`] | `axa-paseto-auth` 0.2.0, `src/lib.rs` |

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod abac;
mod guard;
pub mod keycloak;

pub use abac::{Action, ActionPattern, Decision, Effect, Policy, ReloadablePolicy, Rule};
pub use guard::{
    Allowed, AuthUser, Guard, GuardConfig, PasetoVerifier, Rejection, bearer_token, derive_action,
    middleware,
};
pub use keycloak::{KeycloakSettings, KeycloakVerifier, settings_from_lookup};

/// Verified token claims. Mirrors the auth-service `Claims` exactly so a
/// token signed there round-trips here. `sub` carries the user `pid`.
///
/// The field set is a contract with the auth-service: the service defines
/// an identical struct, and changing one without the other breaks token
/// round-tripping. `exp` / `iat` / `nbf` are unix seconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — the user `pid` (UUID string); the stable identifier a
    /// peer service keys its authorization on.
    pub sub: String,
    /// User email, surfaced for convenience at the edge; not used for
    /// authorization decisions.
    pub email: String,
    /// Human-readable display name carried alongside the subject.
    pub name: String,
    /// Issuer (`iss`) — the auth-service that minted the token. Checked
    /// against the verifier's configured issuer.
    pub iss: String,
    /// Audience (`aud`) — the intended recipient service. Checked against
    /// the verifier's configured audience so a token issued for one peer
    /// cannot be replayed against another.
    pub aud: String,
    /// Expiry (`exp`), unix seconds. Tokens at or past this instant are
    /// rejected. Issued ~5 minutes out (the session is the durable thing).
    pub exp: i64,
    /// Issued-at (`iat`), unix seconds — when the token was minted.
    pub iat: i64,
    /// Not-before (`nbf`), unix seconds. When present, tokens before this
    /// instant are rejected. Omitted from the wire form when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<i64>,
    /// Session id (`sid`) — the originating server-side session, so a
    /// token can be correlated back to (and revoked with) its session.
    pub sid: String,
    /// Granted scopes, if any. Empty when the token carries none.
    ///
    /// **Deprecated for authorization** (kept on the wire for
    /// compatibility; removal is a future major): the ABAC guard ignores
    /// `scope` and decides from [`attrs`](Self::attrs) instead. See
    /// `agents/share/authorization-attributes.md` §3.
    #[serde(default)]
    pub scope: Vec<String>,
    /// Granted roles, if any. Empty when the token carries none.
    ///
    /// **Deprecated for authorization** (kept on the wire for
    /// compatibility; removal is a future major): the ABAC guard ignores
    /// `roles` and decides from [`attrs`](Self::attrs) instead — a role,
    /// where one is wanted, is just another attribute (`role=editor`).
    /// See `agents/share/authorization-attributes.md` §3.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Subject attributes for ABAC authorization — a string→strings map
    /// minted by the auth-service from the user's assigned attributes
    /// (e.g. `access: ["write"]`, `dept: ["cardiology"]`,
    /// `svc: ["true"]` for machine peers). Multi-valued keys mean "has
    /// each of these values"; policies match set-membership; unknown
    /// attributes are inert (forward-compatible). Absent on the wire
    /// (old tokens) ⇒ empty map — no re-issue needed. Evaluated by the
    /// [`abac`] engine per `agents/share/authorization-attributes.md`
    /// §2–§3, alongside the pseudo-attributes `sub` and `email`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, Vec<String>>,
}

/// Why a token was refused, or why a verifier could not be built.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The signature was valid but a claim was not acceptable: the wrong issuer,
    /// or an unverified email.
    #[error("claim rejected: {0}")]
    Claim(String),

    /// A token was rejected, or a verifier could not be built: bad signature, wrong
    /// audience, expired, unreachable realm, invalid settings.
    #[error("keycloak: {0}")]
    Keycloak(String),
}

/// Build a verifier from `<PREFIX>_KEYCLOAK_*` in the process environment, as
/// [`verifier_from_lookup`] does.
///
/// # Errors
///
/// See [`verifier_from_lookup`].
pub fn verifier_from_env(prefix: &str) -> Result<Option<KeycloakVerifier>, VerifyError> {
    verifier_from_lookup(prefix, &|name| std::env::var(name).ok())
}

/// Build a verifier from `<PREFIX>_KEYCLOAK_*` variables read through `lookup`.
/// `Ok(None)` means Keycloak is off, because the URL is unset. Must run inside a
/// Tokio runtime.
///
/// | Variable | Meaning |
/// |---|---|
/// | `<PREFIX>_KEYCLOAK_URL` | Server base URL, `https` unless loopback. Unset means off |
/// | `<PREFIX>_KEYCLOAK_REALM` | Realm name. Required once the URL is set |
/// | `<PREFIX>_KEYCLOAK_AUDIENCES` | Accepted audiences, comma-separated. Required |
/// | `<PREFIX>_KEYCLOAK_ROLE_MAP_FILE` | JSON role map in a file. Wins over `_ROLE_MAP` |
/// | `<PREFIX>_KEYCLOAK_ROLE_MAP` | JSON role map, `{"<role>": {"<attr>": ["<value>"]}}` |
/// | `<PREFIX>_KEYCLOAK_REQUIRE_VERIFIED_EMAIL` | `false`, `0`, `no`, or `off` disables the check |
///
/// A client role is keyed `"<client>:<role>"`.
///
/// # Errors
///
/// Fails when the URL is set and anything else is missing or malformed. Treat that
/// as a boot failure: running on with Keycloak off would refuse every Keycloak
/// caller and hide the reason.
pub fn verifier_from_lookup(
    prefix: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<KeycloakVerifier>, VerifyError> {
    settings_from_lookup(prefix, lookup)?
        .map(KeycloakVerifier::new)
        .transpose()
}
