//! A local OIDC provider for tests.
//!
//! It serves a Keycloak-shaped discovery document and JWKS on an ephemeral loopback
//! port, and mints RS256 access tokens, so a token goes through the real checks
//! without a Keycloak container.
//!
//! Copied from `authentication-verifier` 0.9.1, `src/test_idp.rs`. `test-key.der` is
//! the same throwaway key, which protects nothing. It is DER rather than PEM so that
//! the secret scan stays meaningful.

use std::sync::atomic::{AtomicUsize, Ordering};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Realm the test `IdP` serves.
pub const REALM: &str = "mxi";
/// Audience the default test tokens carry.
pub const AUDIENCE: &str = "mxi-api";

/// A throwaway RSA key generated for tests; it protects nothing.
const PRIVATE_KEY_DER: &[u8] = include_bytes!("test-key.der");
/// The matching public modulus, base64url.
const JWK_N: &str = "1q1nDig-3n168wojWGD6_iPY3KRH8xodiETRGGYORY1CcSY4KAqE58xeZXXkd_23K_WyF9uPgPtX4C5NamjPS8UnaCYk-_ck0-GIlsAzEvdCXBA6ORV0SyWthx8aBwoac0KMBJr0j8P0tU_GB3_PQ_bmeNE2zbwRqyBHQYfgzRWFYxWvyVlKgqIhAdkT55t6MBb5WPODdD0tSeycde7YBQB1OP0YMkjs6nPXDCbOaqBf5lMrkLe-PW8oHLuEdqR1StgOqp29Gh9fM1ae492xvOSdOUQ6h00pd8XGbcJMwUqF3RUL-BeMjeUAWHm7RzEBOLnn5cBm0C-JUD9iWsUf9Q";

static JTI: AtomicUsize = AtomicUsize::new(0);

/// A running local OIDC provider.
#[derive(Debug, Clone)]
pub struct TestIdp {
    /// Base URL to give `KeycloakSettings::url`, e.g. `http://127.0.0.1:41234`.
    pub base_url: String,
}

fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

impl TestIdp {
    /// Start the provider on an ephemeral loopback port. It serves until
    /// the Tokio runtime shuts down.
    ///
    /// # Panics
    ///
    /// If it cannot bind a loopback port (a test environment failure).
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let base = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().expect("local addr").port()
        );
        let discovery = json!({
            "issuer": format!("{base}/realms/{REALM}"),
            "authorization_endpoint": format!("{base}/auth"),
            "token_endpoint": format!("{base}/token"),
            "jwks_uri": format!("{base}/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"],
        })
        .to_string();
        let jwks = json!({ "keys": [{
            "kty": "RSA", "alg": "RS256", "use": "sig", "kid": "k1", "n": JWK_N, "e": "AQAB"
        }]})
        .to_string();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (discovery, jwks) = (discovery.clone(), jwks.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (status, body) = if path.ends_with("/openid-configuration") {
                        ("200 OK", discovery)
                    } else if path == "/certs" {
                        ("200 OK", jwks)
                    } else {
                        ("404 Not Found", "{}".to_string())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        Self { base_url: base }
    }

    /// The issuer this `IdP`'s tokens carry: `{base}/realms/mxi`.
    #[must_use]
    pub fn issuer(&self) -> String {
        format!("{}/realms/{REALM}", self.base_url)
    }

    /// Mint an RS256 access token for a verified user with realm roles
    /// `editor` and `offline_access`, valid for five minutes. `tweak` edits
    /// the claim object before signing (e.g. change `aud`, `exp`, or
    /// `realm_access.roles`).
    ///
    /// # Panics
    ///
    /// If signing fails (the embedded key is valid, so it does not).
    pub fn token(&self, tweak: impl FnOnce(&mut Value)) -> String {
        let mut claims = json!({
            "iss": self.issuer(),
            "aud": [AUDIENCE],
            "sub": "0c4f1e2a-0000-4000-8000-000000000001",
            "azp": "mxi-web",
            "typ": "Bearer",
            "jti": format!("jti-{}", JTI.fetch_add(1, Ordering::Relaxed)),
            "iat": now() - 10,
            "exp": now() + 300,
            "email": "ada@example.com",
            "email_verified": true,
            "name": "Ada",
            "preferred_username": "ada",
            "sid": "sess-1",
            "realm_access": { "roles": ["editor", "offline_access"] },
        });
        tweak(&mut claims);
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("k1".into());
        jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_der(PRIVATE_KEY_DER),
        )
        .expect("sign test token")
    }
}
