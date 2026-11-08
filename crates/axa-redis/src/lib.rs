//! A Redis cache with mandatory expiry.
//!
//! Values are stored as JSON under a key prefix, so several services can share
//! one Redis without colliding. [`Cache::memory`] gives the same behaviour inside
//! one process, for tests and for development with no Redis.
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
//!   error and calls the loader. Every call has a time limit,
//!   [`CacheConfig::timeout`], so a Redis that is down or frozen costs a request
//!   that long at most, never the client's own retries.
//! - **An unreadable entry is a miss.** After a change to a cached type, old
//!   entries are skipped rather than failing every read until they expire.
//!
//! # What to keep out of it
//!
//! Redis keeps values in plain text. Do not cache secrets, session ids, or
//! tokens. Cached records can also outlive a change to them, so caching data
//! subject to masking or erasure needs its own decision: an entry written
//! before an erasure survives it until its TTL ends unless it is deleted too.

use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

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
    /// Redis did not answer within [`CacheConfig::timeout`].
    #[error("the cache did not answer in time")]
    Timeout,
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
    /// How long one call may wait for Redis.
    pub timeout: Duration,
}

/// How long one call waits for Redis unless [`CacheConfig::timeout`] says
/// otherwise. A cache is an optimisation, so it gets little of a request's time.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

impl CacheConfig {
    /// Build settings, with [`DEFAULT_TIMEOUT`].
    #[must_use]
    pub fn new(url: impl Into<String>, prefix: impl Into<String>, default_ttl: Duration) -> Self {
        Self {
            url: url.into(),
            prefix: prefix.into(),
            default_ttl,
            timeout: DEFAULT_TIMEOUT,
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
fn decode<T: DeserializeOwned>(stored: Option<&str>) -> Option<T> {
    let stored = stored?;
    match serde_json::from_str(stored) {
        Ok(value) => Some(value),
        Err(error) => {
            // The error's text can quote the value, and a key can name a person.
            tracing::warn!(
                line = error.line(),
                "unreadable cache entry treated as a miss"
            );
            None
        }
    }
}

/// The longest an in-process entry lives, for a TTL too large for the clock.
const MEMORY_MAX_TTL: Duration = Duration::from_hours(24 * 365);

/// The most the in-process cache holds, in bytes of keys and values.
const MEMORY_LIMIT: usize = 32 * 1024 * 1024;

/// Entries in this process, each with the moment it expires.
#[derive(Debug, Default)]
struct Memory {
    entries: HashMap<String, (Instant, String)>,
    bytes: usize,
}

impl Memory {
    fn get(&mut self, key: &str, now: Instant) -> Option<String> {
        let (expires, value) = self.entries.get(key)?;
        if *expires > now {
            return Some(value.clone());
        }
        self.remove(key);
        None
    }

    fn remove(&mut self, key: &str) -> bool {
        match self.entries.remove(key) {
            Some((_, value)) => {
                self.bytes -= key.len() + value.len();
                true
            }
            None => false,
        }
    }

    fn insert(&mut self, key: &str, value: String, expires: Instant, now: Instant) {
        self.remove(key);
        let size = key.len() + value.len();
        if size > MEMORY_LIMIT {
            return;
        }
        if self.bytes + size > MEMORY_LIMIT {
            let stale: Vec<String> = self
                .entries
                .iter()
                .filter(|(_, (when, _))| *when <= now)
                .map(|(name, _)| name.clone())
                .collect();
            for name in stale {
                self.remove(&name);
            }
        }
        // A cache may lose anything, so when live entries fill it, it starts again.
        if self.bytes + size > MEMORY_LIMIT {
            self.entries.clear();
            self.bytes = 0;
        }
        self.bytes += size;
        self.entries.insert(key.to_string(), (expires, value));
    }
}

#[derive(Clone)]
enum Backend {
    Redis(ConnectionManager),
    Memory(Arc<Mutex<Memory>>),
}

impl Backend {
    fn with_memory<T>(memory: &Mutex<Memory>, use_it: impl FnOnce(&mut Memory) -> T) -> T {
        use_it(&mut memory.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// A connection to the cache. Cheap to clone, and it reconnects by itself.
#[derive(Clone)]
pub struct Cache {
    backend: Backend,
    prefix: String,
    default_ttl: Duration,
    timeout: Duration,
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
            backend: Backend::Redis(ConnectionManager::new(client).await?),
            prefix: config.prefix.clone(),
            default_ttl: config.default_ttl,
            timeout: config.timeout,
        })
    }

    /// A cache held in this process, for tests and for development with no
    /// Redis. Replicas do not share it, so it does not belong in production. It
    /// holds at most 32 MB, and starts again when full.
    ///
    /// # Errors
    ///
    /// Fails when the default TTL is below one millisecond.
    pub fn memory(prefix: impl Into<String>, default_ttl: Duration) -> Result<Self, Error> {
        ttl_millis(default_ttl)?;
        Ok(Self {
            backend: Backend::Memory(Arc::default()),
            prefix: prefix.into(),
            default_ttl,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Run a Redis call under the time limit. The client retries a lost
    /// connection for seconds by itself, which a request must not wait for.
    async fn bounded<T>(
        &self,
        call: impl Future<Output = Result<T, redis::RedisError>>,
    ) -> Result<T, Error> {
        match tokio::time::timeout(self.timeout, call).await {
            Ok(result) => Ok(result?),
            Err(_) => Err(Error::Timeout),
        }
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
        if let Backend::Redis(connection) = &self.backend {
            let mut connection = connection.clone();
            let ping = redis::cmd("PING");
            let _: String = self.bounded(ping.query_async(&mut connection)).await?;
        }
        Ok(())
    }

    /// Read a value. A missing, expired, or unreadable entry is `None`.
    ///
    /// # Errors
    ///
    /// Fails when Redis does.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, Error> {
        let full = self.key(key);
        let stored: Option<String> = match &self.backend {
            Backend::Redis(connection) => {
                let mut connection = connection.clone();
                self.bounded(connection.get(&full)).await?
            }
            Backend::Memory(memory) => {
                Backend::with_memory(memory, |memory| memory.get(&full, Instant::now()))
            }
        };
        Ok(decode(stored.as_deref()))
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
        let full = self.key(key);
        match &self.backend {
            Backend::Redis(connection) => {
                let mut connection = connection.clone();
                let (): () = self
                    .bounded(connection.pset_ex(full, encoded, millis))
                    .await?;
            }
            Backend::Memory(memory) => {
                let now = Instant::now();
                let expires = now.checked_add(ttl).unwrap_or(now + MEMORY_MAX_TTL);
                Backend::with_memory(memory, |memory| {
                    memory.insert(&full, encoded, expires, now);
                });
            }
        }
        Ok(())
    }

    /// Remove an entry, returning whether it existed.
    ///
    /// # Errors
    ///
    /// Fails when Redis does.
    pub async fn delete(&self, key: &str) -> Result<bool, Error> {
        let full = self.key(key);
        match &self.backend {
            Backend::Redis(connection) => {
                let mut connection = connection.clone();
                let removed: usize = self.bounded(connection.del(full)).await?;
                Ok(removed > 0)
            }
            Backend::Memory(memory) => {
                Ok(Backend::with_memory(memory, |memory| memory.remove(&full)))
            }
        }
    }

    /// Whether an entry exists.
    ///
    /// # Errors
    ///
    /// Fails when Redis does.
    pub async fn exists(&self, key: &str) -> Result<bool, Error> {
        let full = self.key(key);
        match &self.backend {
            Backend::Redis(connection) => {
                let mut connection = connection.clone();
                self.bounded(connection.exists(full)).await
            }
            Backend::Memory(memory) => Ok(Backend::with_memory(memory, |memory| {
                memory.get(&full, Instant::now()).is_some()
            })),
        }
    }

    /// Read a value, or load and store it on a miss.
    ///
    /// A Redis failure is logged and the value is loaded anyway, so a cache
    /// outage slows a service down rather than breaking it. After a failed read
    /// it does not try to store the value, so an outage costs one time limit,
    /// not two. Only the loader's own error is returned, and a failed load stores
    /// nothing.
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
            Err(error) => {
                tracing::warn!(%error, "cache read failed, loading instead");
                return load().await;
            }
        }
        let value = load().await?;
        if let Err(error) = self.set(key, &value).await {
            tracing::warn!(%error, "cache write failed");
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: Duration = Duration::from_mins(1);

    fn memory() -> Cache {
        Cache::memory("svc", LIVE).unwrap()
    }

    #[tokio::test]
    async fn the_memory_cache_stores_like_redis() {
        let cache = memory();
        assert_eq!(cache.get::<u32>("a").await.unwrap(), None);
        cache.set("a", &7_u32).await.unwrap();
        assert_eq!(cache.get::<u32>("a").await.unwrap(), Some(7));
        assert!(cache.exists("a").await.unwrap());
        assert!(cache.delete("a").await.unwrap());
        assert!(!cache.delete("a").await.unwrap());
        assert!(!cache.exists("a").await.unwrap());
        cache.ping().await.unwrap();
    }

    #[tokio::test]
    async fn clones_of_a_memory_cache_share_it() {
        let cache = memory();
        cache.clone().set("a", &1_u32).await.unwrap();
        assert_eq!(cache.get::<u32>("a").await.unwrap(), Some(1));
    }

    #[tokio::test]
    async fn the_memory_cache_refuses_a_zero_ttl() {
        assert!(matches!(
            Cache::memory("svc", Duration::ZERO),
            Err(Error::InvalidTtl)
        ));
        assert!(matches!(
            memory().set_with_ttl("a", &1_u32, Duration::ZERO).await,
            Err(Error::InvalidTtl)
        ));
    }

    #[tokio::test]
    async fn the_memory_cache_loads_on_a_miss_only() {
        let cache = memory();
        let first: Result<u32, ()> = cache.get_or_load("a", || async { Ok(1) }).await;
        let second: Result<u32, ()> = cache.get_or_load("a", || async { Ok(2) }).await;
        assert_eq!((first, second), (Ok(1), Ok(1)));
    }

    #[test]
    fn an_expired_entry_is_gone_and_its_bytes_are_returned() {
        let mut memory = Memory::default();
        let now = Instant::now();
        memory.insert("k", "value".to_string(), now + LIVE, now);
        assert_eq!(memory.get("k", now).as_deref(), Some("value"));
        assert_eq!(memory.get("k", now + LIVE), None);
        assert_eq!(memory.bytes, 0);
    }

    #[test]
    fn replacing_an_entry_counts_only_the_new_one() {
        let mut memory = Memory::default();
        let now = Instant::now();
        memory.insert("k", "long value".to_string(), now + LIVE, now);
        memory.insert("k", "v".to_string(), now + LIVE, now);
        assert_eq!(memory.bytes, 2);
    }

    #[test]
    fn the_memory_cache_never_holds_more_than_its_limit() {
        let mut memory = Memory::default();
        let now = Instant::now();
        let half = "x".repeat(MEMORY_LIMIT / 2);
        for key in ["a", "b", "c", "d"] {
            memory.insert(key, half.clone(), now + LIVE, now);
            assert!(memory.bytes <= MEMORY_LIMIT);
        }
        assert_eq!(memory.get("d", now).as_deref(), Some(half.as_str()));
        memory.insert("big", "x".repeat(MEMORY_LIMIT + 1), now + LIVE, now);
        assert_eq!(memory.get("big", now), None);
    }

    #[test]
    fn stale_entries_go_before_live_ones() {
        let mut memory = Memory::default();
        let now = Instant::now();
        let later = now + Duration::from_secs(5);
        let half = "x".repeat(MEMORY_LIMIT / 2);
        memory.insert("stale", half.clone(), later, now);
        memory.insert("live", half, now + LIVE, now);
        memory.insert("new", "y".repeat(16), later + LIVE, later);
        assert!(memory.entries.contains_key("live"));
        assert!(!memory.entries.contains_key("stale"));
    }

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
        assert_eq!(decode::<u32>(Some("7")), Some(7));
    }

    #[test]
    fn a_missing_value_is_a_miss() {
        assert_eq!(decode::<u32>(None), None);
    }

    #[test]
    fn an_unreadable_value_is_a_miss() {
        assert_eq!(decode::<u32>(Some("not json")), None);
    }

    thread_local! {
        /// What this thread has logged, as each event's message and fields.
        static LOGGED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Record every event from here on into its thread's [`LOGGED`], and forget what
    /// this thread logged before. The recorder is the global default: one set only for
    /// a thread can miss an event, as `tracing` caches whether a call site is wanted
    /// for all threads at once, and another test can ask first.
    fn record_logs() {
        static SET: std::sync::Once = std::sync::Once::new();
        SET.call_once(|| {
            tracing::subscriber::set_global_default(Recorder).expect("no other recorder");
        });
        // A call site first reached before the recorder was set may still be cached as
        // wanted by no one.
        tracing::callsite::rebuild_interest_cache();
        LOGGED.with(|logged| logged.borrow_mut().clear());
    }

    fn logged() -> Vec<String> {
        LOGGED.with(|logged| logged.borrow().clone())
    }

    struct Recorder;

    struct Line<'a>(&'a mut String);

    impl tracing::field::Visit for Line<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    impl tracing::Subscriber for Recorder {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut line = String::new();
            event.record(&mut Line(&mut line));
            LOGGED.with(|logged| logged.borrow_mut().push(line));
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// A key that names a person, and a value that only its owner should see.
    const PERSON: &str = "person:ana@example.com";
    const SECRET: &str = "s3cret-diagnosis";

    fn assert_named_neither(lines: &[String]) {
        assert!(!lines.is_empty(), "nothing was logged");
        for line in lines {
            assert!(
                !line.contains("ana@example.com"),
                "a key was logged: {line}"
            );
            assert!(!line.contains(SECRET), "a value was logged: {line}");
        }
    }

    #[tokio::test]
    async fn an_unreadable_entry_is_logged_without_its_key_or_value() {
        record_logs();
        let cache = memory();
        let Backend::Memory(held) = &cache.backend else {
            unreachable!("a memory cache")
        };
        let now = Instant::now();
        Backend::with_memory(held, |held| {
            held.insert(
                &cache.key(PERSON),
                format!("{{\"note\": \"{SECRET}\" unterminated"),
                now + LIVE,
                now,
            );
        });

        assert_eq!(cache.get::<serde_json::Value>(PERSON).await.unwrap(), None);
        let loaded: Result<u32, ()> = cache.get_or_load(PERSON, || async { Ok(1) }).await;
        assert_eq!(loaded, Ok(1));
        assert_named_neither(&logged());
    }

    #[tokio::test]
    async fn a_failed_cache_write_is_logged_without_its_key_or_value() {
        record_logs();
        // JSON has no key that is not text, so this cannot be stored.
        let unstorable = || async {
            Ok::<_, ()>(std::collections::HashMap::from([(
                (1_u8, 2_u8),
                SECRET.to_string(),
            )]))
        };

        let loaded = memory().get_or_load(PERSON, unstorable).await.unwrap();
        assert_eq!(loaded[&(1, 2)], SECRET);
        assert_named_neither(&logged());
    }
}
