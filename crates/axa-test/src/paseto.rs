//! PASETO tokens for tests, signed under a throwaway key, for a service whose guard
//! is `axa-paseto-auth`.
//!
//! Build the guard's verifier from [`TestKeys::verifier`] in the service's test boot,
//! then mint a token per caller:
//!
//! ```
//! use axa_test::paseto::TestKeys;
//!
//! let keys = TestKeys::new();
//! let verifier = keys.verifier("authentication-service", "todo-service");
//! let acme = keys
//!     .token("authentication-service", "todo-service")
//!     .attr("tenant", ["acme"])
//!     .attr("access", ["write"])
//!     .sign();
//! assert!(verifier.verify(&acme).is_ok());
//! ```
//!
//! **Never use these keys outside tests.** The seed is in this source, so anyone could
//! mint a token they verify. A verifier from [`TestKeys`] trusts only its own key, so a
//! service built with a real verifier never accepts these tokens.

use std::collections::BTreeMap;

use authentication_verifier::Verifier;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::SigningKey;
use rusty_paseto::core::{Footer, Key, Paseto, PasetoAsymmetricPrivateKey, Payload, Public, V4};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The seed [`TestKeys::new`] signs with. It is public, and protects nothing.
const TEST_SEED: [u8; 32] = [42; 32];

/// `iat` for every test token: 14 November 2023.
const ISSUED_AT: i64 = 1_700_000_000;

/// `exp` for a token that does not expire within any test run.
const FAR_FUTURE: i64 = ISSUED_AT + 10_000_000_000;

/// A throwaway Ed25519 signing key, its key set, and tokens it signs.
#[derive(Clone)]
pub struct TestKeys {
    seed: [u8; 32],
}

impl std::fmt::Debug for TestKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestKeys")
            .field("kid", &self.kid())
            .finish_non_exhaustive()
    }
}

impl Default for TestKeys {
    fn default() -> Self {
        Self::new()
    }
}

impl TestKeys {
    /// The test key every call shares.
    #[must_use]
    pub fn new() -> Self {
        Self { seed: TEST_SEED }
    }

    /// A key from `seed`, such as a second key to show that a token signed by another
    /// key is refused.
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { seed }
    }

    fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.seed)
    }

    /// The key's id, as the auth service derives it: the SHA-256 of the public key,
    /// in unpadded URL-safe base64.
    #[must_use]
    pub fn kid(&self) -> String {
        let public = self.signing_key().verifying_key().to_bytes();
        URL_SAFE_NO_PAD.encode(Sha256::digest(public))
    }

    /// The public key set, as the auth service publishes it, with this one key.
    #[must_use]
    pub fn key_set(&self) -> Value {
        let public = self.signing_key().verifying_key().to_bytes();
        json!({
            "keys": [{
                "kty": "OKP", "crv": "Ed25519", "use": "sig",
                "kid": self.kid(), "x": URL_SAFE_NO_PAD.encode(public),
            }]
        })
    }

    /// A verifier that trusts this key only, for tokens from `issuer` to `audience`.
    ///
    /// # Panics
    ///
    /// Never, as the key set it builds is always well formed.
    #[must_use]
    pub fn verifier(&self, issuer: &str, audience: &str) -> Verifier {
        Verifier::from_paseto_keys_value(&self.key_set(), issuer, audience)
            .expect("a test key set is well formed")
    }

    /// A token from `issuer` to `audience`, signed by this key, for a caller with no
    /// attributes until [`TestToken::attr`] adds some.
    #[must_use]
    pub fn token(&self, issuer: &str, audience: &str) -> TestToken {
        TestToken {
            keys: self.clone(),
            sub: "11111111-1111-1111-1111-111111111111".to_string(),
            email: "test@example.com".to_string(),
            name: "Test Caller".to_string(),
            iss: issuer.to_string(),
            aud: audience.to_string(),
            exp: FAR_FUTURE,
            sid: "test-session".to_string(),
            attrs: BTreeMap::new(),
        }
    }
}

/// A token being built, signed by [`TestToken::sign`].
#[derive(Debug, Clone)]
pub struct TestToken {
    keys: TestKeys,
    sub: String,
    email: String,
    name: String,
    iss: String,
    aud: String,
    exp: i64,
    sid: String,
    attrs: BTreeMap<String, Vec<String>>,
}

impl TestToken {
    /// Set `name` in the caller's `attrs` to `values`, such as `("tenant", ["acme"])`
    /// or `("access", ["write"])`.
    #[must_use]
    pub fn attr<I, S>(mut self, name: &str, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.attrs.insert(
            name.to_string(),
            values.into_iter().map(Into::into).collect(),
        );
        self
    }

    /// The caller's subject, their stable id.
    #[must_use]
    pub fn subject(mut self, sub: &str) -> Self {
        self.sub = sub.to_string();
        self
    }

    /// The caller's email.
    #[must_use]
    pub fn email(mut self, email: &str) -> Self {
        self.email = email.to_string();
        self
    }

    /// Expire at `exp`, in Unix seconds, such as a time in the past to show that an
    /// expired token is refused.
    #[must_use]
    pub fn expires_at(mut self, exp: i64) -> Self {
        self.exp = exp;
        self
    }

    /// The signed token, to send as `authorization: Bearer <token>`.
    ///
    /// # Panics
    ///
    /// Never, as every claim is plain JSON and the key is always valid.
    #[must_use]
    pub fn sign(&self) -> String {
        let payload = json!({
            "sub": self.sub, "email": self.email, "name": self.name,
            "iss": self.iss, "aud": self.aud,
            "exp": self.exp, "iat": ISSUED_AT, "sid": self.sid,
            "attrs": self.attrs,
        })
        .to_string();
        let key = Key::<64>::from(self.keys.signing_key().to_keypair_bytes());
        let private = PasetoAsymmetricPrivateKey::<V4, Public>::from(&key);
        let footer = json!({ "kid": self.keys.kid() }).to_string();
        let mut builder = Paseto::<V4, Public>::builder();
        builder.set_payload(Payload::from(payload.as_str()));
        builder.set_footer(Footer::from(footer.as_str()));
        builder.try_sign(&private).expect("a test token signs")
    }

    /// The `authorization` header value for this token.
    #[must_use]
    pub fn bearer(&self) -> String {
        format!("Bearer {}", self.sign())
    }
}
