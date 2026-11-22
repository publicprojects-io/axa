//! Limits shared across replicas, counted in Postgres or Redis rather than in each
//! process, so three replicas behind a load balancer give a client one limit, not
//! three.
//!
//! A [`SharedLimiter`] counts requests per key in a fixed window, in one statement per
//! request: an upsert on Postgres, and an atomic `INCR` with `EXPIRE NX` on Redis. A
//! service names what happens while the store cannot be reached, with [`WhenDown`],
//! because only it knows whether an outage should let requests through.
//!
//! ```ignore
//! use axa_request_rate_limit::shared::{SharedLimiter, Store, WhenDown, by_client_shared};
//!
//! let store = Store::postgres(pool.clone()).await?;
//! let limiter = SharedLimiter::new(store, "api", limit, window, &secret, WhenDown::Allow)?;
//! let router = routes.layer(from_fn_with_state(limiter, by_client_shared));
//! ```
//!
//! The store holds no address: a client is counted under a hash of its address keyed
//! by the service's secret, which every replica must share.

use std::{
    fmt::Write as _,
    hash::{Hash, Hasher},
    net::IpAddr,
    num::NonZeroU32,
    sync::Arc,
};

use axa_problem::Problem;
use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::{EXEMPT, Verdict, client_key};

/// The fewest bytes the secret that keys client hashes may have.
pub const MIN_SECRET_BYTES: usize = 32;

/// How long a client is told to wait when the store is down and requests are refused.
pub const DOWN_RETRY_SECONDS: u64 = 5;

/// What happens to a request while the store cannot be reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenDown {
    /// It goes ahead uncounted, and a warning is logged. Choose this where an outage
    /// of the store must not take the service down with it.
    Allow,
    /// It is refused with a `503` problem, code `rate_limit_unavailable`, and
    /// `Retry-After`. Choose this where the limit guards something that must not be
    /// flooded.
    Refuse,
}

/// A setting that cannot make a limiter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    /// The secret is under [`MIN_SECRET_BYTES`].
    #[error("the shared limit's secret needs at least {MIN_SECRET_BYTES} bytes")]
    ShortSecret,
    /// The name is not 1 to 64 ASCII letters, digits, `_`, or `-`.
    #[error("a shared limit's name must be 1 to 64 ASCII letters, digits, _ or -")]
    BadName,
}

/// Where the counts are kept.
#[derive(Clone)]
pub enum Store {
    /// An `axa_rate_limits` table.
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    /// Keys under `axa:ratelimit:`.
    #[cfg(feature = "redis")]
    Redis(redis::aio::ConnectionManager),
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => f.write_str("Postgres"),
            #[cfg(feature = "redis")]
            Self::Redis(_) => f.write_str("Redis"),
        }
    }
}

#[cfg(feature = "postgres")]
const POSTGRES_SETUP: &str = "CREATE UNLOGGED TABLE IF NOT EXISTS axa_rate_limits (
    key TEXT PRIMARY KEY,
    window_index BIGINT NOT NULL,
    count BIGINT NOT NULL
)";

/// Each row's own window length, so [`Store::purge`] ends each limit's windows by
/// its own length. A table made before 0.3.3 gains it, empty until a row is counted.
#[cfg(feature = "postgres")]
const POSTGRES_WINDOW_COLUMN: &str =
    "ALTER TABLE axa_rate_limits ADD COLUMN IF NOT EXISTS window_seconds BIGINT";

/// Counts one request in the window the database's clock is in, and answers the count
/// and the seconds until that window ends.
#[cfg(feature = "postgres")]
const POSTGRES_COUNT: &str = "WITH now AS (
    SELECT floor(extract(epoch FROM clock_timestamp()))::bigint AS second
)
INSERT INTO axa_rate_limits AS held (key, window_index, count, window_seconds)
SELECT $1, now.second / $2, 1, $2 FROM now
ON CONFLICT (key) DO UPDATE SET
    count = CASE WHEN held.window_index = EXCLUDED.window_index THEN held.count + 1 ELSE 1 END,
    window_index = EXCLUDED.window_index,
    window_seconds = EXCLUDED.window_seconds
RETURNING count, $2 - (SELECT second FROM now) % $2 AS reset";

impl Store {
    /// A Postgres store on `pool`, making its table if needed. The table is
    /// `UNLOGGED`: counts are lost if Postgres crashes, which costs at most one window.
    ///
    /// # Errors
    ///
    /// Fails when the table cannot be made.
    #[cfg(feature = "postgres")]
    pub async fn postgres(pool: sqlx::PgPool) -> Result<Self, sqlx::Error> {
        sqlx::query(POSTGRES_SETUP).execute(&pool).await?;
        sqlx::query(POSTGRES_WINDOW_COLUMN).execute(&pool).await?;
        Ok(Self::Postgres(pool))
    }

    /// A Redis store over `connection`.
    #[cfg(feature = "redis")]
    #[must_use]
    pub const fn redis(connection: redis::aio::ConnectionManager) -> Self {
        Self::Redis(connection)
    }

    /// Count one request for `key` in a window of `window` seconds, answering the count
    /// so far and the seconds until the window ends.
    async fn count(&self, key: &str, window: u32) -> Result<(u64, u64), String> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(pool) => {
                use sqlx::Row;
                let row = sqlx::query(POSTGRES_COUNT)
                    .bind(key)
                    .bind(i64::from(window))
                    .fetch_one(pool)
                    .await
                    .map_err(|error| error.to_string())?;
                let count: i64 = row.try_get("count").map_err(|error| error.to_string())?;
                let reset: i64 = row.try_get("reset").map_err(|error| error.to_string())?;
                Ok((
                    u64::try_from(count).unwrap_or(0),
                    u64::try_from(reset).unwrap_or(0),
                ))
            }
            #[cfg(feature = "redis")]
            Self::Redis(connection) => {
                let mut connection = connection.clone();
                let key = format!("axa:ratelimit:{key}");
                let (count, ttl): (u64, i64) = redis::pipe()
                    .atomic()
                    .incr(&key, 1_u64)
                    .cmd("EXPIRE")
                    .arg(&key)
                    .arg(window)
                    .arg("NX")
                    .ignore()
                    .ttl(&key)
                    .query_async(&mut connection)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok((count, u64::try_from(ttl).unwrap_or(u64::from(window))))
            }
        }
    }

    /// Delete counts from windows that have ended, each by its own limit's window, so
    /// limits of different lengths can share one store. `window` is used only for a
    /// row counted before 0.3.3, which has no window of its own. Redis expires its own.
    /// Run this on a schedule, such as hourly.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    // With Redis alone, there is nothing to delete or wait for.
    #[cfg_attr(
        not(feature = "postgres"),
        allow(
            unknown_lints,
            unused_variables,
            clippy::unused_async,
            clippy::unused_async_trait_impl
        )
    )]
    pub async fn purge(&self, window: NonZeroU32) -> Result<u64, String> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(pool) => sqlx::query(
                "DELETE FROM axa_rate_limits
                 WHERE (window_index + 1) * COALESCE(window_seconds, $1)
                     <= floor(extract(epoch FROM clock_timestamp()))::bigint",
            )
            .bind(i64::from(window.get()))
            .execute(pool)
            .await
            .map(|done| done.rows_affected())
            .map_err(|error| error.to_string()),
            #[cfg(feature = "redis")]
            Self::Redis(_) => Ok(0),
        }
    }
}

/// A limit on requests per key in a fixed window, counted in a [`Store`] that every
/// replica shares. Clones share everything.
#[derive(Clone)]
pub struct SharedLimiter {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    name: String,
    limit: u32,
    window: u32,
    /// The HMAC made from the secret once, where one it refuses is found, so hashing a
    /// key cannot fail.
    mac: Hmac<Sha256>,
    when_down: WhenDown,
}

impl std::fmt::Debug for SharedLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedLimiter")
            .field("store", &self.inner.store)
            .field("name", &self.inner.name)
            .field("limit", &self.inner.limit)
            .field("window", &self.inner.window)
            .field("when_down", &self.inner.when_down)
            .finish_non_exhaustive()
    }
}

/// Feeds what a key's `Hash` writes into HMAC-SHA256, with every integer as fixed-width
/// little-endian bytes, so replicas built by other Rust releases or for other machines
/// count a client under the same key, as `DefaultHasher`, whose algorithm may change,
/// and `usize`'s native width and order would not.
struct KeyHasher(Hmac<Sha256>);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        let digest = self.0.clone().finalize().into_bytes();
        let mut first = [0; 8];
        first.copy_from_slice(&digest[..8]);
        u64::from_le_bytes(first)
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn write_u8(&mut self, i: u8) {
        self.0.update(&[i]);
    }

    fn write_u16(&mut self, i: u16) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_u32(&mut self, i: u32) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_u64(&mut self, i: u64) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_u128(&mut self, i: u128) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_usize(&mut self, i: usize) {
        self.write_u64(u64::try_from(i).unwrap_or(u64::MAX)); // ARI-5
    }

    fn write_i8(&mut self, i: i8) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_i16(&mut self, i: i16) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_i32(&mut self, i: i32) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_i64(&mut self, i: i64) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_i128(&mut self, i: i128) {
        self.0.update(&i.to_le_bytes());
    }

    fn write_isize(&mut self, i: isize) {
        self.write_i64(i64::try_from(i).unwrap_or(i64::MAX)); // ARI-5
    }
}

/// The store could not be reached, and nothing was counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Down;

impl SharedLimiter {
    /// `limit` requests per key in each window of `window_seconds`, counted in `store`
    /// under `name`, so two limits can share one store. `secret` keys the hash a client
    /// is counted under, and must be the same on every replica.
    ///
    /// # Errors
    ///
    /// [`SetupError`] for a short secret or a bad name.
    pub fn new(
        store: Store,
        name: &str,
        limit: NonZeroU32,
        window_seconds: NonZeroU32,
        secret: &[u8],
        when_down: WhenDown,
    ) -> Result<Self, SetupError> {
        if secret.len() < MIN_SECRET_BYTES {
            return Err(SetupError::ShortSecret);
        }
        let fits = (1..=64).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !fits {
            return Err(SetupError::BadName);
        }
        // HMAC takes a key of any length, so this does not fail.
        let mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|_| SetupError::ShortSecret)?;
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                name: name.to_string(),
                limit: limit.get(),
                window: window_seconds.get(),
                mac,
                when_down,
            }),
        })
    }

    /// The hash `key` is counted under, keyed by the secret, so the store never holds
    /// the key itself.
    fn hashed(&self, key: impl Hash) -> String {
        let mut hasher = KeyHasher(self.inner.mac.clone());
        key.hash(&mut hasher);
        let digest = hasher.0.finalize().into_bytes();
        let mut stored = format!("{}:", self.inner.name);
        for byte in &digest[..16] {
            let _ = write!(stored, "{byte:02x}");
        }
        stored
    }

    /// Count one request for `key`.
    ///
    /// # Errors
    ///
    /// [`Down`] when the store cannot be reached. What then happens is the caller's,
    /// as [`WhenDown`] says for the middleware.
    pub async fn check(&self, key: impl Hash) -> Result<Verdict, Down> {
        let stored = self.hashed(key);
        let (count, reset) = self
            .inner
            .store
            .count(&stored, self.inner.window)
            .await
            .map_err(|error| {
                tracing::warn!(error.msg = %error, "ratelimit_store_down");
                Down
            })?;
        let limit = u64::from(self.inner.limit);
        Ok(Verdict {
            allowed: count <= limit,
            limit: self.inner.limit,
            remaining: u32::try_from(limit.checked_sub(count).unwrap_or(0)).unwrap_or(0), // ARI-3
            reset_seconds: reset.max(1),
            overflowed: false,
            first_refusal: count == limit + 1,
        })
    }

    /// What [`WhenDown`] says for a request the store could not count.
    fn when_down(&self) -> Option<Response> {
        match self.inner.when_down {
            WhenDown::Allow => None,
            WhenDown::Refuse => {
                let mut response = Problem::new(StatusCode::SERVICE_UNAVAILABLE)
                    .with_extension("code", serde_json::json!("rate_limit_unavailable"))
                    .into_response();
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from(DOWN_RETRY_SECONDS));
                Some(response)
            }
        }
    }
}

async fn shared(
    verdict: Result<Verdict, Down>,
    limiter: &SharedLimiter,
    request: Request,
    next: Next,
) -> Response {
    match verdict {
        Ok(verdict) if !verdict.allowed => verdict.refusal(),
        Ok(verdict) => {
            let mut response = next.run(request).await;
            verdict.stamp(&mut response);
            response
        }
        Err(Down) => match limiter.when_down() {
            Some(refusal) => refusal,
            None => next.run(request).await,
        },
    }
}

/// Limit each client across replicas, as [`crate::by_client`] does in one process. The
/// probes in [`EXEMPT`] are not counted.
pub async fn by_client_shared(
    State(limiter): State<SharedLimiter>,
    request: Request,
    next: Next,
) -> Response {
    if EXEMPT.contains(&request.uri().path()) {
        return next.run(request).await;
    }
    let client: Option<IpAddr> = client_key(request.extensions());
    let verdict = limiter.check(client).await;
    shared(verdict, &limiter, request, next).await
}

#[cfg(all(test, feature = "postgres"))]
mod tests {
    use super::*;

    fn limiter() -> SharedLimiter {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/none").unwrap();
        let one = NonZeroU32::new(1).unwrap();
        SharedLimiter::new(
            Store::Postgres(pool),
            "api",
            one,
            one,
            &[7; 32],
            WhenDown::Refuse,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_key_is_counted_under_one_hash_on_every_build() {
        let limiter = limiter();
        // HMAC-SHA256 under the secret of what `"k"`'s `Hash` writes, `k` then `0xff`,
        // its first 16 bytes: computed apart from this crate, so a build whose hash
        // differs, from another Rust release or machine, fails here.
        assert_eq!(limiter.hashed("k"), "api:01754edd21ab0860ceee492554db4491");
        let client: Option<IpAddr> = Some("203.0.113.7".parse().unwrap());
        assert_eq!(limiter.hashed(client), limiter.hashed(client));
        assert_ne!(limiter.hashed(client), limiter.hashed(None::<IpAddr>));
        assert!(!limiter.hashed(client).contains("203"));
    }

    fn fed(feed: impl FnOnce(&mut KeyHasher)) -> u64 {
        let mut hasher = KeyHasher(Hmac::<Sha256>::new_from_slice(&[7; 32]).unwrap());
        feed(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn every_integer_is_fed_in_fixed_width_little_endian_order() {
        let bytes = |b: &[u8]| {
            let b = b.to_vec();
            fed(move |h| h.write(&b))
        };
        assert_eq!(fed(|h| h.write_u8(0x12)), bytes(&[0x12]));
        assert_eq!(fed(|h| h.write_u16(0x1234)), bytes(&[0x34, 0x12]));
        assert_eq!(
            fed(|h| h.write_u32(0x1234_5678)),
            bytes(&0x1234_5678_u32.to_le_bytes())
        );
        assert_eq!(fed(|h| h.write_u64(7)), bytes(&7_u64.to_le_bytes()));
        assert_eq!(fed(|h| h.write_u128(7)), bytes(&7_u128.to_le_bytes()));
        assert_eq!(fed(|h| h.write_i8(-2)), bytes(&[0xfe]));
        assert_eq!(fed(|h| h.write_i16(-2)), bytes(&(-2_i16).to_le_bytes()));
        assert_eq!(fed(|h| h.write_i32(-2)), bytes(&(-2_i32).to_le_bytes()));
        assert_eq!(fed(|h| h.write_i64(-2)), bytes(&(-2_i64).to_le_bytes()));
        assert_eq!(fed(|h| h.write_i128(-2)), bytes(&(-2_i128).to_le_bytes()));
        // A `usize` or `isize` is fed as 64 bits, so a 32-bit replica agrees.
        assert_eq!(fed(|h| h.write_usize(7)), fed(|h| h.write_u64(7)));
        assert_eq!(fed(|h| h.write_isize(-2)), fed(|h| h.write_i64(-2)));
        // Widths differ, so one value at two widths is two keys.
        assert_ne!(fed(|h| h.write_u8(1)), fed(|h| h.write_u16(1)));
    }

    #[tokio::test]
    async fn the_store_is_named_and_the_secret_kept_out_of_debug_text() {
        let printed = format!("{:?}", limiter());
        assert!(printed.contains("store: Postgres"), "{printed}");
        assert!(
            !printed.contains("secret") && !printed.contains("7, 7"),
            "{printed}"
        );
    }
}
