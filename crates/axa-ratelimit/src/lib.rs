//! In-process request limits and failed-login lockout.
//!
//! A [`Limiter`] counts requests per key in a fixed window. [`by_client`] is the common
//! case, one limit per client address. [`lock_out_failures`] counts only the `401`s
//! a client earns, so that guessing a password is slowed and ordinary use is not.
//!
//! ```no_run
//! use std::num::NonZeroU32;
//!
//! use axa_ratelimit::{Limiter, by_client, lock_out_failures};
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
//! to come from the proxy, and all clients share one limit.
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
//! # What is kept
//!
//! A key is held only as a keyed hash, and the first request after two windows drops
//! it. An address is personal data, so the limiter neither stores nor logs one.
//!
//! # Limits
//!
//! Each process counts for itself, so with several replicas a client gets one limit
//! from each. A client can send up to twice the limit across the end of a window.
//! A limit protects a service from a noisy client. It does not stop a distributed attack.

use std::{
    hash::{BuildHasher, Hash, RandomState},
    net::{IpAddr, Ipv6Addr},
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axa_problem::Problem;
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

const LIMIT: HeaderName = HeaderName::from_static("ratelimit-limit");
const REMAINING: HeaderName = HeaderName::from_static("ratelimit-remaining");
const RESET: HeaderName = HeaderName::from_static("ratelimit-reset");

/// One key's count in its window.
#[derive(Debug, Clone, Copy)]
struct Slot {
    start: Instant,
    count: u32,
}

impl Slot {
    const fn new(start: Instant) -> Self {
        Self { start, count: 0 }
    }
}

/// The counts. A sweep is at most once a window, so that a table full of live keys
/// does not make every new key scan it.
#[derive(Debug)]
struct Table {
    slots: std::collections::HashMap<u64, Slot>,
    /// Shared by new keys once `slots` is full.
    overflow: Slot,
    next_sweep: Instant,
}

/// A limit on requests per key in a fixed window.
///
/// Clones share their counts.
#[derive(Debug, Clone)]
pub struct Limiter {
    limit: u32,
    window: Duration,
    max_keys: usize,
    hasher: RandomState,
    table: Arc<Mutex<Table>>,
}

/// What a limiter says about one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Whether the request may go ahead.
    pub allowed: bool,
    /// The limit.
    pub limit: u32,
    /// Requests left in this window.
    pub remaining: u32,
    /// Whole seconds until this window ends, rounded up, so never too few to wait.
    pub reset_seconds: u64,
}

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
            hasher: RandomState::new(),
            table: Arc::new(Mutex::new(Table {
                slots: std::collections::HashMap::new(),
                overflow: Slot::new(now),
                next_sweep: now + window,
            })),
        }
    }

    /// Count one request by `key`.
    pub fn check(&self, key: impl Hash) -> Verdict {
        self.count(self.hasher.hash_one(key), true, Instant::now())
    }

    /// What [`Self::check`] would say about `key`, counting nothing.
    pub fn peek(&self, key: impl Hash) -> Verdict {
        self.count(self.hasher.hash_one(key), false, Instant::now())
    }

    fn count(&self, key: u64, charge: bool, now: Instant) -> Verdict {
        let window = self.window;
        // A poisoned lock holds counts, which are safe to go on using.
        let mut table = self
            .table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Table {
            slots,
            overflow,
            next_sweep,
        } = &mut *table;
        if now >= *next_sweep {
            slots.retain(|_, slot| now.saturating_duration_since(slot.start) < window);
            *next_sweep = now + window;
        }
        let known = slots.contains_key(&key);
        let slot = if !known && slots.len() >= self.max_keys {
            overflow
        } else if !known && !charge {
            return self.verdict(Slot::new(now), true, now);
        } else {
            slots.entry(key).or_insert_with(|| Slot::new(now))
        };
        if now.saturating_duration_since(slot.start) >= window {
            *slot = Slot::new(now);
        }
        let allowed = slot.count < self.limit;
        if allowed && charge {
            slot.count += 1;
        }
        self.verdict(*slot, allowed, now)
    }

    fn verdict(&self, slot: Slot, allowed: bool, now: Instant) -> Verdict {
        let left = (slot.start + self.window).saturating_duration_since(now);
        Verdict {
            allowed,
            limit: self.limit,
            remaining: self.limit - slot.count,
            reset_seconds: left.as_secs() + u64::from(left.subsec_nanos() > 0),
        }
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        self.table.lock().unwrap().slots.len()
    }
}

impl Verdict {
    /// Add `RateLimit-Limit`, `RateLimit-Remaining`, and `RateLimit-Reset`.
    pub fn stamp(&self, response: &mut Response) {
        let headers = response.headers_mut();
        headers.insert(LIMIT, HeaderValue::from(self.limit));
        headers.insert(REMAINING, HeaderValue::from(self.remaining));
        headers.insert(RESET, HeaderValue::from(self.reset_seconds));
    }

    /// The `429`, with `Retry-After` and the `RateLimit-*` headers.
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

/// Limit each client. Every response gets the `RateLimit-*` headers, apart from the
/// probes in [`EXEMPT`], which are not counted.
pub async fn by_client(State(limiter): State<Limiter>, request: Request, next: Next) -> Response {
    if EXEMPT.contains(&request.uri().path()) {
        return next.run(request).await;
    }
    let verdict = limiter.check(client_key(request.extensions()));
    if !verdict.allowed {
        return verdict.refusal();
    }
    let mut response = next.run(request).await;
    verdict.stamp(&mut response);
    response
}

/// Refuse a client that has earned the limit in `401`s, until the window ends. Only a
/// `401` counts, so a client that signs in correctly is never slowed.
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
    let key = client_key(request.extensions());
    let verdict = limiter.peek(key);
    if !verdict.allowed {
        return verdict.refusal();
    }
    let response = next.run(request).await;
    if response.status() == StatusCode::UNAUTHORIZED {
        limiter.check(key);
    }
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
            axa_problem::PROBLEM_JSON
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
}
