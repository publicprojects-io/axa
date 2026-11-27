//! A guard for operator routes, such as an admin page or a debugging endpoint, that
//! takes one static credential rather than a signed-in user.
//!
//! [`Operator`] holds bearer tokens or Basic credentials, from a secret store, and
//! [`protect`] refuses every request to a route under it that does not send one of them,
//! with a `401` problem and a `WWW-Authenticate` header for each scheme. The
//! credential sent is compared in constant time, and logs never see it.
//!
//! ```
//! use axa::operator::{Operator, protect};
//! use axum::{Router, routing::get};
//!
//! # fn run() -> axa::Result<()> {
//! let token = "a-token-of-at-least-32-bytes-from-the-secret-store";
//! let operator = Operator::bearer(token)?;
//! let ops: Router = protect(Router::new().route("/queue", get(|| async { "ok" })), &operator);
//! let app: Router = Router::new().nest("/ops", ops);
//! # Ok(())
//! # }
//! ```
//!
//! Give each operator, or each tool, its own credential, and add the new one with
//! [`Operator::or`] while an old one is retired.

use std::fmt;

use axum::{
    Router,
    extract::Request,
    http::{
        HeaderValue,
        header::{AUTHORIZATION, WWW_AUTHENTICATE},
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
};

use crate::{Error, Result};

/// The fewest bytes a bearer token may have.
pub const MIN_TOKEN_BYTES: usize = 32;

/// The fewest bytes a Basic password may have.
pub const MIN_PASSWORD_BYTES: usize = 16;

/// The longest credential accepted, in bytes, so a check costs little.
const MAX_CREDENTIAL_BYTES: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Bearer,
    Basic,
}

impl Scheme {
    const fn name(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::Basic => "basic",
        }
    }

    const fn challenge(self) -> &'static str {
        match self {
            Self::Bearer => "Bearer",
            Self::Basic => "Basic realm=\"operator\", charset=\"UTF-8\"",
        }
    }
}

#[derive(Clone)]
struct Credential {
    scheme: Scheme,
    /// What follows the scheme in the `authorization` header.
    expected: Vec<u8>,
}

/// The credentials an operator route accepts.
#[derive(Clone)]
pub struct Operator {
    credentials: Vec<Credential>,
}

impl fmt::Debug for Operator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // How many of each, and never what they are.
        let count = |scheme| {
            self.credentials
                .iter()
                .filter(|held| held.scheme == scheme)
                .count()
        };
        f.debug_struct("Operator")
            .field("bearer", &count(Scheme::Bearer))
            .field("basic", &count(Scheme::Basic))
            .finish()
    }
}

fn refuse(message: &str) -> Error {
    Error::string(&format!("operator credential: {message}"))
}

/// Whether every byte is a visible ASCII character, as a header value allows.
fn visible(text: &str) -> bool {
    text.bytes().all(|byte| byte.is_ascii_graphic())
}

impl Operator {
    /// Accept `authorization: Bearer <token>`.
    ///
    /// # Errors
    ///
    /// Fails when the token is under [`MIN_TOKEN_BYTES`], over 1024 bytes, or has a
    /// byte that is not visible ASCII.
    pub fn bearer(token: &str) -> Result<Self> {
        if token.len() < MIN_TOKEN_BYTES {
            return Err(refuse(&format!(
                "a bearer token needs at least {MIN_TOKEN_BYTES} bytes"
            )));
        }
        if token.len() > MAX_CREDENTIAL_BYTES || !visible(token) {
            return Err(refuse(
                "a bearer token must be at most 1024 visible ASCII characters",
            ));
        }
        Ok(Self {
            credentials: vec![Credential {
                scheme: Scheme::Bearer,
                expected: token.as_bytes().to_vec(),
            }],
        })
    }

    /// Accept `authorization: Basic` with this user and password.
    ///
    /// # Errors
    ///
    /// Fails when the user is empty or holds a `:`, the password is under
    /// [`MIN_PASSWORD_BYTES`], or either has a control character.
    pub fn basic(user: &str, password: &str) -> Result<Self> {
        if user.is_empty() || user.contains(':') {
            return Err(refuse("a Basic user must be set and hold no colon"));
        }
        if password.len() < MIN_PASSWORD_BYTES {
            return Err(refuse(&format!(
                "a Basic password needs at least {MIN_PASSWORD_BYTES} bytes"
            )));
        }
        if format!("{user}{password}").chars().any(char::is_control) {
            return Err(refuse("a Basic user or password holds a control character"));
        }
        let expected = encode_base64(format!("{user}:{password}").as_bytes());
        if expected.len() > MAX_CREDENTIAL_BYTES {
            return Err(refuse("a Basic credential is too long"));
        }
        Ok(Self {
            credentials: vec![Credential {
                scheme: Scheme::Basic,
                expected: expected.into_bytes(),
            }],
        })
    }

    /// Accept `other`'s credentials as well as these.
    #[must_use]
    pub fn or(mut self, other: Self) -> Self {
        self.credentials.extend(other.credentials);
        self
    }

    /// Whether an `authorization` header value is one of these credentials. Every
    /// credential is compared, each in constant time, so timing shows neither which
    /// one matched nor how much of one did.
    #[must_use]
    pub fn accepts(&self, authorization: &[u8]) -> bool {
        let text = String::from_utf8_lossy(authorization);
        let Some((scheme, sent)) = text.split_once(' ') else {
            return false;
        };
        let sent = sent.trim_start_matches(' ').as_bytes();
        let mut accepted = false;
        for held in &self.credentials {
            let same =
                scheme.eq_ignore_ascii_case(held.scheme.name()) & same_bytes(&held.expected, sent);
            accepted |= same;
        }
        accepted
    }

    fn challenges(&self) -> Vec<&'static str> {
        let mut challenges: Vec<_> = self
            .credentials
            .iter()
            .map(|held| held.scheme.challenge())
            .collect();
        challenges.dedup();
        challenges
    }
}

/// Compare in time that depends only on the length of `expected`.
fn same_bytes(expected: &[u8], sent: &[u8]) -> bool {
    let mut difference = u8::from(expected.len() != sent.len());
    for (index, byte) in expected.iter().enumerate() {
        difference |= byte ^ sent.get(index).copied().unwrap_or(0);
    }
    difference == 0
}

/// Standard base64 with padding, as a client encodes Basic credentials.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "RCA-EXC-2: the chunk index is at most 3, so the shifts are at most 18 of 32 bits"
)]
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3).checked_mul(4).unwrap_or(0)); // ARI-9
    for chunk in bytes.chunks(3) {
        let triple = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |held, (index, &byte)| {
                held | (u32::from(byte) << (16 - 8 * index))
            });
        for index in 0..4 {
            if index <= chunk.len() {
                let six = (triple >> (18 - 6 * index)) & 0x3f;
                if let Some(&letter) = usize::try_from(six).ok().and_then(|at| ALPHABET.get(at)) {
                    encoded.push(char::from(letter));
                }
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

/// Refuse every request to a route of `router` that does not send one of
/// `operator`'s credentials. A path under it that matches no route is answered by the
/// app it is nested in, as any unknown path is, with a `404` that says nothing of
/// which operator routes there are.
pub fn protect<S>(router: Router<S>, operator: &Operator) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let operator = operator.clone();
    router.layer(middleware::from_fn(move |request: Request, next: Next| {
        let operator = operator.clone();
        async move { check(&operator, request, next).await }
    }))
}

async fn check(operator: &Operator, request: Request, next: Next) -> Response {
    let sent = request
        .headers()
        .get(AUTHORIZATION)
        .map(HeaderValue::as_bytes);
    if sent.is_some_and(|sent| operator.accepts(sent)) {
        return next.run(request).await;
    }
    let reason = if sent.is_some() {
        "operator credential refused"
    } else {
        "operator credential missing"
    };
    let mut response = Error::Unauthorized(reason.to_string()).into_response();
    for challenge in operator.challenges() {
        response
            .headers_mut()
            .append(WWW_AUTHENTICATE, HeaderValue::from_static(challenge));
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        routing::get,
    };
    use tower::ServiceExt;

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn app(operator: &Operator) -> Router {
        let ops = protect(
            Router::new().route("/queue", get(|| async { "ok" })),
            operator,
        );
        Router::new()
            .route("/", get(|| async { "public" }))
            .nest("/ops", ops)
    }

    async fn call(app: &Router, path: &str, authorization: Option<&str>) -> Response {
        let mut request = Request::get(path);
        if let Some(value) = authorization {
            request = request.header(AUTHORIZATION, value);
        }
        app.clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn only_the_token_passes_and_the_rest_of_the_app_is_open() {
        let app = app(&Operator::bearer(TOKEN).unwrap());
        let passed = call(&app, "/ops/queue", Some(&format!("Bearer {TOKEN}"))).await;
        assert_eq!(passed.status(), StatusCode::OK);
        let lower = call(&app, "/ops/queue", Some(&format!("bearer {TOKEN}"))).await;
        assert_eq!(
            lower.status(),
            StatusCode::OK,
            "the scheme is not case sensitive"
        );
        assert_eq!(call(&app, "/", None).await.status(), StatusCode::OK);
        for sent in [
            None,
            Some("Bearer"),
            Some("Bearer 0123456789abcdef0123456789abcdeX"),
            Some("Bearer 0123456789abcdef0123456789abcdef0"),
            Some("Bearer 0123456789abcdef"),
            Some("Basic 0123456789abcdef0123456789abcdef"),
        ] {
            let refused = call(&app, "/ops/queue", sent).await;
            assert_eq!(refused.status(), StatusCode::UNAUTHORIZED, "{sent:?}");
            assert_eq!(refused.headers()[WWW_AUTHENTICATE], "Bearer", "{sent:?}");
            let body = to_bytes(refused.into_body(), 4096).await.unwrap();
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(body.contains("\"code\":\"unauthorized\""), "{body}");
            assert!(!body.contains("0123456789"), "{body}");
        }
    }

    #[tokio::test]
    async fn a_path_matching_no_operator_route_is_the_apps_404() {
        let app = app(&Operator::bearer(TOKEN).unwrap());
        let unknown = call(&app, "/ops/nothing-here", None).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        let known = call(&app, "/ops/queue", None).await;
        assert_eq!(known.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn basic_credentials_pass_and_both_schemes_are_offered() {
        let operator = Operator::basic("ops", "a-long-password-1")
            .unwrap()
            .or(Operator::bearer(TOKEN).unwrap());
        let app = app(&operator);
        let basic = format!("Basic {}", encode_base64(b"ops:a-long-password-1"));
        assert_eq!(
            call(&app, "/ops/queue", Some(&basic)).await.status(),
            StatusCode::OK
        );
        let bearer = format!("Bearer {TOKEN}");
        assert_eq!(
            call(&app, "/ops/queue", Some(&bearer)).await.status(),
            StatusCode::OK
        );
        let wrong = format!("Basic {}", encode_base64(b"ops:a-long-password-2"));
        let refused = call(&app, "/ops/queue", Some(&wrong)).await;
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        let offered: Vec<_> = refused.headers().get_all(WWW_AUTHENTICATE).iter().collect();
        assert_eq!(offered.len(), 2);
        assert!(offered[0].to_str().unwrap().starts_with("Basic realm="));
        assert_eq!(offered[1], "Bearer");
    }

    #[test]
    fn a_weak_credential_is_refused_at_boot() {
        assert!(Operator::bearer("short").is_err());
        assert!(Operator::bearer(&format!("{TOKEN} space")).is_err());
        assert!(Operator::bearer(&"x".repeat(2000)).is_err());
        assert!(Operator::basic("", "a-long-password-1").is_err());
        assert!(Operator::basic("o:ps", "a-long-password-1").is_err());
        assert!(Operator::basic("ops", "short").is_err());
        assert!(Operator::basic("ops", "a-long-password-\n").is_err());
    }

    #[test]
    fn debug_output_hides_the_credentials() {
        let operator =
            Operator::bearer(TOKEN)
                .unwrap()
                .or(Operator::basic("ops", "a-long-password-1").unwrap());
        let shown = format!("{operator:?}");
        assert_eq!(shown, "Operator { bearer: 1, basic: 1 }");
        // Uneven counts, so a count of the wrong scheme cannot read the same.
        let more = operator.or(Operator::bearer(&"b".repeat(MIN_TOKEN_BYTES)).unwrap());
        assert_eq!(format!("{more:?}"), "Operator { bearer: 2, basic: 1 }");
    }

    #[test]
    fn a_credential_at_each_length_limit_is_kept_and_one_past_it_is_refused() {
        assert!(Operator::bearer(&"t".repeat(MIN_TOKEN_BYTES)).is_ok());
        assert!(Operator::bearer(&"t".repeat(MIN_TOKEN_BYTES - 1)).is_err());
        assert!(Operator::bearer(&"t".repeat(MAX_CREDENTIAL_BYTES)).is_ok());
        assert!(Operator::bearer(&"t".repeat(MAX_CREDENTIAL_BYTES + 1)).is_err());
        assert!(Operator::basic("ops", &"p".repeat(MIN_PASSWORD_BYTES)).is_ok());
        assert!(Operator::basic("ops", &"p".repeat(MIN_PASSWORD_BYTES - 1)).is_err());
        // `ops:` and 764 bytes make 768, which base64 makes exactly 1024; one more is 1028.
        assert!(Operator::basic("ops", &"p".repeat(764)).is_ok());
        assert!(Operator::basic("ops", &"p".repeat(765)).is_err());
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(
            encode_base64(b"Aladdin:open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        assert_eq!(encode_base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn a_comparison_of_a_different_length_is_unequal() {
        assert!(same_bytes(b"abc", b"abc"));
        assert!(!same_bytes(b"abc", b"abcd"));
        assert!(!same_bytes(b"abc", b"ab"));
        assert!(!same_bytes(b"abc", b""));
    }
}
