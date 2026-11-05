//! Behaviour against a real Redis. Ignored by default. Run them with:
//!
//! ```sh
//! AXA_TEST_REDIS_URL=redis://localhost:6379 cargo test -p axa-redis -- --ignored
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axa_redis::{Cache, CacheConfig, Error};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Item {
    id: u32,
    name: String,
}

fn item() -> Item {
    Item {
        id: 1,
        name: "one".to_string(),
    }
}

/// A prefix unique to this run, so repeated runs cannot see each other.
fn prefix(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("axa-test:{name}:{nanos}")
}

async fn cache(name: &str, ttl: Duration) -> Cache {
    let url = std::env::var("AXA_TEST_REDIS_URL").expect("AXA_TEST_REDIS_URL");
    Cache::connect(&CacheConfig::new(url, prefix(name), ttl))
        .await
        .expect("connect")
}

const MINUTE: Duration = Duration::from_mins(1);

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn values_round_trip() {
    let cache = cache("round-trip", MINUTE).await;
    cache.ping().await.unwrap();
    assert_eq!(cache.get::<Item>("a").await.unwrap(), None);
    assert!(!cache.exists("a").await.unwrap());

    cache.set("a", &item()).await.unwrap();
    assert_eq!(cache.get("a").await.unwrap(), Some(item()));
    assert!(cache.exists("a").await.unwrap());

    assert!(cache.delete("a").await.unwrap());
    assert!(!cache.delete("a").await.unwrap());
    assert_eq!(cache.get::<Item>("a").await.unwrap(), None);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn entries_expire() {
    let cache = cache("expire", MINUTE).await;
    cache
        .set_with_ttl("a", &item(), Duration::from_millis(50))
        .await
        .unwrap();
    assert!(cache.exists("a").await.unwrap());
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(cache.get::<Item>("a").await.unwrap(), None);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn the_default_ttl_applies() {
    let cache = cache("default-ttl", Duration::from_millis(50)).await;
    cache.set("a", &item()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!cache.exists("a").await.unwrap());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn a_zero_ttl_stores_nothing() {
    let cache = cache("zero-ttl", MINUTE).await;
    let result = cache.set_with_ttl("a", &item(), Duration::ZERO).await;
    assert!(matches!(result, Err(Error::InvalidTtl)));
    assert!(!cache.exists("a").await.unwrap());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn prefixes_keep_services_apart() {
    let one = cache("one", MINUTE).await;
    let two = cache("two", MINUTE).await;
    one.set("a", &item()).await.unwrap();
    assert_eq!(two.get::<Item>("a").await.unwrap(), None);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn a_changed_type_reads_as_a_miss() {
    let cache = cache("changed-type", MINUTE).await;
    cache.set("a", &"just text").await.unwrap();
    assert_eq!(cache.get::<Item>("a").await.unwrap(), None);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn get_or_load_loads_once() {
    let cache = cache("load-once", MINUTE).await;
    let loaded: Result<Item, Error> = cache.get_or_load("a", || async { Ok(item()) }).await;
    assert_eq!(loaded.unwrap(), item());
    let cached: Result<Item, Error> = cache
        .get_or_load("a", || async { panic!("the loader must not run on a hit") })
        .await;
    assert_eq!(cached.unwrap(), item());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn a_failed_load_stores_nothing() {
    let cache = cache("failed-load", MINUTE).await;
    let result: Result<Item, String> = cache
        .get_or_load("a", || async { Err("source down".to_string()) })
        .await;
    assert_eq!(result.unwrap_err(), "source down");
    assert!(!cache.exists("a").await.unwrap());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn a_bad_url_is_an_error() {
    let config = CacheConfig::new("not a url", "x", MINUTE);
    assert!(matches!(
        Cache::connect(&config).await,
        Err(Error::Redis(_))
    ));
}
