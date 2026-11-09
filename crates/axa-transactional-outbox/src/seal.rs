//! Payloads sealed at rest with XChaCha20-Poly1305, behind the `encryption` feature.
//!
//! A sealed payload is a version byte, the key's id, a random 24-byte nonce, and the
//! ciphertext with its tag. The version, the key id, and the event's id are the
//! associated data, so a sealed payload moved to another event does not open.

use std::fmt;

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, KeyInit, Payload},
};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroize;

/// The shortest secret [`Key::new`] accepts, in bytes.
pub const MIN_KEY_LEN: usize = 32;

/// The first byte of every sealed payload, so that the format can change.
const VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 2 + NONCE_LEN;

/// Keeps a key made for this from being the same as one made for something else from
/// the same secret.
const CONTEXT: &[u8] = b"axa-transactional-outbox payload key, version 1\0";

/// One key for sealing payloads, with the id a sealed payload names it by. Its `Debug`
/// shows the id and never the key, and its key material is wiped when it is dropped.
#[derive(Clone)]
pub struct Key {
    id: u8,
    cipher: XChaCha20Poly1305,
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Key")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// A secret shorter than [`MIN_KEY_LEN`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTooShort {
    /// The length given, in bytes.
    pub len: usize,
}

impl fmt::Display for KeyTooShort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "an outbox key needs at least {MIN_KEY_LEN} bytes, and this has {}",
            self.len
        )
    }
}

impl std::error::Error for KeyTooShort {}

impl Key {
    /// A key named `id` from `secret`, which must be at least [`MIN_KEY_LEN`] random
    /// bytes. Give each new secret a new id, so that payloads sealed under the old one
    /// still open while it is kept in [`Keys`].
    ///
    /// # Errors
    ///
    /// [`KeyTooShort`] when `secret` is shorter than [`MIN_KEY_LEN`].
    pub fn new(id: u8, secret: &[u8]) -> Result<Self, KeyTooShort> {
        if secret.len() < MIN_KEY_LEN {
            return Err(KeyTooShort { len: secret.len() });
        }
        // The hasher writes the key straight into one buffer, wiped once the cipher has
        // it. The hasher wipes itself, and the cipher wipes its own copy when dropped.
        let mut derived = chacha20poly1305::Key::default();
        Sha256::new()
            .chain_update(CONTEXT)
            .chain_update(secret)
            .finalize_into(&mut derived);
        let cipher = XChaCha20Poly1305::new(&derived);
        derived.as_mut_slice().zeroize();
        Ok(Self { id, cipher })
    }

    /// The id a sealed payload names this key by.
    #[must_use]
    pub fn id(&self) -> u8 {
        self.id
    }
}

/// The keys a service holds: one it seals with, and older ones it still opens with,
/// so that a key can be replaced without a payload becoming unreadable. Its `Debug`
/// shows the ids and never a key.
#[derive(Clone)]
pub struct Keys {
    current: Key,
    older: Vec<Key>,
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let older: Vec<u8> = self.older.iter().map(Key::id).collect();
        f.debug_struct("Keys")
            .field("current", &self.current.id)
            .field("older", &older)
            .finish()
    }
}

/// Why a sealed payload did not open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unopened {
    /// No key held has the id it names, or its format is newer than this crate.
    NoKey,
    /// It does not open under the key it names, so it was changed.
    Tampered,
}

impl Keys {
    /// Seal with `current`.
    #[must_use]
    pub fn new(current: Key) -> Self {
        Self {
            current,
            older: Vec::new(),
        }
    }

    /// Also open payloads sealed with `key`. A key with the id of one already held
    /// is ignored.
    #[must_use]
    pub fn and_open_with(mut self, key: Key) -> Self {
        if key.id != self.current.id && self.older.iter().all(|held| held.id != key.id) {
            self.older.push(key);
        }
        self
    }

    /// Seal `plain` for the event `event_id`.
    pub(crate) fn seal(&self, event_id: Uuid, plain: &[u8]) -> Result<Vec<u8>, sqlx::Error> {
        let key = &self.current;
        let nonce = XNonce::try_generate().map_err(|error| sqlx::Error::Encode(error.into()))?;
        let aad = associated(key.id, event_id);
        let sealed = key
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plain,
                    aad: &aad,
                },
            )
            .map_err(|_| sqlx::Error::Encode("the payload could not be sealed".into()))?;
        let mut out = Vec::with_capacity(HEADER_LEN + sealed.len());
        out.push(VERSION);
        out.push(key.id);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Open `sealed`, which was sealed for `event_id`.
    pub(crate) fn open(&self, event_id: Uuid, sealed: &[u8]) -> Result<Vec<u8>, Unopened> {
        if sealed.len() < HEADER_LEN || sealed[0] != VERSION {
            return Err(Unopened::NoKey);
        }
        let id = sealed[1];
        let key = std::iter::once(&self.current)
            .chain(&self.older)
            .find(|key| key.id == id)
            .ok_or(Unopened::NoKey)?;
        let nonce = XNonce::try_from(&sealed[2..HEADER_LEN]).map_err(|_| Unopened::Tampered)?;
        let aad = associated(id, event_id);
        key.cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &sealed[HEADER_LEN..],
                    aad: &aad,
                },
            )
            .map_err(|_| Unopened::Tampered)
    }
}

/// The associated data: what a sealed payload is bound to.
fn associated(key_id: u8, event_id: Uuid) -> [u8; 18] {
    let mut aad = [0; 18];
    aad[0] = VERSION;
    aad[1] = key_id;
    aad[2..].copy_from_slice(event_id.as_bytes());
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: u8, byte: u8) -> Key {
        Key::new(id, &[byte; 32]).unwrap()
    }

    #[test]
    fn a_short_secret_is_refused() {
        assert_eq!(Key::new(1, &[7; 31]).unwrap_err(), KeyTooShort { len: 31 });
        assert!(Key::new(1, &[7; 32]).is_ok());
        assert!(Key::new(1, &[7; 64]).is_ok());
    }

    #[test]
    fn a_sealed_payload_opens_only_for_its_event_and_with_its_key() {
        let keys = Keys::new(key(1, 7));
        let event = Uuid::new_v4();
        let sealed = keys.seal(event, b"{\"name\":\"Ada\"}").unwrap();
        assert!(!sealed.windows(3).any(|part| part == b"Ada"));
        assert_eq!(keys.open(event, &sealed).unwrap(), b"{\"name\":\"Ada\"}");
        // Moved to another event.
        assert_eq!(keys.open(Uuid::new_v4(), &sealed), Err(Unopened::Tampered));
        // Changed by one bit.
        let mut changed = sealed.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert_eq!(keys.open(event, &changed), Err(Unopened::Tampered));
        // A different secret under the same id.
        assert_eq!(
            Keys::new(key(1, 8)).open(event, &sealed),
            Err(Unopened::Tampered)
        );
        // An id no key has.
        assert_eq!(
            Keys::new(key(2, 7)).open(event, &sealed),
            Err(Unopened::NoKey)
        );
    }

    #[test]
    fn each_seal_has_its_own_nonce() {
        let keys = Keys::new(key(1, 7));
        let event = Uuid::new_v4();
        assert_ne!(
            keys.seal(event, b"x").unwrap(),
            keys.seal(event, b"x").unwrap()
        );
    }

    #[test]
    fn an_older_key_still_opens_what_it_sealed() {
        let event = Uuid::new_v4();
        let old = Keys::new(key(1, 7));
        let sealed = old.seal(event, b"kept").unwrap();
        let rotated = Keys::new(key(2, 9)).and_open_with(key(1, 7));
        assert_eq!(rotated.open(event, &sealed).unwrap(), b"kept");
        assert_eq!(rotated.seal(event, b"new").unwrap()[1], 2);
    }

    #[test]
    fn debug_text_leaves_out_the_key() {
        let keys = Keys::new(Key::new(3, b"a service secret of thirty-two b").unwrap());
        for text in [format!("{keys:?}"), format!("{:?}", keys.current)] {
            assert!(text.contains('3'), "{text}");
            assert!(!text.contains("secret"), "{text}");
            assert!(!text.contains("cipher"), "{text}");
        }
    }
}
