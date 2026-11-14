//! Webhook signing and verifying, to the [Standard Webhooks] scheme.
//!
//! A sender signs `{id}.{timestamp}.{body}` and sends three headers: `webhook-id`,
//! `webhook-timestamp` in Unix seconds, and `webhook-signature`, which lists one
//! signature for each key it signs with, separated by spaces: `v1,` and base64 for
//! HMAC-SHA256, and `v1a,` and base64 for Ed25519. While a key rotates, the sender signs
//! with the old key and the new, and a receiver that knows either accepts it.
//!
//! ```
//! use axa_webhooks::{Signer, SigningKey, Verifier};
//!
//! let key: SigningKey = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".parse().unwrap();
//! let signer = Signer::new(key.clone());
//! let headers = signer.headers("msg_1", 1_700_000_000, br#"{"ok":true}"#).unwrap();
//!
//! let verifier = Verifier::new(key.verifying_key());
//! let delivery = verifier
//!     .verify_at(&headers, br#"{"ok":true}"#, 1_700_000_010)
//!     .unwrap();
//! assert_eq!(delivery.id, "msg_1");
//! assert!(verifier.verify_at(&headers, b"{}", 1_700_000_010).is_err());
//! ```
//!
//! A receiver takes [`Verified`] as an extractor, with the [`Verifier`] in its state,
//! and the handler runs only for a delivery signed by a key it knows, inside the
//! timestamp window. The window stops an old delivery being sent again later, but not
//! within it: a receiver that must act once per delivery keeps the ids it has seen for
//! as long as the window, as [`Delivery::id`] is the same on every retry.
//!
//! [Standard Webhooks]: https://www.standardwebhooks.com/

use std::{
    fmt,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axa_problem::Problem;
use axum::{
    body::Bytes,
    extract::{FromRef, FromRequest, Request},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer as _;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The header naming the delivery, the same on every retry.
pub const ID_HEADER: &str = "webhook-id";

/// The header holding when the delivery was signed, in Unix seconds.
pub const TIMESTAMP_HEADER: &str = "webhook-timestamp";

/// The header listing the signatures.
pub const SIGNATURE_HEADER: &str = "webhook-signature";

/// How far a delivery's timestamp may be from now, either way, unless a verifier says
/// otherwise: five minutes, as Standard Webhooks suggests.
pub const DEFAULT_TOLERANCE: Duration = Duration::from_mins(5);

/// The most signatures a delivery may list, so that one cannot make a receiver check
/// any number.
pub const MAX_SIGNATURES: usize = 16;

/// The longest id, in bytes.
pub const MAX_ID_BYTES: usize = 256;

/// The shortest and longest HMAC secret Standard Webhooks allows, in bytes.
const HMAC_KEY_BYTES: std::ops::RangeInclusive<usize> = 24..=64;

const HMAC_PREFIX: &str = "whsec_";
const SECRET_KEY_PREFIX: &str = "whsk_";
const PUBLIC_KEY_PREFIX: &str = "whpk_";

/// Why a key did not parse. It never quotes the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyError {
    /// It does not start with a prefix this kind of key may have.
    Prefix,
    /// What follows the prefix is not base64.
    Base64,
    /// It is not a length this kind of key has.
    Length,
    /// It is not an Ed25519 key, or a secret key's public half does not match it.
    Ed25519,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Prefix => "a webhook key starts with whsec_, whsk_, or whpk_",
            Self::Base64 => "a webhook key is base64 after its prefix",
            Self::Length => {
                "a whsec_ key holds 24 to 64 bytes, a whsk_ key 32 or 64, and a whpk_ key 32"
            }
            Self::Ed25519 => "a whsk_ or whpk_ key is not a valid Ed25519 key",
        })
    }
}

impl std::error::Error for KeyError {}

fn decode(text: &str) -> Result<Vec<u8>, KeyError> {
    STANDARD.decode(text.trim()).map_err(|_| KeyError::Base64)
}

fn hmac_key(text: &str) -> Result<Vec<u8>, KeyError> {
    let bytes = decode(text)?;
    if HMAC_KEY_BYTES.contains(&bytes.len()) {
        Ok(bytes)
    } else {
        Err(KeyError::Length)
    }
}

/// A key a sender signs with: an HMAC secret, `whsec_` and base64, or an Ed25519
/// secret key, `whsk_` and the base64 of its 32-byte seed, or of the seed and the public
/// key, 64 bytes.
#[derive(Clone)]
pub struct SigningKey(Signing);

#[derive(Clone)]
enum Signing {
    Hmac(Vec<u8>),
    Ed25519(ed25519_dalek::SigningKey),
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Signing::Hmac(_) => "SigningKey(hmac)",
            Signing::Ed25519(_) => "SigningKey(ed25519)",
        })
    }
}

impl FromStr for SigningKey {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Self, KeyError> {
        let text = text.trim();
        if let Some(rest) = text.strip_prefix(HMAC_PREFIX) {
            return Ok(Self(Signing::Hmac(hmac_key(rest)?)));
        }
        let rest = text
            .strip_prefix(SECRET_KEY_PREFIX)
            .ok_or(KeyError::Prefix)?;
        let bytes = decode(rest)?;
        let seed: [u8; 32] = bytes
            .get(..32)
            .filter(|_| matches!(bytes.len(), 32 | 64))
            .and_then(|seed| seed.try_into().ok())
            .ok_or(KeyError::Length)?;
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        if bytes.len() == 64 && bytes[32..] != key.verifying_key().to_bytes() {
            return Err(KeyError::Ed25519);
        }
        Ok(Self(Signing::Ed25519(key)))
    }
}

impl SigningKey {
    /// The key a receiver verifies with: the same secret for HMAC, and the public key
    /// for Ed25519.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        VerifyingKey(match &self.0 {
            Signing::Hmac(secret) => Verifying::Hmac(secret.clone()),
            Signing::Ed25519(key) => Verifying::Ed25519(key.verifying_key()),
        })
    }

    /// The Ed25519 public key as `whpk_` and base64, to hand to receivers, or `None`
    /// for an HMAC secret, which has no public half.
    #[must_use]
    pub fn public_key(&self) -> Option<String> {
        match &self.0 {
            Signing::Hmac(_) => None,
            Signing::Ed25519(key) => Some(format!(
                "{PUBLIC_KEY_PREFIX}{}",
                STANDARD.encode(key.verifying_key().to_bytes())
            )),
        }
    }

    /// This key's signature of `content`, as it is listed in the header.
    fn sign(&self, content: &[u8]) -> String {
        match &self.0 {
            Signing::Hmac(secret) => format!("v1,{}", STANDARD.encode(hmac(secret, content))),
            Signing::Ed25519(key) => {
                format!("v1a,{}", STANDARD.encode(key.sign(content).to_bytes()))
            }
        }
    }
}

/// A key a receiver verifies with: an HMAC secret, `whsec_` and base64, or an Ed25519
/// public key, `whpk_` and base64.
#[derive(Clone)]
pub struct VerifyingKey(Verifying);

#[derive(Clone)]
enum Verifying {
    Hmac(Vec<u8>),
    Ed25519(ed25519_dalek::VerifyingKey),
}

impl fmt::Debug for VerifyingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Verifying::Hmac(_) => "VerifyingKey(hmac)",
            Verifying::Ed25519(_) => "VerifyingKey(ed25519)",
        })
    }
}

impl FromStr for VerifyingKey {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Self, KeyError> {
        let text = text.trim();
        if let Some(rest) = text.strip_prefix(HMAC_PREFIX) {
            return Ok(Self(Verifying::Hmac(hmac_key(rest)?)));
        }
        let rest = text
            .strip_prefix(PUBLIC_KEY_PREFIX)
            .ok_or(KeyError::Prefix)?;
        let bytes: [u8; 32] = decode(rest)?.try_into().map_err(|_| KeyError::Length)?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|_| KeyError::Ed25519)?;
        Ok(Self(Verifying::Ed25519(key)))
    }
}

impl VerifyingKey {
    /// Whether `signature`, of the version it names, is this key's of `content`. An
    /// HMAC is compared in constant time.
    fn verifies(&self, version: &str, signature: &[u8], content: &[u8]) -> bool {
        match (&self.0, version) {
            (Verifying::Hmac(secret), "v1") => Hmac::<Sha256>::new_from_slice(secret)
                .is_ok_and(|mac| mac.chain_update(content).verify_slice(signature).is_ok()),
            (Verifying::Ed25519(key), "v1a") => ed25519_dalek::Signature::from_slice(signature)
                .is_ok_and(|signature| key.verify_strict(content, &signature).is_ok()),
            _ => false,
        }
    }
}

fn hmac(secret: &[u8], content: &[u8]) -> Vec<u8> {
    // Every key length is valid for HMAC, so this cannot fail.
    let mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.chain_update(content).finalize().into_bytes().to_vec()
}

/// What is signed: `{id}.{timestamp}.{body}`.
fn content(id: &str, timestamp: u64, body: &[u8]) -> Vec<u8> {
    let mut content = format!("{id}.{timestamp}.").into_bytes();
    content.extend_from_slice(body);
    content
}

/// Whether `id` may name a delivery: 1 to 256 bytes of visible ASCII.
fn is_id(id: &str) -> bool {
    (1..=MAX_ID_BYTES).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Seconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Why a delivery could not be signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignError {
    /// The id is not 1 to 256 bytes of visible ASCII.
    InvalidId,
}

impl fmt::Display for SignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a webhook id is 1 to 256 bytes of visible ASCII")
    }
}

impl std::error::Error for SignError {}

/// Signs deliveries with one key or more.
#[derive(Debug, Clone)]
pub struct Signer {
    keys: Vec<SigningKey>,
}

impl Signer {
    /// A signer with `key`.
    #[must_use]
    pub fn new(key: SigningKey) -> Self {
        Self { keys: vec![key] }
    }

    /// Sign with `key` as well, as while a key rotates.
    #[must_use]
    pub fn and(mut self, key: SigningKey) -> Self {
        self.keys.push(key);
        self
    }

    /// The `webhook-signature` value for the delivery `id` of `body` at `timestamp`.
    ///
    /// # Errors
    ///
    /// Fails when `id` is not 1 to 256 bytes of visible ASCII.
    pub fn signature(&self, id: &str, timestamp: u64, body: &[u8]) -> Result<String, SignError> {
        if !is_id(id) {
            return Err(SignError::InvalidId);
        }
        let content = content(id, timestamp, body);
        let signatures: Vec<String> = self.keys.iter().map(|key| key.sign(&content)).collect();
        Ok(signatures.join(" "))
    }

    /// The three headers for the delivery `id` of `body` at `timestamp`.
    ///
    /// # Errors
    ///
    /// As [`Signer::signature`].
    pub fn headers(&self, id: &str, timestamp: u64, body: &[u8]) -> Result<HeaderMap, SignError> {
        let signature = self.signature(id, timestamp, body)?;
        let value = |text: &str| HeaderValue::from_str(text).map_err(|_| SignError::InvalidId);
        let mut headers = HeaderMap::new();
        headers.insert(ID_HEADER, value(id)?);
        headers.insert(TIMESTAMP_HEADER, HeaderValue::from(timestamp));
        headers.insert(SIGNATURE_HEADER, value(&signature)?);
        Ok(headers)
    }

    /// [`Signer::headers`] at this moment.
    ///
    /// # Errors
    ///
    /// As [`Signer::signature`].
    pub fn headers_now(&self, id: &str, body: &[u8]) -> Result<HeaderMap, SignError> {
        self.headers(id, now(), body)
    }
}

/// A delivery that verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// Its id, the same on every retry of it.
    pub id: String,
    /// When it was signed, in Unix seconds.
    pub timestamp: u64,
}

/// Why a delivery was refused. As a response it is a problem: `400`, code
/// `webhook_headers_invalid`, for a header that is missing or malformed, and `401` for
/// the rest, with a `detail` that quotes nothing sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// A header is missing.
    MissingHeader(&'static str),
    /// A header is not what Standard Webhooks says it is.
    MalformedHeader(&'static str),
    /// The timestamp is further from now than the verifier's tolerance. Code
    /// `webhook_timestamp_out_of_range`.
    OutOfWindow,
    /// No signature listed is one of the verifier's keys' for this delivery. Code
    /// `webhook_signature_invalid`.
    NoValidSignature,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader(name) => write!(f, "the {name} header is missing"),
            Self::MalformedHeader(name) => write!(f, "the {name} header is malformed"),
            Self::OutOfWindow => f.write_str("the webhook timestamp is too far from now"),
            Self::NoValidSignature => f.write_str("no webhook signature is valid"),
        }
    }
}

impl std::error::Error for Refusal {}

impl Refusal {
    /// The status it answers with.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::MissingHeader(_) | Self::MalformedHeader(_) => StatusCode::BAD_REQUEST,
            Self::OutOfWindow | Self::NoValidSignature => StatusCode::UNAUTHORIZED,
        }
    }

    /// The problem's `code`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingHeader(_) | Self::MalformedHeader(_) => "webhook_headers_invalid",
            Self::OutOfWindow => "webhook_timestamp_out_of_range",
            Self::NoValidSignature => "webhook_signature_invalid",
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        Problem::new(self.status())
            .with_detail(self.to_string())
            .with_extension("code", self.code().into())
            .into_response()
    }
}

/// Verifies deliveries against one key or more.
#[derive(Debug, Clone)]
pub struct Verifier {
    keys: Vec<VerifyingKey>,
    tolerance: Duration,
}

impl Verifier {
    /// A verifier with `key`, and the [`DEFAULT_TOLERANCE`].
    #[must_use]
    pub fn new(key: VerifyingKey) -> Self {
        Self {
            keys: vec![key],
            tolerance: DEFAULT_TOLERANCE,
        }
    }

    /// Accept a signature by `key` as well, as while a key rotates.
    #[must_use]
    pub fn and(mut self, key: VerifyingKey) -> Self {
        self.keys.push(key);
        self
    }

    /// How far a timestamp may be from now, either way.
    #[must_use]
    pub fn tolerance(mut self, tolerance: Duration) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Verify the delivery in `headers` of `body`, now.
    ///
    /// # Errors
    ///
    /// As [`Verifier::verify_at`].
    pub fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<Delivery, Refusal> {
        self.verify_at(headers, body, now())
    }

    /// Verify the delivery in `headers` of `body`, as at `now`, in Unix seconds.
    ///
    /// # Errors
    ///
    /// Refuses a missing or malformed header, a timestamp further from `now` than the
    /// tolerance, and a delivery where no signature listed, of up to
    /// [`MAX_SIGNATURES`], is one of this verifier's keys'. A signature of a version it
    /// does not know is passed over, as Standard Webhooks says.
    pub fn verify_at(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        now: u64,
    ) -> Result<Delivery, Refusal> {
        let header = |name: &'static str| {
            headers
                .get(name)
                .ok_or(Refusal::MissingHeader(name))?
                .to_str()
                .map_err(|_| Refusal::MalformedHeader(name))
        };
        let id = header(ID_HEADER)?;
        if !is_id(id) {
            return Err(Refusal::MalformedHeader(ID_HEADER));
        }
        let timestamp = header(TIMESTAMP_HEADER)?;
        let timestamp: u64 = timestamp
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| timestamp.parse().ok())
            .flatten()
            .ok_or(Refusal::MalformedHeader(TIMESTAMP_HEADER))?;
        if timestamp.abs_diff(now) > self.tolerance.as_secs() {
            return Err(Refusal::OutOfWindow);
        }
        let listed: Vec<&str> = header(SIGNATURE_HEADER)?.split_ascii_whitespace().collect();
        if listed.is_empty() || listed.len() > MAX_SIGNATURES {
            return Err(Refusal::MalformedHeader(SIGNATURE_HEADER));
        }
        let content = content(id, timestamp, body);
        let verified = listed.iter().any(|entry| {
            let Some((version, encoded)) = entry.split_once(',') else {
                return false;
            };
            let Ok(signature) = STANDARD.decode(encoded) else {
                return false;
            };
            self.keys
                .iter()
                .any(|key| key.verifies(version, &signature, &content))
        });
        if !verified {
            return Err(Refusal::NoValidSignature);
        }
        Ok(Delivery {
            id: id.to_string(),
            timestamp,
        })
    }
}

/// An extractor for a verified delivery: the handler runs only when the [`Verifier`]
/// in the state verifies the request's headers and body, which it holds as bytes for
/// the handler to read, as with `serde_json::from_slice`. A refusal answers as
/// [`Refusal`] does, and the service's body limit applies as ever.
#[derive(Debug, Clone)]
pub struct Verified {
    /// The delivery.
    pub delivery: Delivery,
    /// The body, exactly as it was signed.
    pub body: Bytes,
}

impl<S> FromRequest<S> for Verified
where
    Verifier: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Response> {
        let verifier = Verifier::from_ref(state);
        let headers = request.headers().clone();
        let body = Bytes::from_request(request, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let delivery = verifier
            .verify(&headers, &body)
            .map_err(IntoResponse::into_response)?;
        Ok(Self { delivery, body })
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, routing::post};
    use tower::ServiceExt;

    use super::*;

    /// The secret from Standard Webhooks' own test vector.
    const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";

    fn key(text: &str) -> SigningKey {
        text.parse().unwrap()
    }

    fn ed25519(seed: u8) -> SigningKey {
        key(&format!(
            "{SECRET_KEY_PREFIX}{}",
            STANDARD.encode([seed; 32])
        ))
    }

    #[test]
    fn it_matches_the_standard_webhooks_test_vector() {
        let signer = Signer::new(key(SECRET));
        let signature = signer
            .signature(
                "msg_p5jXN8AQM9LWM0D4loKWxJek",
                1_614_265_330,
                br#"{"test": 2432232314}"#,
            )
            .unwrap();
        assert_eq!(signature, "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=");
    }

    #[test]
    fn a_delivery_verifies_with_either_kind_of_key() {
        for key in [key(SECRET), ed25519(7)] {
            let headers = Signer::new(key.clone())
                .headers("msg_1", 1_000, b"body")
                .unwrap();
            let verifier = Verifier::new(key.verifying_key());
            let delivery = verifier.verify_at(&headers, b"body", 1_000).unwrap();
            assert_eq!(
                delivery,
                Delivery {
                    id: "msg_1".into(),
                    timestamp: 1_000
                }
            );
            let public = key
                .public_key()
                .map(|text| text.parse::<VerifyingKey>().unwrap());
            if let Some(public) = public {
                assert!(
                    Verifier::new(public)
                        .verify_at(&headers, b"body", 1_000)
                        .is_ok()
                );
            }
        }
    }

    #[test]
    fn any_change_to_what_was_signed_is_refused() {
        let signer = Signer::new(key(SECRET));
        let verifier = Verifier::new(key(SECRET).verifying_key());
        let headers = signer.headers("msg_1", 1_000, b"body").unwrap();
        assert_eq!(
            verifier.verify_at(&headers, b"bodY", 1_000),
            Err(Refusal::NoValidSignature)
        );
        for (name, value) in [(ID_HEADER, "msg_2"), (TIMESTAMP_HEADER, "1001")] {
            let mut changed = headers.clone();
            changed.insert(name, HeaderValue::from_static(value));
            assert_eq!(
                verifier.verify_at(&changed, b"body", 1_000),
                Err(Refusal::NoValidSignature),
                "{name}"
            );
        }
        let other = Verifier::new(ed25519(1).verifying_key());
        assert_eq!(
            other.verify_at(&headers, b"body", 1_000),
            Err(Refusal::NoValidSignature)
        );
    }

    #[test]
    fn while_a_key_rotates_either_key_verifies() {
        let (old, new) = (key(SECRET), ed25519(9));
        let headers = Signer::new(old.clone())
            .and(new.clone())
            .headers("msg_1", 1_000, b"body")
            .unwrap();
        let listed = headers[SIGNATURE_HEADER].to_str().unwrap();
        assert!(
            listed.starts_with("v1,") && listed.contains(" v1a,"),
            "{listed}"
        );
        for key in [&old, &new] {
            let verifier = Verifier::new(key.verifying_key());
            assert!(verifier.verify_at(&headers, b"body", 1_000).is_ok());
        }
        let only_old = Signer::new(old).headers("msg_1", 1_000, b"body").unwrap();
        let both = Verifier::new(new.verifying_key()).and(key(SECRET).verifying_key());
        assert!(both.verify_at(&only_old, b"body", 1_000).is_ok());
    }

    #[test]
    fn a_timestamp_outside_the_window_is_refused_either_way() {
        let headers = Signer::new(key(SECRET))
            .headers("msg_1", 1_000, b"")
            .unwrap();
        let verifier = Verifier::new(key(SECRET).verifying_key()).tolerance(Duration::from_mins(1));
        assert!(verifier.verify_at(&headers, b"", 1_060).is_ok());
        assert!(verifier.verify_at(&headers, b"", 940).is_ok());
        for now in [1_061, 939] {
            assert_eq!(
                verifier.verify_at(&headers, b"", now),
                Err(Refusal::OutOfWindow)
            );
        }
    }

    #[test]
    fn malformed_headers_are_refused_before_any_signature_is_checked() {
        let verifier = Verifier::new(key(SECRET).verifying_key());
        let good = Signer::new(key(SECRET))
            .headers("msg_1", 1_000, b"")
            .unwrap();
        let too_many = vec!["v1,AAAA"; MAX_SIGNATURES + 1].join(" ");
        let cases = [
            (ID_HEADER, None, Refusal::MissingHeader(ID_HEADER)),
            (
                ID_HEADER,
                Some("has space"),
                Refusal::MalformedHeader(ID_HEADER),
            ),
            (
                TIMESTAMP_HEADER,
                None,
                Refusal::MissingHeader(TIMESTAMP_HEADER),
            ),
            (
                TIMESTAMP_HEADER,
                Some("+1000"),
                Refusal::MalformedHeader(TIMESTAMP_HEADER),
            ),
            (
                TIMESTAMP_HEADER,
                Some("1e3"),
                Refusal::MalformedHeader(TIMESTAMP_HEADER),
            ),
            (
                SIGNATURE_HEADER,
                None,
                Refusal::MissingHeader(SIGNATURE_HEADER),
            ),
            (
                SIGNATURE_HEADER,
                Some("  "),
                Refusal::MalformedHeader(SIGNATURE_HEADER),
            ),
            (
                SIGNATURE_HEADER,
                Some(too_many.as_str()),
                Refusal::MalformedHeader(SIGNATURE_HEADER),
            ),
        ];
        for (name, value, refusal) in cases {
            let mut headers = good.clone();
            match value {
                Some(value) => headers.insert(name, HeaderValue::from_str(value).unwrap()),
                None => headers.remove(name),
            };
            assert_eq!(
                verifier.verify_at(&headers, b"", 1_000),
                Err(refusal),
                "{name} {value:?}"
            );
        }
    }

    #[test]
    fn an_unknown_version_or_undecodable_signature_is_passed_over() {
        let verifier = Verifier::new(key(SECRET).verifying_key());
        let mut headers = Signer::new(key(SECRET))
            .headers("msg_1", 1_000, b"")
            .unwrap();
        let real = headers[SIGNATURE_HEADER].to_str().unwrap().to_string();
        let listed = format!("v2,AAAA v1,!!! nonsense v1a,AAAA {real}");
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&listed).unwrap());
        assert!(verifier.verify_at(&headers, b"", 1_000).is_ok());
        // An HMAC signature listed as Ed25519 is not accepted.
        let swapped = real.replacen("v1,", "v1a,", 1);
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_str(&swapped).unwrap());
        assert_eq!(
            verifier.verify_at(&headers, b"", 1_000),
            Err(Refusal::NoValidSignature)
        );
    }

    #[test]
    fn keys_parse_strictly_and_never_print() {
        let secret_bytes = STANDARD.encode([1; 20]);
        for (text, error) in [
            ("MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw", KeyError::Prefix),
            ("whsec_!!!", KeyError::Base64),
            (&format!("whsec_{secret_bytes}") as &str, KeyError::Length),
            (
                &format!("whsk_{}", STANDARD.encode([1; 31])),
                KeyError::Length,
            ),
            (
                &format!("whsk_{}", STANDARD.encode([1; 64])),
                KeyError::Ed25519,
            ),
        ] {
            assert_eq!(text.parse::<SigningKey>().unwrap_err(), error, "{text}");
        }
        let whole = ed25519(3);
        let Signing::Ed25519(inner) = &whole.0 else {
            unreachable!()
        };
        let mut both = inner.to_bytes().to_vec();
        both.extend_from_slice(&inner.verifying_key().to_bytes());
        assert!(
            format!("whsk_{}", STANDARD.encode(&both))
                .parse::<SigningKey>()
                .is_ok()
        );
        assert_eq!(
            format!("whpk_{}", STANDARD.encode([1; 31]))
                .parse::<VerifyingKey>()
                .unwrap_err(),
            KeyError::Length
        );
        assert_eq!(key(SECRET).public_key(), None);
        let shown = format!("{:?} {:?} {:?}", key(SECRET), whole, whole.verifying_key());
        assert_eq!(
            shown,
            "SigningKey(hmac) SigningKey(ed25519) VerifyingKey(ed25519)"
        );
    }

    #[test]
    fn an_id_that_is_not_visible_ascii_cannot_be_signed() {
        let signer = Signer::new(key(SECRET));
        for id in ["", "a b", "naïve", &"x".repeat(MAX_ID_BYTES + 1)] {
            assert_eq!(
                signer.signature(id, 1, b""),
                Err(SignError::InvalidId),
                "{id}"
            );
        }
        assert!(signer.signature(&"x".repeat(MAX_ID_BYTES), 1, b"").is_ok());
    }

    fn receiver() -> Router {
        Router::new()
            .route(
                "/hooks",
                post(|Verified { delivery, body }: Verified| async move {
                    format!("{} {}", delivery.id, String::from_utf8_lossy(&body))
                }),
            )
            .with_state(Verifier::new(key(SECRET).verifying_key()))
    }

    async fn deliver(headers: HeaderMap, body: &'static str) -> (StatusCode, String) {
        let mut request = Request::post("/hooks").body(Body::from(body)).unwrap();
        *request.headers_mut() = headers;
        let response = receiver().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn the_extractor_runs_the_handler_only_for_a_verified_delivery() {
        let signer = Signer::new(key(SECRET));
        let headers = signer.headers_now("msg_1", br#"{"a":1}"#).unwrap();
        assert_eq!(
            deliver(headers.clone(), r#"{"a":1}"#).await,
            (StatusCode::OK, r#"msg_1 {"a":1}"#.to_string())
        );

        let (status, body) = deliver(headers, r#"{"a":"secret"}"#).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            body.contains(r#""code":"webhook_signature_invalid""#),
            "{body}"
        );
        assert!(!body.contains("secret"), "{body}");

        let (status, body) = deliver(HeaderMap::new(), "{}").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body.contains(r#""code":"webhook_headers_invalid""#),
            "{body}"
        );

        let old = signer.headers("msg_1", now() - DEFAULT_TOLERANCE.as_secs() - 1, b"{}");
        let (status, body) = deliver(old.unwrap(), "{}").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            body.contains(r#""code":"webhook_timestamp_out_of_range""#),
            "{body}"
        );
    }
}
