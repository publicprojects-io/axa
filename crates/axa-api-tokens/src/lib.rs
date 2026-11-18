//! Opaque API tokens: a prefix, an id, and a secret kept only as its SHA-256 hash, with
//! scopes, an expiry, and the time of last use.
//!
//! A token reads `crm_<id>_<secret>`. The prefix names the service, so a secret
//! scanner can find a leaked one. The id is the row's key. The secret, 62 hex
//! digits holding 244 random bits, is shown once, when the token is issued, and only its hash is stored, so
//! a copy of the table lets no one in.
//!
//! ```
//! use std::time::{Duration, SystemTime};
//! use axa_api_tokens::{Prefix, Record, Scope, issue, parse_bearer};
//!
//! let prefix = Prefix::new("crm").unwrap();
//! let issued = issue(&prefix);
//! // Show `issued.token()` to its owner once; store the rest.
//! let now = SystemTime::now();
//! let record = Record {
//!     id: issued.id().to_string(),
//!     hash: issued.hash().to_string(),
//!     scopes: vec![Scope::new("contacts:read").unwrap()],
//!     expires_at: Some(now + Duration::from_hours(24 * 90)),
//!     revoked: false,
//!     last_used_at: None,
//! };
//!
//! // On a request: parse, find the record by id, then verify.
//! let header = format!("Bearer {}", issued.token());
//! let presented = parse_bearer(&header, &prefix).unwrap();
//! assert_eq!(presented.id(), record.id);
//! let verified = record.verify(&presented, now).unwrap();
//! assert!(verified.allows("contacts:read"));
//! assert!(!verified.allows("contacts:write"));
//! ```
//!
//! [`Verified::record_use`] says when to write `last_used_at`, at most once a minute
//! per token, so a busy token does not write on every request.

use std::{
    fmt,
    time::{Duration, SystemTime},
};

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The shortest gap between two writes of a token's last use.
pub const USE_RECORD_INTERVAL: Duration = Duration::from_mins(1);

/// Why a token or a part of one was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A prefix that is not 2 to 16 lowercase ASCII letters and digits, starting with a
    /// letter.
    #[error("a token prefix must be 2 to 16 lowercase letters and digits, starting with a letter")]
    BadPrefix,
    /// A scope that is not 1 to 64 lowercase ASCII letters, digits, `:`, `.`, `_`, or `-`.
    #[error("a scope must be 1 to 64 lowercase letters, digits, or : . _ -")]
    BadScope,
}

/// Why a presented token was refused. Tell the caller only that it was, with a `401`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// The token is not this record's.
    #[error("the token does not match")]
    Mismatch,
    /// The record was revoked.
    #[error("the token was revoked")]
    Revoked,
    /// The record has expired.
    #[error("the token has expired")]
    Expired,
}

/// The service's part of every token, such as `crm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefix(String);

impl Prefix {
    /// A prefix of 2 to 16 lowercase ASCII letters and digits, starting with a letter.
    ///
    /// # Errors
    ///
    /// [`Error::BadPrefix`] for anything else.
    pub fn new(prefix: &str) -> Result<Self, Error> {
        let fits = (2..=16).contains(&prefix.len())
            && prefix.starts_with(|c: char| c.is_ascii_lowercase())
            && prefix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if fits {
            Ok(Self(prefix.to_string()))
        } else {
            Err(Error::BadPrefix)
        }
    }

    /// The prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One thing a token may do, such as `contacts:read`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Scope(String);

impl Scope {
    /// A scope of 1 to 64 lowercase ASCII letters, digits, `:`, `.`, `_`, or `-`.
    ///
    /// # Errors
    ///
    /// [`Error::BadScope`] for anything else.
    pub fn new(scope: &str) -> Result<Self, Error> {
        let fits = (1..=64).contains(&scope.len())
            && scope
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b":._-".contains(&b));
        if fits {
            Ok(Self(scope.to_string()))
        } else {
            Err(Error::BadScope)
        }
    }

    /// The scope.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A token just issued: the whole token to show its owner once, and the id and hash to
/// store.
pub struct Issued {
    token: String,
    id: String,
    hash: String,
}

impl fmt::Debug for Issued {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The id and hash may be shown, and the token never.
        f.debug_struct("Issued")
            .field("id", &self.id)
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

impl Issued {
    /// The whole token, to show its owner once. Never store or log it.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The id, the record's key.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The SHA-256 hash of the secret, as 64 lowercase hex digits, to store.
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }
}

/// The hex digits of a random v4 uuid, its fixed version and variant digits left out.
fn random_hex() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    // Digit 12 is the version, always `4`, and digit 16 holds the variant bits.
    hex.char_indices()
        .filter(|(index, _)| *index != 12)
        .map(|(_, digit)| digit)
        .collect()
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn hash_hex(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(HEX_DIGITS[usize::from(byte & 0xf)]));
    }
    hex
}

/// Issue a new token under `prefix`.
#[must_use]
pub fn issue(prefix: &Prefix) -> Issued {
    let id = Uuid::new_v4().simple().to_string();
    let secret = format!("{}{}", random_hex(), random_hex());
    let hash = hash_hex(&secret);
    Issued {
        token: format!("{}_{id}_{secret}", prefix.as_str()),
        id,
        hash,
    }
}

/// A token sent by a caller, split, and not yet checked against its record.
pub struct Presented {
    id: String,
    secret: String,
}

impl fmt::Debug for Presented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Presented")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Presented {
    /// The id, to find the record by.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Split `token` into its id and secret, or `None` when it is not a token under
/// `prefix`: three parts, the id 32 hex digits and the secret 62.
#[must_use]
pub fn parse(token: &str, prefix: &Prefix) -> Option<Presented> {
    let rest = token.strip_prefix(prefix.as_str())?.strip_prefix('_')?;
    let (id, secret) = rest.split_once('_')?;
    let hex = |text: &str, length: usize| {
        text.len() == length
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    (hex(id, 32) && hex(secret, 62)).then(|| Presented {
        id: id.to_string(),
        secret: secret.to_string(),
    })
}

/// [`parse`] the token in an `authorization` header value, `Bearer <token>`, its
/// scheme in any case.
#[must_use]
pub fn parse_bearer(authorization: &str, prefix: &Prefix) -> Option<Presented> {
    let (scheme, token) = authorization.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    parse(token.trim_start_matches(' '), prefix)
}

/// What a service keeps of one token, in its own table.
#[derive(Debug, Clone)]
pub struct Record {
    /// The id, from [`Issued::id`].
    pub id: String,
    /// The hash, from [`Issued::hash`].
    pub hash: String,
    /// What the token may do.
    pub scopes: Vec<Scope>,
    /// When it stops working, if ever.
    pub expires_at: Option<SystemTime>,
    /// Whether it was revoked.
    pub revoked: bool,
    /// When it was last used, if it has been.
    pub last_used_at: Option<SystemTime>,
}

/// Compare in time that depends only on the length of `expected`.
fn same_bytes(expected: &[u8], sent: &[u8]) -> bool {
    let mut difference = u8::from(expected.len() != sent.len());
    for (index, byte) in expected.iter().enumerate() {
        difference |= byte ^ sent.get(index).copied().unwrap_or(0);
    }
    difference == 0
}

impl Record {
    /// Check `presented` against this record at `now`: the secret's hash, compared in
    /// constant time, then revocation, then expiry.
    ///
    /// # Errors
    ///
    /// [`Refused`] says why, for the service's own log. Answer every one with the same
    /// `401`.
    pub fn verify(&self, presented: &Presented, now: SystemTime) -> Result<Verified, Refused> {
        let matches = same_bytes(self.hash.as_bytes(), hash_hex(&presented.secret).as_bytes())
            & same_bytes(self.id.as_bytes(), presented.id.as_bytes());
        if !matches {
            return Err(Refused::Mismatch);
        }
        if self.revoked {
            return Err(Refused::Revoked);
        }
        if self.expires_at.is_some_and(|at| at <= now) {
            return Err(Refused::Expired);
        }
        let record_use = self.last_used_at.is_none_or(|at| {
            now.duration_since(at)
                .is_ok_and(|since| since >= USE_RECORD_INTERVAL)
        });
        Ok(Verified {
            id: self.id.clone(),
            scopes: self.scopes.clone(),
            record_use,
        })
    }
}

/// A token that passed, with what it may do.
#[derive(Debug, Clone)]
pub struct Verified {
    id: String,
    scopes: Vec<Scope>,
    record_use: bool,
}

impl Verified {
    /// The token's id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Whether the token may do `scope`.
    #[must_use]
    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|held| held.as_str() == scope)
    }

    /// Whether to write `last_used_at` now: when it was never used, or last recorded
    /// [`USE_RECORD_INTERVAL`] ago or more.
    #[must_use]
    pub const fn record_use(&self) -> bool {
        self.record_use
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> Prefix {
        Prefix::new("crm").unwrap()
    }

    fn record(issued: &Issued) -> Record {
        Record {
            id: issued.id().to_string(),
            hash: issued.hash().to_string(),
            scopes: vec![Scope::new("contacts:read").unwrap()],
            expires_at: None,
            revoked: false,
            last_used_at: None,
        }
    }

    #[test]
    fn an_issued_token_has_the_prefix_and_only_its_hash_is_kept() {
        let issued = issue(&prefix());
        let token = issued.token();
        assert!(token.starts_with("crm_"), "{token}");
        assert_eq!(token.len(), "crm_".len() + 32 + 1 + 62);
        let secret = token.rsplit('_').next().unwrap();
        assert_eq!(issued.hash(), hash_hex(secret));
        assert_eq!(issued.hash().len(), 64);
        assert!(!issued.hash().contains(secret));
        let shown = format!("{issued:?}");
        assert!(!shown.contains(secret), "{shown}");
        assert!(shown.contains(issued.id()) && shown.contains(issued.hash()), "{shown}");
        let presented = parse(token, &prefix()).unwrap();
        let shown = format!("{presented:?}");
        assert!(!shown.contains(secret), "{shown}");
        assert!(shown.contains(presented.id()), "{shown}");
        let other = issue(&prefix());
        assert_ne!(other.token(), issued.token());
        assert_ne!(other.id(), issued.id());
    }

    #[test]
    fn a_token_verifies_against_its_own_record_only() {
        let issued = issue(&prefix());
        let presented = parse(issued.token(), &prefix()).unwrap();
        let verified = record(&issued)
            .verify(&presented, SystemTime::now())
            .unwrap();
        assert_eq!(verified.id(), issued.id());
        assert!(verified.allows("contacts:read"));
        assert!(!verified.allows("contacts:write"));
        let other = issue(&prefix());
        assert_eq!(
            record(&other)
                .verify(&presented, SystemTime::now())
                .unwrap_err(),
            Refused::Mismatch
        );
        // A secret changed in its last digit.
        let mut changed = issued.token().to_string();
        let last = changed.pop().unwrap();
        changed.push(if last == '0' { '1' } else { '0' });
        let presented = parse(&changed, &prefix()).unwrap();
        assert_eq!(
            record(&issued)
                .verify(&presented, SystemTime::now())
                .unwrap_err(),
            Refused::Mismatch
        );
    }

    #[test]
    fn a_revoked_or_expired_token_is_refused() {
        let issued = issue(&prefix());
        let presented = parse(issued.token(), &prefix()).unwrap();
        let now = SystemTime::now();
        let revoked = Record {
            revoked: true,
            ..record(&issued)
        };
        assert_eq!(
            revoked.verify(&presented, now).unwrap_err(),
            Refused::Revoked
        );
        let expired = Record {
            expires_at: Some(now),
            ..record(&issued)
        };
        assert_eq!(
            expired.verify(&presented, now).unwrap_err(),
            Refused::Expired
        );
        let later = Record {
            expires_at: Some(now + Duration::from_secs(1)),
            ..record(&issued)
        };
        assert!(later.verify(&presented, now).is_ok());
    }

    #[test]
    fn last_use_is_recorded_at_most_once_a_minute() {
        let issued = issue(&prefix());
        let presented = parse(issued.token(), &prefix()).unwrap();
        let now = SystemTime::now();
        assert!(
            record(&issued)
                .verify(&presented, now)
                .unwrap()
                .record_use()
        );
        let recent = Record {
            last_used_at: Some(now - Duration::from_secs(30)),
            ..record(&issued)
        };
        assert!(!recent.verify(&presented, now).unwrap().record_use());
        let older = Record {
            last_used_at: Some(now - USE_RECORD_INTERVAL),
            ..record(&issued)
        };
        assert!(older.verify(&presented, now).unwrap().record_use());
    }

    #[test]
    fn only_a_well_formed_token_under_the_prefix_parses() {
        let issued = issue(&prefix());
        let token = issued.token();
        assert!(parse(token, &prefix()).is_some());
        assert!(parse_bearer(&format!("Bearer {token}"), &prefix()).is_some());
        assert!(parse_bearer(&format!("bearer  {token}"), &prefix()).is_some());
        assert!(parse_bearer(&format!("Basic {token}"), &prefix()).is_none());
        assert!(parse(token, &Prefix::new("ppm").unwrap()).is_none());
        for bad in [
            String::new(),
            "crm_".to_string(),
            token.to_uppercase(),
            format!("{token}0"),
            token[..token.len() - 1].to_string(),
            token.replacen('_', "-", 1),
            format!("{token}_x"),
        ] {
            assert!(parse(&bad, &prefix()).is_none(), "{bad}");
        }
    }

    #[test]
    fn prefixes_and_scopes_are_short_codes() {
        for good in ["crm", "ab", "wpm2", "abcdefghijklmnop"] {
            assert!(Prefix::new(good).is_ok(), "{good}");
        }
        for bad in ["", "c", "Crm", "2crm", "crm_", "abcdefghijklmnopq"] {
            assert_eq!(Prefix::new(bad), Err(Error::BadPrefix), "{bad}");
        }
        for good in ["contacts:read", "jobs.run", "a", "x-y_z"] {
            assert!(Scope::new(good).is_ok(), "{good}");
        }
        for bad in ["", "Contacts", "a b", "a\n", &"x".repeat(65)] {
            assert_eq!(Scope::new(bad), Err(Error::BadScope), "{bad}");
        }
    }

    #[test]
    fn a_secret_has_the_randomness_of_two_uuids() {
        let hex = random_hex();
        assert_eq!(hex.len(), 31);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
