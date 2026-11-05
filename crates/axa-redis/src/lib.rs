//! A Redis cache with mandatory expiry.
//!
//! Values are stored as JSON under a key prefix, so several services can share
//! one Redis without colliding.
//!
//! ```no_run
//! use axa_redis::{Cache, CacheConfig};
//! use std::time::Duration;
//!
//! # async fn run() -> Result<(), axa_redis::Error> {
//! let config = CacheConfig::new("redis://localhost", "my_service", Duration::from_secs(60));
//! let cache = Cache::connect(&config).await?;
//!
//! cache.set("greeting", &"hello").await?;
//! let greeting: Option<String> = cache.get("greeting").await?;
//!
//! // Falls back to the loader on a miss, and also if Redis is down.
//! let total: Result<u64, std::io::Error> =
//!     cache.get_or_load("total", || async { Ok(42) }).await;
//! # Ok(())
//! # }
//! ```
//!
//! # What this crate promises
//!
//! - **Every entry expires.** There is no way to store a value without a TTL,
//!   because an entry that never expires is data nobody remembers holding.
//! - **A cache failure is not a service failure.** `get_or_load` logs a Redis
//!   error and calls the loader.
//! - **An unreadable entry is a miss.** After a change to a cached type, old
//!   entries are skipped rather than failing every read until they expire.
//!
//! # What to keep out of it
//!
//! Redis keeps values in plain text. Do not cache secrets, session ids, or
//! tokens. Cached records can also outlive a change to them, so caching data
//! subject to masking or erasure needs its own decision: an entry written
//! before an erasure survives it until its TTL ends unless it is deleted too.

use std::{future::Future, time::Duration};

use redis::{AsyncCommands, aio::ConnectionManager};
use serde::{Serialize, de::DeserializeOwned};

/// A cache failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Redis failed or could not be reached.
    #[error(transparent)]
    Redis(#[from] redis::RedisError),
    /// A value could not be encoded as JSON.
    #[error(transparent)]
    Encode(#[from] serde_json::Error),
    /// A TTL below one millisecond, which would store nothing useful.
    #[error("a cache entry needs a TTL of at least one millisecond")]
    InvalidTtl,
}

/// Cache settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    /// Connection string. Use `rediss://` for TLS.
    pub url: String,
    /// Prefix on every key, usually the service name.
    pub prefix: String,
    /// How long entries live unless a call says otherwise.
    pub default_ttl: Duration,
}

impl CacheConfig {
    /// Build settings.
    #[must_use]
    pub fn new(url: impl Into<String>, prefix: impl Into<String>, default_ttl: Duration) -> Self {
        Self {
            url: url.into(),
            prefix: prefix.into(),
            default_ttl,
        }
    }
}

/// Join a prefix and a key.
fn full_key(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}:{key}")
    }
}

/// A TTL in whole milliseconds, refusing one that rounds to nothing.
fn ttl_millis(ttl: Duration) -> Result<u64, Error> {
    match u64::try_from(ttl.as_millis()) {
        Ok(0) => Err(Error::InvalidTtl),
        Ok(millis) => Ok(millis),
        // A TTL past `u64` milliseconds is hundreds of millions of years.
        Err(_) => Ok(u64::MAX),
    }
}

/// Decode a stored value, treating anything unreadable as a miss.
fn decode<T: DeserializeOwned>(key: &str, stored: Option<&str>) -> Option<T> {
    let stored = stored?;
    match serde_json::from_str(stored) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(key, %error, "unreadable cache entry treated as a miss");
            None
        }
    }
}

/// A connection to the cache. Cheap to clone, and it reconnects by itself.
#[derive(Clone)]
pub struct Cache {
    connection: ConnectionManager,
    prefix: String,
    default_ttl: Duration,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the connection: its address can carry a password.
        f.debug_struct("Cache")
            .field("prefix", &self.prefix)
            .field("default_ttl", &self.default_ttl)
            .finish_non_exhaustive()
    }
}

impl Cache {
    /// Connect.
    ///
    /// # Errors
    ///
    /// Fails when the URL is invalid or Redis cannot be reached, or when the
    /// default TTL is below one millisecond.
    pub async fn connect(config: &CacheConfig) -> Result<Self, Error> {
        ttl_millis(config.default_ttl)?;
        let client = redis::Client::open(config.url.as_str())?;
        Ok(Self {
            connection: ConnectionManager::new(client).await?,
            prefix: config.prefix.clone(),
            default_ttl: config.default_ttl,
        })
    }

    /// The full key for `key`, including the prefix.
    fn key(&self, key: &str) -> String {
        full_key(&self.prefix, key)
    }

    /// Check that Redis answers.
    ///
    /// # Errors
    ///
    /// Fails when it does not.
    pub async fn ping(&self) -> Result<(), Error> {
        let mut connection = self.connection.clone();
        let _: String = redis::cmd("PING").query_async(&mut connection).await?;
        Ok(())
    }

    /// Read a value. A missing, expired, or unreadable entry is `None`.
    ///
    /// # Errors
    ///
    /// Fails when Redis does.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, Error> {
        let full = self.key(key);
        let mut connection = self.connection.clone();
        let stored: Option<String> = connection.get(&full).await?;
        Ok(decode(&full, stored.as_deref()))
    }

    /// Store a value for the default TTL.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be encoded or Redis fails.
    pub async fn set<T: Serialize + Sync>(&self, key: &str, value: &T) -> Result<(), Error> {
        self.set_with_ttl(key, value, self.default_ttl).await
    }

    /// Store a value for `ttl`.
    ///
    /// # Errors
    ///
    /// Fails on a TTL below one millisecond, when the value cannot be encoded,
    /// or when Redis fails.
    pub async fn set_with_ttl<T: Serialize + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> Result<(), Error> {
        let millis = ttl_millis(ttl)?;
        let encoded = serde_json::to_string(value)?;
        let mut connection = self.connection.clone();
        let (): () = connection.pset_ex(self.key(key), encoded, millis).await?;
        Ok(())
    }

    /// Remove an entry, returning whether it existed.
    ///
    /// # Errors
    ///
    /// Fails when Redis does.
    pub async fn delete(&self, key: &str) -> Result<bool, Error> {
        let mut connection = self.connection.clone();
        let removed: usize = connection.del(self.key(key)).await?;
        Ok(removed > 0)
    }

    /// Whether an entry exists.
    ///
    /// # Errors
    ///
    /// Fails when Redis does.
    pub async fn exists(&self, key: &str) -> Result<bool, Error> {
        let mut connection = self.connection.clone();
        Ok(connection.exists(self.key(key)).await?)
    }

    /// Read a value, or load and store it on a miss.
    ///
    /// A Redis failure is logged and the value is loaded anyway, so a cache
    /// outage slows a service down rather than breaking it. Only the loader's
    /// own error is returned, and a failed load stores nothing.
    ///
    /// # Errors
    ///
    /// Returns the loader's error.
    pub async fn get_or_load<T, E, F, Fut>(&self, key: &str, load: F) -> Result<T, E>
    where
        T: Serialize + DeserializeOwned + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        match self.get(key).await {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => {}
            Err(error) => tracing::warn!(key, %error, "cache read failed, loading instead"),
        }
        let value = load().await?;
        if let Err(error) = self.set(key, &value).await {
            tracing::warn!(key, %error, "cache write failed");
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_carry_the_prefix() {
        assert_eq!(full_key("svc", "a"), "svc:a");
        assert_eq!(full_key("", "a"), "a");
    }

    #[test]
    fn a_zero_ttl_is_refused() {
        assert!(matches!(ttl_millis(Duration::ZERO), Err(Error::InvalidTtl)));
        assert!(matches!(
            ttl_millis(Duration::from_micros(999)),
            Err(Error::InvalidTtl)
        ));
    }

    #[test]
    fn ttls_are_in_milliseconds() {
        assert_eq!(ttl_millis(Duration::from_secs(2)).unwrap(), 2_000);
    }

    #[test]
    fn a_huge_ttl_saturates() {
        assert_eq!(ttl_millis(Duration::MAX).unwrap(), u64::MAX);
    }

    #[test]
    fn stored_values_decode() {
        assert_eq!(decode::<u32>("k", Some("7")), Some(7));
    }

    #[test]
    fn a_missing_value_is_a_miss() {
        assert_eq!(decode::<u32>("k", None), None);
    }

    #[test]
    fn an_unreadable_value_is_a_miss() {
        assert_eq!(decode::<u32>("k", Some("not json")), None);
    }
}
