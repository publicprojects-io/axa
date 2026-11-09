//! In-process request limits and failed-login lockout.
//!
//! A [`Limiter`] counts requests per key in a fixed window. [`by_client`] is the common
//! case, one limit per client address. [`by_key`] counts by a key a service chooses,
//! such as the signed-in user. [`lock_out_failures`] counts only the `401`s a client
//! earns, so that guessing a password is slowed and ordinary use is not.
//!
//! ```no_run
//! use std::num::NonZeroU32;
//!
//! use axa_request_rate_limit::{Limiter, by_client, lock_out_failures};
//! use axum::{Router, middleware::from_fn_with_state};
//!
//! # fn router(routes: Router, requests: NonZeroU32, failures: NonZeroU32) -> Router {
//! routes
//!     .layer(from_fn_with_state(Limiter::new(failures, 900.try_into().unwrap()), lock_out_failures))
//!     .layer(from_fn_with_state(Limiter::new(requests, 60.try_into().unwrap()), by_client))
//! # }
//! ```
//!
//! # Where the layers go
//!
//! Add them inside `axa::middleware::apply`, which is where the client's address is
//! worked out and where a refusal gets its request id and security headers. Add them
//! after the authentication layer, so that they sit outside it and see its `401`s.
//!
//! Behind a proxy, `TRUSTED_PROXIES` has to list it. Without that every request seems
//! to come from the proxy, and all clients share one limit. [`by_client`] warns once,
//! with the address left out, when the first [`SHARED_ALLOWANCE_SAMPLE`] requests it
//! counts all came from one private or loopback address, which is how that looks.
//!
//! # The key
//!
//! [`client_key`] reads the address `axa` resolved, as a plain [`IpAddr`] request
//! extension. It never reads `X-Forwarded-For`, because a caller writes the first entry
//! of that and could buy a fresh limit with each request. An IPv6 address counts by its
//! `/64`, which is what one subscriber is usually given. A request with no address
//! shares one limit with every other such request.
//!
//! For a limit of your own, such as a tighter one on one route, call [`Limiter::check`]
//! with any key that implements [`Hash`], and answer with [`Verdict::refusal`] or
//! [`Verdict::stamp`].
//!
//! # Counting by user
//!
//! [`Limiter::key`] takes a function from the request to a key, and [`by_key`] counts
//! by it. A function that returns `None` leaves the request uncounted, so a limit that
//! counts only signed-in users stays true. For several limits, such as a tight one on
//! comparisons and a looser one on everything else, use one limiter for each class, or
//! one keyed by `(subject, class)`.
//!
//! ```no_run
//! use std::num::NonZeroU32;
//!
//! use axa_request_rate_limit::{Limiter, by_key};
//! use axum::{Router, extract::Request, middleware::from_fn_with_state};
//!
//! /// The signed-in user, as the service's authentication layer left it.
//! #[derive(Clone)]
//! struct Subject(String);
//!
//! # fn router(routes: Router, limit: NonZeroU32) -> Router {
//! let per_user = Limiter::new(limit, 60.try_into().unwrap())
//!     .key(|request: &Request| request.extensions().get::<Subject>().map(|s| s.0.clone()));
//! routes.layer(from_fn_with_state(per_user, by_key))
//! # }
//! ```
//!
//! # A full table
//!
//! Past [`MAX_KEYS`], a key not yet held has nowhere of its own to be counted.
//! [`WhenFull`] says what happens to it. The default, [`WhenFull::Share`], counts all
//! such keys against one shared limit. A service where a refusal can harm someone,
//! such as one that holds care records, should choose [`WhenFull::Allow`], and count
//! [`Verdict::overflowed`] in a metric, because an overflow that passes unseen hides
//! an attack.
//!
//! # Seeing a refusal
//!
//! Every response from [`by_client`], [`by_key`], and [`lock_out_failures`] carries
//! its [`Verdict`] as a response extension, refused or not. [`Verdict::first_refusal`]
//! is true once per key per window, so a service can write one audit row for a client
//! that went over, rather than one for every refused request. The service does that in
//! its own layer outside the limiter, which sees the request and the response:
//!
//! ```no_run
//! use axa_request_rate_limit::Verdict;
//! use axum::{extract::Request, middleware::Next, response::Response};
//!
//! async fn audit_refusals(request: Request, next: Next) -> Response {
//!     let path = request.uri().path().to_owned();
//!     let response = next.run(request).await;
//!     if response.extensions().get::<Verdict>().is_some_and(|v| v.first_refusal) {
//!         // Write the audit row for `path`, and count the refusal in a metric. If the
//!         // write fails, the service decides: log it and count it, or answer otherwise.
//!         let _ = path;
//!     }
//!     response
//! }
//! ```
//!
//! # Lifting one key's limit
//!
//! [`Limiter::set_limit_for`] gives one key its own limit, such as a higher one for an
//! integration, from the next request on, with no restart. [`Limiter::clear_limit_for`]
//! takes it back. A key with a limit of its own is always held, even when the table is
//! full, and at most [`MAX_OVERRIDES`] are kept. Past that, setting another is refused.
//!
//! # What is kept
//!
//! A key is held only as a keyed hash, and the first request after two windows drops
//! it. An address is personal data, so the limiter neither stores nor logs one. A key
//! with a limit of its own is held as a keyed hash too.
//!
//! # Limits
//!
//! Each process counts for itself, so with several replicas a client gets one limit
//! from each. A client can send up to twice the limit across the end of a window.
//! A limit protects a service from a noisy client. It does not stop a distributed attack.

use std::{
    collections::HashMap,
    fmt,
    hash::{BuildHasher, Hash, RandomState},
    net::{IpAddr, Ipv6Addr},
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axa_problem_details_for_http_apis::Problem;
use axum::{
    extract::{Request, State},
    http::{Extensions, HeaderName, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Paths never limited. A refused probe would restart a healthy service.
pub const EXEMPT: [&str; 3] = ["/_ping", "/_health", "/_readiness"];

/// The most keys held at once, so that many clients cost a bounded amount of memory.
pub const MAX_KEYS: usize = 100_000;

/// The most keys with a limit of their own held at once.
pub const MAX_OVERRIDES: usize = 10_000;

/// How many requests [`by_client`] looks at before it warns that every one came from
/// one private or loopback address.
pub const SHARED_ALLOWANCE_SAMPLE: u32 = 100;

const LIMIT: HeaderName = HeaderName::from_static("ratelimit-limit");
const REMAINING: HeaderName = HeaderName::from_static("ratelimit-remaining");
const RESET: HeaderName = HeaderName::from_static("ratelimit-reset");

/// What happens to a key not yet held once [`MAX_KEYS`] are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WhenFull {
    /// All such keys share one limit. This is the default.
    #[default]
    Share,
    /// Such a key goes ahead uncounted. Choose this where a refusal can harm someone.
    Allow,
    /// Such a key is refused until the table has room.
    Refuse,
}

/// One key's count in its window.
#[derive(Debug, Clone, Copy)]
struct Slot {
    start: Instant,
    count: u32,
    /// Whether a refusal has been answered in this window.
    refused: bool,
}

impl Slot {
    const fn new(start: Instant) -> Self {
        Self {
            start,
            count: 0,
            refused: false,
        }
    }
}

/// How one request is counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Count it if it goes ahead.
    Charge,
    /// Say what a count would, and change nothing.
    Peek,
    /// Count nothing, but a refusal is answered, so it is the window's refusal.
    Gate,
}

/// The counts. A sweep is at most once a window, so that a table full of live keys
/// does not make every new key scan it.
#[derive(Debug)]
struct Table {
    slots: HashMap<u64, Slot>,
    /// Shared by new keys once `slots` is full.
    overflow: Slot,
    next_sweep: Instant,
    /// Keys with a limit of their own.
    overrides: HashMap<u64, u32>,
    /// Whether every client so far looks like one proxy.
    shared: Shared,
}

/// What [`by_client`] has seen of where its requests come from. It holds the client
/// only as a keyed hash.
#[derive(Debug, Clone, Copy, Default)]
enum Shared {
    /// Every request so far came from this one private or loopback client.
    Watching { client: u64, requests: u32 },
    /// No request yet.
    #[default]
    Unseen,
    /// Warned, or seen more than one client, or a public one. Nothing more to say.
    Settled,
}

/// A limit on requests per key in a fixed window.
///
/// Clones share their counts and their per-key limits.
#[derive(Debug, Clone)]
pub struct Limiter {
    limit: u32,
    window: Duration,
    max_keys: usize,
    max_overrides: usize,
    when_full: WhenFull,
    hasher: RandomState,
    table: Arc<Mutex<Table>>,
}

/// What a limiter says about one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Whether the request may go ahead.
    pub allowed: bool,
    /// The limit, which is the key's own when it has one.
    pub limit: u32,
    /// Requests left in this window.
    pub remaining: u32,
    /// Whole seconds until this window ends, rounded up, so never too few to wait.
    pub reset_seconds: u64,
    /// Whether the key was not held because the table was full, so [`WhenFull`]
    /// decided.
    pub overflowed: bool,
    /// Whether this is the first refusal for this key in this window. Under
    /// [`WhenFull::Share`] and [`WhenFull::Refuse`], the keys that overflowed count as
    /// one key.
    pub first_refusal: bool,
}

/// A limit of its own was refused, because [`MAX_OVERRIDES`] keys already have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyOverrides;

impl fmt::Display for TooManyOverrides {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("too many keys with a limit of their own")
    }
}

impl std::error::Error for TooManyOverrides {}

impl Limiter {
    /// `limit` requests per key in each window of `window_seconds`.
    ///
    /// Neither can be zero, so a setting of `0` cannot quietly mean either "refuse
    /// everyone" or "no limit". A service with no limit leaves the layer out.
    #[must_use]
    pub fn new(limit: NonZeroU32, window_seconds: NonZeroU32) -> Self {
        let now = Instant::now();
        let window = Duration::from_secs(u64::from(window_seconds.get()));
        Self {
            limit: limit.get(),
            window,
            max_keys: MAX_KEYS,
            max_overrides: MAX_OVERRIDES,
            when_full: WhenFull::default(),
            hasher: RandomState::new(),
            table: Arc::new(Mutex::new(Table {
                slots: HashMap::new(),
                overflow: Slot::new(now),
                next_sweep: now + window,
                overrides: HashMap::new(),
                shared: Shared::Unseen,
            })),
        }
    }

    /// What happens to a key not yet held once [`MAX_KEYS`] are. The default is
    /// [`WhenFull::Share`].
    #[must_use]
    pub const fn when_full(mut self, when_full: WhenFull) -> Self {
        self.when_full = when_full;
        self
    }

    /// Count by a key taken from each request, for [`by_key`]. A request for which
    /// `key` returns `None` is not counted.
    #[must_use]
    pub fn key<K, F>(self, key: F) -> Keyed<K>
    where
        F: Fn(&Request) -> Option<K> + Send + Sync + 'static,
    {
        Keyed {
            limiter: self,
            key: Arc::new(key),
        }
    }

    /// Count one request by `key`.
    pub fn check(&self, key: impl Hash) -> Verdict {
        self.tally(self.hasher.hash_one(key), Mode::Charge, Instant::now())
    }

    /// What [`Self::check`] would say about `key`, counting nothing.
    pub fn peek(&self, key: impl Hash) -> Verdict {
        self.tally(self.hasher.hash_one(key), Mode::Peek, Instant::now())
    }

    /// Give `key` a limit of its own from its next request, in place of the limiter's.
    /// A key with one is always held, even when the table is full.
    ///
    /// # Errors
    ///
    /// [`TooManyOverrides`] when [`MAX_OVERRIDES`] other keys already have one. None
    /// is dropped to make room.
    pub fn set_limit_for(&self, key: impl Hash, limit: NonZeroU32) -> Result<(), TooManyOverrides> {
        let key = self.hasher.hash_one(key);
        let mut table = self.lock();
        let overrides = &mut table.overrides;
        if !overrides.contains_key(&key) && overrides.len() >= self.max_overrides {
            return Err(TooManyOverrides);
        }
        overrides.insert(key, limit.get());
        Ok(())
    }

    /// Take back `key`'s limit of its own, so the limiter's applies from its next
    /// request. Says whether it had one.
    pub fn clear_limit_for(&self, key: impl Hash) -> bool {
        let key = self.hasher.hash_one(key);
        self.lock().overrides.remove(&key).is_some()
    }

    /// Note one request [`by_client`] counted, from `client` at `address`, and say
    /// whether this is the moment to warn that all clients share one limit.
    fn one_proxy(&self, client: u64, address: Option<IpAddr>) -> bool {
        let private = address.is_some_and(|address| match address.to_canonical() {
            IpAddr::V4(v4) => v4.is_private() || v4.is_loopback(),
            IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
        });
        let mut table = self.lock();
        let next = match table.shared {
            Shared::Settled => return false,
            _ if !private => Shared::Settled,
            Shared::Unseen => Shared::Watching {
                client,
                requests: 1,
            },
            Shared::Watching {
                client: seen,
                requests,
            } if seen == client => Shared::Watching {
                client,
                requests: requests + 1,
            },
            Shared::Watching { .. } => Shared::Settled,
        };
        let warn = matches!(next, Shared::Watching { requests, .. } if requests >= SHARED_ALLOWANCE_SAMPLE);
        table.shared = if warn { Shared::Settled } else { next };
        warn
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        // A poisoned lock holds counts, which are safe to go on using.
        self.table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    fn count(&self, key: u64, charge: bool, now: Instant) -> Verdict {
        self.tally(key, if charge { Mode::Charge } else { Mode::Peek }, now)
    }

    fn tally(&self, key: u64, mode: Mode, now: Instant) -> Verdict {
        let window = self.window;
        let mut table = self.lock();
        let Table {
            slots,
            overflow,
            next_sweep,
            overrides,
            ..
        } = &mut *table;
        if now >= *next_sweep {
            slots.retain(|_, slot| now.saturating_duration_since(slot.start) < window);
            *next_sweep = now + window;
        }
        let own_limit = overrides.get(&key).copied();
        let limit = own_limit.unwrap_or(self.limit);
        let known = slots.contains_key(&key);
        let full = !known && own_limit.is_none() && slots.len() >= self.max_keys;
        let slot = if full {
            match self.when_full {
                WhenFull::Allow => {
                    let mut verdict = self.verdict(Slot::new(now), limit, true, now);
                    verdict.overflowed = true;
                    return verdict;
                }
                WhenFull::Refuse => {
                    if now.saturating_duration_since(overflow.start) >= window {
                        *overflow = Slot::new(now);
                    }
                    let mut verdict = self.verdict(*overflow, limit, false, now);
                    verdict.remaining = 0;
                    verdict.overflowed = true;
                    if mode != Mode::Peek {
                        verdict.first_refusal = !overflow.refused;
                        overflow.refused = true;
                    }
                    return verdict;
                }
                WhenFull::Share => overflow,
            }
        } else if !known && mode != Mode::Charge {
            return self.verdict(Slot::new(now), limit, true, now);
        } else {
            slots.entry(key).or_insert_with(|| Slot::new(now))
        };
        if now.saturating_duration_since(slot.start) >= window {
            *slot = Slot::new(now);
        }
        let allowed = slot.count < limit;
        if allowed && mode == Mode::Charge {
            slot.count += 1;
        }
        let mut verdict = self.verdict(*slot, limit, allowed, now);
        verdict.overflowed = full;
        if !allowed && mode != Mode::Peek {
            verdict.first_refusal = !slot.refused;
            slot.refused = true;
        }
        verdict
    }

    fn verdict(&self, slot: Slot, limit: u32, allowed: bool, now: Instant) -> Verdict {
        let left = (slot.start + self.window).saturating_duration_since(now);
        Verdict {
            allowed,
            limit,
            remaining: limit.saturating_sub(slot.count),
            reset_seconds: left.as_secs() + u64::from(left.subsec_nanos() > 0),
            overflowed: false,
            first_refusal: false,
        }
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        self.table.lock().unwrap().slots.len()
    }
}

/// What [`Limiter::key`] takes: the key a request is counted by, if any.
type KeyOf<K> = dyn Fn(&Request) -> Option<K> + Send + Sync;

/// A [`Limiter`] that counts by a key taken from each request, for [`by_key`]. Made
/// with [`Limiter::key`].
///
/// Clones share their counts.
pub struct Keyed<K> {
    limiter: Limiter,
    key: Arc<KeyOf<K>>,
}

impl<K> Clone for Keyed<K> {
    fn clone(&self) -> Self {
        Self {
            limiter: self.limiter.clone(),
            key: Arc::clone(&self.key),
        }
    }
}

impl<K> fmt::Debug for Keyed<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyed")
            .field("limiter", &self.limiter)
            .finish_non_exhaustive()
    }
}

impl<K: Hash> Keyed<K> {
    /// The limiter underneath, which counts the same keys.
    #[must_use]
    pub const fn limiter(&self) -> &Limiter {
        &self.limiter
    }

    /// Give `key` a limit of its own. See [`Limiter::set_limit_for`].
    ///
    /// # Errors
    ///
    /// [`TooManyOverrides`] when [`MAX_OVERRIDES`] other keys already have one.
    pub fn set_limit_for(&self, key: &K, limit: NonZeroU32) -> Result<(), TooManyOverrides> {
        self.limiter.set_limit_for(key, limit)
    }

    /// Take back `key`'s limit of its own. See [`Limiter::clear_limit_for`].
    pub fn clear_limit_for(&self, key: &K) -> bool {
        self.limiter.clear_limit_for(key)
    }
}

impl Verdict {
    /// Add `RateLimit-Limit`, `RateLimit-Remaining`, and `RateLimit-Reset`, and the
    /// verdict itself as a response extension.
    pub fn stamp(&self, response: &mut Response) {
        let headers = response.headers_mut();
        headers.insert(LIMIT, HeaderValue::from(self.limit));
        headers.insert(REMAINING, HeaderValue::from(self.remaining));
        headers.insert(RESET, HeaderValue::from(self.reset_seconds));
        response.extensions_mut().insert(*self);
    }

    /// The `429`, with `Retry-After`, the `RateLimit-*` headers, and the verdict as a
    /// response extension.
    #[must_use]
    pub fn refusal(&self) -> Response {
        let mut response = Problem::new(StatusCode::TOO_MANY_REQUESTS)
            .with_extension("code", serde_json::json!("too_many_requests"))
            .into_response();
        self.stamp(&mut response);
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(self.reset_seconds));
        response
    }
}

/// The client a request is counted against: the address `axa` resolved, with an IPv6
/// address reduced to its `/64`, or `None` if there is none.
#[must_use]
pub fn client_key(extensions: &Extensions) -> Option<IpAddr> {
    let address = extensions.get::<IpAddr>()?.to_canonical();
    Some(match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(address) => IpAddr::V6(Ipv6Addr::from_bits(address.to_bits() & (!0u128 << 64))),
    })
}

/// Answer `verdict`'s refusal, or run the request and stamp what it answers.
async fn limited(verdict: Verdict, request: Request, next: Next) -> Response {
    if !verdict.allowed {
        return verdict.refusal();
    }
    let mut response = next.run(request).await;
    verdict.stamp(&mut response);
    response
}

/// Limit each client. Every response gets the `RateLimit-*` headers and its
/// [`Verdict`], apart from the probes in [`EXEMPT`], which are not counted.
///
/// Once per limiter, it warns if the first [`SHARED_ALLOWANCE_SAMPLE`] requests it
/// counts all came from one private or loopback address, as they do from a proxy
/// that `TRUSTED_PROXIES` does not list. The warning never holds the address.
pub async fn by_client(State(limiter): State<Limiter>, request: Request, next: Next) -> Response {
    if EXEMPT.contains(&request.uri().path()) {
        return next.run(request).await;
    }
    let client = limiter.hasher.hash_one(client_key(request.extensions()));
    let verdict = limiter.tally(client, Mode::Charge, Instant::now());
    if limiter.one_proxy(client, request.extensions().get::<IpAddr>().copied()) {
        // Never the address: the crate neither stores nor logs one.
        tracing::warn!(
            requests = SHARED_ALLOWANCE_SAMPLE,
            "ratelimit_one_client: every request came from one private address. Behind a \
             proxy, list it in TRUSTED_PROXIES, or all clients share one limit"
        );
    }
    limited(verdict, request, next).await
}

/// Limit by the key [`Limiter::key`] takes from each request. A request with no key is
/// not counted and gets no `RateLimit-*` headers, and neither do the probes in
/// [`EXEMPT`]. Every other response gets them and its [`Verdict`].
pub async fn by_key<K: Hash + Send + 'static>(
    State(keyed): State<Keyed<K>>,
    request: Request,
    next: Next,
) -> Response {
    if EXEMPT.contains(&request.uri().path()) {
        return next.run(request).await;
    }
    let Some(key) = (keyed.key)(&request) else {
        return next.run(request).await;
    };
    let verdict = keyed.limiter.check(key);
    limited(verdict, request, next).await
}

/// Refuse a client that has earned the limit in `401`s, until the window ends. Only a
/// `401` counts, so a client that signs in correctly is never slowed. Every response
/// it counts gets its [`Verdict`].
///
/// A `401` is counted when it is answered, so guesses sent at once can pass the limit
/// by however many are in flight. [`by_client`] outside this layer caps that.
pub async fn lock_out_failures(
    State(limiter): State<Limiter>,
    request: Request,
    next: Next,
) -> Response {
    if EXEMPT.contains(&request.uri().path()) {
        return next.run(request).await;
    }
    let key = limiter.hasher.hash_one(client_key(request.extensions()));
    let verdict = limiter.tally(key, Mode::Gate, Instant::now());
    if !verdict.allowed {
        return verdict.refusal();
    }
    let mut response = next.run(request).await;
    let verdict = if response.status() == StatusCode::UNAUTHORIZED {
        limiter.tally(key, Mode::Charge, Instant::now())
    } else {
        verdict
    };
    response.extensions_mut().insert(verdict);
    response
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, http, middleware::from_fn_with_state, routing::get};
    use tower::ServiceExt;

    use super::*;

    fn limiter(limit: u32, window_seconds: u32) -> Limiter {
        Limiter::new(
            limit.try_into().unwrap(),
            window_seconds.try_into().unwrap(),
        )
    }

    const fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn the_limit_is_exact_and_a_refusal_is_not_counted() {
        let limiter = limiter(2, 60);
        let now = Instant::now();
        assert!(limiter.count(1, true, now).allowed);
        let second = limiter.count(1, true, now);
        assert!(second.allowed);
        assert_eq!(second.remaining, 0);
        assert!(!limiter.count(1, true, now).allowed);
        assert!(!limiter.count(1, true, now).allowed);
        assert!(limiter.count(1, true, now + secs(60)).allowed);
    }

    #[test]
    fn keys_are_counted_apart() {
        let limiter = limiter(1, 60);
        assert!(limiter.check("a").allowed);
        assert!(!limiter.check("a").allowed);
        assert!(limiter.check("b").allowed);
    }

    #[test]
    fn clones_share_their_counts() {
        let first = limiter(1, 60);
        assert!(first.clone().check("a").allowed);
        assert!(!first.check("a").allowed);
    }

    #[test]
    fn the_wait_is_rounded_up_and_never_too_short() {
        let limiter = limiter(1, 60);
        let now = Instant::now();
        assert_eq!(limiter.count(1, true, now).reset_seconds, 60);
        let later = now + Duration::from_millis(59_001);
        assert_eq!(limiter.count(1, true, later).reset_seconds, 1);
    }

    #[test]
    fn a_peek_counts_nothing_and_stores_nothing() {
        let limiter = limiter(1, 60);
        assert!(limiter.peek("a").allowed);
        assert_eq!(limiter.held(), 0);
        limiter.check("a");
        assert!(!limiter.peek("a").allowed);
    }

    #[test]
    fn old_keys_are_dropped_within_two_windows() {
        let limiter = limiter(5, 60);
        let now = Instant::now();
        limiter.count(1, true, now);
        limiter.count(2, true, now + secs(30));
        limiter.count(3, true, now + secs(61));
        assert_eq!(limiter.held(), 2, "key 2 is still in its window");
        limiter.count(3, true, now + secs(121));
        assert_eq!(limiter.held(), 1);
    }

    #[test]
    fn a_key_goes_the_moment_its_window_ends() {
        let limiter = limiter(5, 60);
        let now = Instant::now();
        limiter.count(1, true, now);
        limiter.count(2, true, now + secs(60));
        assert_eq!(limiter.held(), 1);
    }

    #[test]
    fn a_full_table_shares_one_limit_and_never_grows() {
        let mut limiter = limiter(2, 60);
        limiter.max_keys = 2;
        let now = Instant::now();
        limiter.count(1, true, now);
        limiter.count(2, true, now);
        assert!(limiter.count(3, true, now).allowed);
        assert!(limiter.count(4, true, now).allowed);
        assert!(!limiter.count(5, true, now).allowed, "they share a limit");
        assert_eq!(limiter.held(), 2);
        assert!(
            limiter.count(1, true, now).allowed,
            "a held key keeps its own"
        );
    }

    #[test]
    fn a_table_full_of_live_keys_is_not_scanned_for_each_new_key() {
        let mut limiter = limiter(1, 60);
        limiter.max_keys = 1;
        let now = Instant::now();
        limiter.count(1, true, now + secs(1));
        // Past the first sweep, which moves the next one a whole window on.
        limiter.count(2, true, now + secs(60));
        let next_sweep = limiter.table.lock().unwrap().next_sweep;
        limiter.count(3, true, now + secs(61));
        assert_eq!(limiter.table.lock().unwrap().next_sweep, next_sweep);
    }

    #[test]
    fn a_key_is_held_only_as_a_hash() {
        let limiter = limiter(1, 60);
        limiter.check("203.0.113.9");
        assert!(!format!("{limiter:?}").contains("203.0.113.9"));
    }

    fn with_address(address: &str) -> Extensions {
        let mut extensions = Extensions::new();
        extensions.insert(address.parse::<IpAddr>().unwrap());
        extensions
    }

    #[test]
    fn one_ipv6_subscriber_counts_once() {
        let key = |address| client_key(&with_address(address));
        assert_eq!(key("2001:db8:1:2::1"), key("2001:db8:1:2:ffff::9"));
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
    }

    #[test]
    fn an_ipv4_client_counts_the_same_however_it_arrives() {
        let key = |address| client_key(&with_address(address));
        assert_eq!(key("::ffff:203.0.113.9"), key("203.0.113.9"));
        assert_ne!(key("203.0.113.9"), key("203.0.113.10"));
    }

    fn request(path: &str, address: Option<&str>) -> Request {
        let mut request = http::Request::get(path).body(Body::empty()).unwrap();
        if let Some(address) = address {
            *request.extensions_mut() = with_address(address);
        }
        request
    }

    fn limited(limiter: Limiter) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .route("/_ping", get(|| async { "ok" }))
            .layer(from_fn_with_state(limiter, by_client))
    }

    #[tokio::test]
    async fn a_refusal_says_how_long_to_wait() {
        let app = limited(limiter(1, 60));
        let first = app.clone().oneshot(request("/", Some("203.0.113.9"))).await;
        let first = first.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()["ratelimit-limit"], "1");
        assert_eq!(first.headers()["ratelimit-remaining"], "0");
        assert!(first.headers().get(header::RETRY_AFTER).is_none());

        let second = app
            .oneshot(request("/", Some("203.0.113.9")))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(second.headers()["ratelimit-remaining"], "0");
        let wait = second.headers()[header::RETRY_AFTER].to_str().unwrap();
        assert!((1..=60).contains(&wait.parse::<u64>().unwrap()));
        assert_eq!(
            second.headers()[header::CONTENT_TYPE],
            axa_problem_details_for_http_apis::PROBLEM_JSON
        );
        let body = axum::body::to_bytes(second.into_body(), usize::MAX).await;
        let body: serde_json::Value = serde_json::from_slice(&body.unwrap()).unwrap();
        assert_eq!(body["status"], 429);
        assert_eq!(body["code"], "too_many_requests");
    }

    #[tokio::test]
    async fn the_probes_are_never_refused_or_counted() {
        let limiter = limiter(1, 60);
        let app = limited(limiter.clone());
        for _ in 0..3 {
            let response = app.clone().oneshot(request("/_ping", None)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().get("ratelimit-limit").is_none());
        }
        assert_eq!(limiter.held(), 0);
    }

    #[tokio::test]
    async fn requests_with_no_address_share_one_limit() {
        let app = limited(limiter(1, 60));
        let first = app.clone().oneshot(request("/", None)).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = app.oneshot(request("/", None)).await.unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    fn guarded(limiter: Limiter) -> Router {
        Router::new()
            .route(
                "/",
                get(|headers: http::HeaderMap| async move {
                    if headers.contains_key(header::AUTHORIZATION) {
                        StatusCode::OK
                    } else {
                        StatusCode::UNAUTHORIZED
                    }
                }),
            )
            .layer(from_fn_with_state(limiter, lock_out_failures))
    }

    fn signed_in(address: &str) -> Request {
        let mut request = request("/", Some(address));
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
        request
    }

    #[tokio::test]
    async fn failures_lock_a_client_out_and_only_that_client() {
        let app = guarded(limiter(2, 60));
        for _ in 0..2 {
            let response = app.clone().oneshot(request("/", Some("203.0.113.9"))).await;
            assert_eq!(response.unwrap().status(), StatusCode::UNAUTHORIZED);
        }
        let locked = app.clone().oneshot(signed_in("203.0.113.9")).await.unwrap();
        assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(locked.headers().contains_key(header::RETRY_AFTER));
        let other = app.oneshot(signed_in("203.0.113.10")).await.unwrap();
        assert_eq!(other.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn success_never_counts_towards_a_lockout() {
        let limiter = limiter(1, 60);
        let app = guarded(limiter.clone());
        for _ in 0..3 {
            let response = app.clone().oneshot(signed_in("203.0.113.9")).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(limiter.held(), 0);
    }

    // Keyed limits.

    #[derive(Clone)]
    struct Subject(&'static str);

    fn by_subject(limiter: Limiter) -> Router {
        let keyed =
            limiter.key(|request: &Request| request.extensions().get::<Subject>().map(|s| s.0));
        Router::new()
            .route("/", get(|| async { "ok" }))
            .route("/_ping", get(|| async { "ok" }))
            .layer(from_fn_with_state(keyed, by_key))
    }

    fn as_subject(path: &str, subject: Option<&'static str>) -> Request {
        let mut request = request(path, Some("203.0.113.9"));
        if let Some(subject) = subject {
            request.extensions_mut().insert(Subject(subject));
        }
        request
    }

    #[tokio::test]
    async fn a_keyed_limit_counts_by_the_key_it_takes() {
        let app = by_subject(limiter(1, 60));
        let ana = app
            .clone()
            .oneshot(as_subject("/", Some("ana")))
            .await
            .unwrap();
        assert_eq!(ana.status(), StatusCode::OK);
        assert_eq!(ana.headers()["ratelimit-limit"], "1");
        let again = app
            .clone()
            .oneshot(as_subject("/", Some("ana")))
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::TOO_MANY_REQUESTS);
        // The same address, another subject: counted apart.
        let bob = app.oneshot(as_subject("/", Some("bob"))).await.unwrap();
        assert_eq!(bob.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_request_with_no_key_is_not_counted() {
        let limiter = limiter(1, 60);
        let app = by_subject(limiter.clone());
        for _ in 0..3 {
            let response = app.clone().oneshot(as_subject("/", None)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().get("ratelimit-limit").is_none());
            assert!(response.extensions().get::<Verdict>().is_none());
        }
        assert_eq!(limiter.held(), 0);
    }

    #[tokio::test]
    async fn keyed_probes_are_never_refused_or_counted() {
        let limiter = limiter(1, 60);
        let app = by_subject(limiter.clone());
        for _ in 0..3 {
            let response = app.clone().oneshot(as_subject("/_ping", Some("ana"))).await;
            assert_eq!(response.unwrap().status(), StatusCode::OK);
        }
        assert_eq!(limiter.held(), 0);
    }

    // A full table.

    fn full(when_full: WhenFull) -> Limiter {
        let mut limiter = limiter(1, 60).when_full(when_full);
        limiter.max_keys = 1;
        limiter
    }

    #[test]
    fn a_full_table_shares_by_default_and_says_so() {
        let limiter = full(WhenFull::default());
        let now = Instant::now();
        assert!(!limiter.count(1, true, now).overflowed, "a held key");
        let shared = limiter.count(2, true, now);
        assert!(shared.allowed && shared.overflowed);
        assert!(!limiter.count(3, true, now).allowed, "they share a limit");
    }

    #[test]
    fn a_full_table_can_let_new_keys_through() {
        let limiter = full(WhenFull::Allow);
        let now = Instant::now();
        limiter.count(1, true, now);
        for key in 2..10 {
            let verdict = limiter.count(key, true, now);
            assert!(verdict.allowed && verdict.overflowed);
            assert!(!verdict.first_refusal);
        }
        assert_eq!(limiter.held(), 1);
        assert!(
            !limiter.count(1, true, now).allowed,
            "a held key keeps its own"
        );
    }

    #[test]
    fn a_full_table_can_refuse_new_keys() {
        let limiter = full(WhenFull::Refuse);
        let now = Instant::now();
        limiter.count(1, true, now);
        let first = limiter.count(2, true, now);
        assert!(!first.allowed && first.overflowed && first.first_refusal);
        assert_eq!(first.remaining, 0);
        let second = limiter.count(3, true, now);
        assert!(!second.allowed && !second.first_refusal);
        assert_eq!(limiter.held(), 1);
        assert!(limiter.count(2, true, now + secs(61)).allowed, "room again");
    }

    // The first refusal.

    #[test]
    fn the_first_refusal_in_a_window_is_marked_once() {
        let limiter = limiter(1, 60);
        let now = Instant::now();
        assert!(!limiter.count(1, true, now).first_refusal, "allowed");
        assert!(limiter.count(1, true, now).first_refusal);
        assert!(!limiter.count(1, true, now).first_refusal);
        assert!(!limiter.count(1, true, now + secs(30)).first_refusal);
        assert!(limiter.count(1, true, now + secs(60)).allowed);
        assert!(
            limiter.count(1, true, now + secs(61)).first_refusal,
            "a new window"
        );
    }

    #[test]
    fn a_window_that_ends_between_sweeps_clears_the_refusal() {
        let limiter = limiter(1, 60);
        let now = Instant::now();
        limiter.count(1, true, now + secs(1));
        assert!(limiter.count(1, true, now + secs(1)).first_refusal);
        // A sweep that leaves key 1 held, because its window has a second to go.
        limiter.count(2, true, now + secs(60));
        assert_eq!(limiter.held(), 2);
        // Key 1's window ends in its slot, so the slot itself must start afresh.
        assert!(limiter.count(1, true, now + secs(61)).allowed);
        assert!(limiter.count(1, true, now + secs(61)).first_refusal);
    }

    #[test]
    fn a_peek_never_takes_the_first_refusal() {
        let limiter = limiter(1, 60);
        let now = Instant::now();
        limiter.count(1, true, now);
        let peeked = limiter.count(1, false, now);
        assert!(!peeked.allowed && !peeked.first_refusal);
        assert!(limiter.count(1, true, now).first_refusal);
    }

    #[test]
    fn overflowed_keys_share_one_first_refusal() {
        let limiter = full(WhenFull::Share);
        let now = Instant::now();
        limiter.count(1, true, now);
        limiter.count(2, true, now);
        assert!(limiter.count(3, true, now).first_refusal);
        assert!(!limiter.count(4, true, now).first_refusal);
    }

    // The verdict on every response.

    #[tokio::test]
    async fn every_response_carries_its_verdict() {
        let app = limited(limiter(1, 60));
        let allowed = app
            .clone()
            .oneshot(request("/", Some("203.0.113.9")))
            .await
            .unwrap();
        let verdict = allowed.extensions().get::<Verdict>().copied().unwrap();
        assert!(verdict.allowed && !verdict.first_refusal);
        let refused = app
            .clone()
            .oneshot(request("/", Some("203.0.113.9")))
            .await
            .unwrap();
        let verdict = refused.extensions().get::<Verdict>().copied().unwrap();
        assert!(!verdict.allowed && verdict.first_refusal);
        let again = app
            .oneshot(request("/", Some("203.0.113.9")))
            .await
            .unwrap();
        assert!(!again.extensions().get::<Verdict>().unwrap().first_refusal);

        let keyed = by_subject(limiter(1, 60));
        let response = keyed.oneshot(as_subject("/", Some("ana"))).await.unwrap();
        assert!(response.extensions().get::<Verdict>().is_some());
    }

    #[tokio::test]
    async fn a_lockout_marks_its_first_refusal_and_carries_its_verdict() {
        let app = guarded(limiter(1, 60));
        let failed = app
            .clone()
            .oneshot(request("/", Some("203.0.113.9")))
            .await
            .unwrap();
        assert_eq!(failed.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(failed.extensions().get::<Verdict>().unwrap().remaining, 0);
        let locked = app.clone().oneshot(signed_in("203.0.113.9")).await.unwrap();
        assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(locked.extensions().get::<Verdict>().unwrap().first_refusal);
        let again = app.oneshot(signed_in("203.0.113.9")).await.unwrap();
        assert!(!again.extensions().get::<Verdict>().unwrap().first_refusal);
    }

    // A key's own limit.

    #[test]
    fn a_key_can_have_a_limit_of_its_own_with_no_restart() {
        let limiter = limiter(1, 60);
        assert!(limiter.check("integration").allowed);
        assert!(!limiter.check("integration").allowed);
        limiter
            .set_limit_for("integration", 3.try_into().unwrap())
            .unwrap();
        let lifted = limiter.check("integration");
        assert!(lifted.allowed);
        assert_eq!((lifted.limit, lifted.remaining), (3, 1));
        assert_eq!(limiter.check("other").limit, 1, "only that key");
        assert!(
            limiter.clone().clear_limit_for("integration"),
            "clones share them"
        );
        let back = limiter.check("integration");
        assert!(!back.allowed);
        assert_eq!((back.limit, back.remaining), (1, 0));
        assert!(!limiter.clear_limit_for("integration"));
    }

    #[test]
    fn a_key_with_its_own_limit_is_held_when_the_table_is_full() {
        let limiter = full(WhenFull::Refuse);
        let now = Instant::now();
        limiter.count(1, true, now);
        limiter.set_limit_for(2_u64, 5.try_into().unwrap()).unwrap();
        let key = limiter.hasher.hash_one(2_u64);
        let verdict = limiter.count(key, true, now);
        assert!(verdict.allowed && !verdict.overflowed);
        assert_eq!(verdict.limit, 5);
    }

    #[test]
    fn too_many_own_limits_are_refused_and_none_is_dropped() {
        let mut limiter = limiter(1, 60);
        limiter.max_overrides = 2;
        let five = 5.try_into().unwrap();
        limiter.set_limit_for("a", five).unwrap();
        limiter.set_limit_for("b", five).unwrap();
        assert_eq!(limiter.set_limit_for("c", five), Err(TooManyOverrides));
        assert_eq!(
            limiter.set_limit_for("a", 9.try_into().unwrap()),
            Ok(()),
            "a change"
        );
        assert_eq!(limiter.check("a").limit, 9);
        assert_eq!(limiter.check("b").limit, 5);
    }

    #[test]
    fn an_own_limit_is_held_only_as_a_hash() {
        let limiter = limiter(1, 60);
        limiter
            .set_limit_for("integration-7", 5.try_into().unwrap())
            .unwrap();
        assert!(!format!("{limiter:?}").contains("integration-7"));
        let keyed = limiter.key(|_: &Request| Some("x"));
        assert!(!format!("{keyed:?}").contains("integration-7"));
    }

    // The warning about one proxy.

    /// Every event logged, as `LEVEL field=value ...`, with no subscriber crate.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<String>>>);

    impl tracing::Subscriber for Captured {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            struct Text(String);
            impl tracing::field::Visit for Text {
                fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
                    use fmt::Write;
                    let _ = write!(self.0, " {}={value:?}", field.name());
                }
            }
            let mut text = Text(event.metadata().level().to_string());
            event.record(&mut text);
            self.0.lock().unwrap().push(text.0);
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// Send a request to `/` from each address in turn, and return what was logged.
    async fn logged_for(addresses: impl IntoIterator<Item = &'static str>) -> Vec<String> {
        let captured = Captured::default();
        let _guard = tracing::dispatcher::set_default(&tracing::Dispatch::new(captured.clone()));
        let app = limited(limiter(10_000, 60));
        for address in addresses {
            app.clone()
                .oneshot(request("/", Some(address)))
                .await
                .unwrap();
        }
        captured.0.lock().unwrap().clone()
    }

    fn warnings(logged: &[String]) -> Vec<&String> {
        logged
            .iter()
            .filter(|line| line.starts_with("WARN"))
            .collect()
    }

    #[tokio::test]
    async fn one_private_client_is_warned_about_once_and_never_named() {
        let logged = logged_for(std::iter::repeat_n("10.0.0.7", 99)).await;
        assert!(
            warnings(&logged).is_empty(),
            "not before the sample is full"
        );

        let logged = logged_for(std::iter::repeat_n("10.0.0.7", 250)).await;
        let warned = warnings(&logged);
        assert_eq!(warned.len(), 1, "{logged:?}");
        assert!(warned[0].contains("TRUSTED_PROXIES"), "{}", warned[0]);
        assert!(
            !logged.iter().any(|line| line.contains("10.0.0")),
            "{logged:?}"
        );
    }

    #[tokio::test]
    async fn a_loopback_client_is_warned_about_too() {
        let logged = logged_for(std::iter::repeat_n("::1", 100)).await;
        assert_eq!(warnings(&logged).len(), 1, "{logged:?}");
        assert!(
            !logged.iter().any(|line| line.contains("::1")),
            "{logged:?}"
        );
    }

    #[tokio::test]
    async fn mixed_clients_are_not_warned_about() {
        let addresses = std::iter::repeat_n("10.0.0.7", 99)
            .chain(["10.0.0.8"])
            .chain(std::iter::repeat_n("10.0.0.7", 200));
        assert_eq!(warnings(&logged_for(addresses).await).len(), 0);
    }

    #[tokio::test]
    async fn a_public_client_is_not_warned_about() {
        let logged = logged_for(std::iter::repeat_n("203.0.113.9", 150)).await;
        assert_eq!(warnings(&logged).len(), 0);
        let addresses = ["203.0.113.9"]
            .into_iter()
            .chain(std::iter::repeat_n("10.0.0.7", 150));
        assert_eq!(warnings(&logged_for(addresses).await).len(), 0);
    }
}
