//! Limits shared across replicas. The Postgres and Redis tests are ignored by default:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... AXA_TEST_REDIS_URL=redis://localhost:6379 \
//!   cargo test -p axa-request-rate-limit --all-features -- --ignored --test-threads=1
//! ```
#![cfg(all(feature = "postgres", feature = "redis"))]

use std::{net::IpAddr, num::NonZeroU32, time::Duration};

use axa_request_rate_limit::shared::{
    SetupError, SharedLimiter, Store, WhenDown, by_client_shared,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::RETRY_AFTER},
    middleware::from_fn_with_state,
    routing::get,
};
use tower::ServiceExt;

const SECRET: [u8; 32] = [7; 32];

fn n(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap()
}

async fn postgres() -> Store {
    let url = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DROP TABLE IF EXISTS axa_rate_limits")
        .execute(&pool)
        .await
        .unwrap();
    Store::postgres(pool).await.unwrap()
}

async fn redis_store() -> Store {
    let url = std::env::var("AXA_TEST_REDIS_URL").expect("AXA_TEST_REDIS_URL");
    let client = redis::Client::open(url).unwrap();
    let mut connection = redis::aio::ConnectionManager::new(client).await.unwrap();
    let _: () = redis::cmd("FLUSHDB")
        .query_async(&mut connection)
        .await
        .unwrap();
    Store::redis(connection)
}

/// Two replicas, three requests a minute, sharing one store.
async fn replicas_share_one_limit(store: Store) {
    let one =
        SharedLimiter::new(store.clone(), "api", n(3), n(60), &SECRET, WhenDown::Refuse).unwrap();
    let two = SharedLimiter::new(store, "api", n(3), n(60), &SECRET, WhenDown::Refuse).unwrap();
    let client = "203.0.113.7".parse::<IpAddr>().unwrap();
    for (replica, remaining) in [(&one, 2), (&two, 1), (&one, 0)] {
        let verdict = replica.check(Some(client)).await.unwrap();
        assert!(verdict.allowed);
        assert_eq!(verdict.remaining, remaining);
        assert!(verdict.reset_seconds >= 1 && verdict.reset_seconds <= 60);
    }
    let refused = two.check(Some(client)).await.unwrap();
    assert!(!refused.allowed && refused.first_refusal);
    let again = one.check(Some(client)).await.unwrap();
    assert!(!again.allowed && !again.first_refusal);
    let other = "203.0.113.8".parse::<IpAddr>().unwrap();
    assert!(
        one.check(Some(other)).await.unwrap().allowed,
        "another client"
    );
}

async fn a_window_ends(store: Store) {
    let limiter =
        SharedLimiter::new(store, "short", n(1), n(1), &SECRET, WhenDown::Refuse).unwrap();
    assert!(limiter.check("k").await.unwrap().allowed);
    let mut refused = false;
    for _ in 0..5 {
        if !limiter.check("k").await.unwrap().allowed {
            refused = true;
            break;
        }
    }
    assert!(refused, "the second in a window is refused");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(limiter.check("k").await.unwrap().allowed, "a new window");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn postgres_replicas_share_one_limit_and_store_no_address() {
    let store = postgres().await;
    replicas_share_one_limit(store.clone()).await;
    let Store::Postgres(pool) = &store else {
        unreachable!()
    };
    let keys: Vec<String> = sqlx::query_scalar("SELECT key FROM axa_rate_limits")
        .fetch_all(pool)
        .await
        .unwrap();
    assert_eq!(keys.len(), 2);
    assert!(
        keys.iter()
            .all(|key| key.starts_with("api:") && !key.contains("203.0.113")),
        "{keys:?}"
    );
    a_window_ends(store.clone()).await;
    assert!(
        store.purge(n(1)).await.unwrap() >= 1,
        "ended windows are purged"
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn redis_replicas_share_one_limit_and_a_window_ends() {
    replicas_share_one_limit(redis_store().await).await;
    a_window_ends(redis_store().await).await;
}

fn unreachable_store() -> Store {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://axa:axa@127.0.0.1:1/axa")
        .unwrap();
    Store::Postgres(pool)
}

async fn status_when_down(when_down: WhenDown) -> (StatusCode, Option<String>, String) {
    let limiter =
        SharedLimiter::new(unreachable_store(), "api", n(3), n(60), &SECRET, when_down).unwrap();
    let app = Router::new()
        .route("/", get(|| async { "ok" }))
        .layer(from_fn_with_state(limiter, by_client_shared));
    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let retry = response
        .headers()
        .get(RETRY_AFTER)
        .map(|value| value.to_str().unwrap().to_string());
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    (status, retry, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn the_service_chooses_what_a_store_outage_does() {
    let (status, _, body) = status_when_down(WhenDown::Allow).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "ok"));
    let (status, retry, body) = status_when_down(WhenDown::Refuse).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(retry.as_deref(), Some("5"));
    assert!(body.contains("rate_limit_unavailable"), "{body}");
}

#[tokio::test]
async fn a_short_secret_or_a_bad_name_is_refused() {
    let short = SharedLimiter::new(
        unreachable_store(),
        "api",
        n(1),
        n(1),
        &[1; 31],
        WhenDown::Allow,
    );
    assert_eq!(short.unwrap_err(), SetupError::ShortSecret);
    for name in ["", "a b", "a:b", &"x".repeat(65)] {
        let bad = SharedLimiter::new(
            unreachable_store(),
            name,
            n(1),
            n(1),
            &SECRET,
            WhenDown::Allow,
        );
        assert_eq!(bad.unwrap_err(), SetupError::BadName, "{name}");
    }
}
