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

/// A TCP proxy to the real Redis that a test can cut or stall.
struct Proxy {
    port: u16,
    stalled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    accepting: tokio::task::JoinHandle<()>,
    links: std::sync::Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl Proxy {
    async fn start() -> Self {
        use std::sync::{Arc, Mutex, atomic::AtomicBool};
        let url = std::env::var("AXA_TEST_REDIS_URL").expect("AXA_TEST_REDIS_URL");
        let target = url
            .trim_start_matches("redis://")
            .trim_end_matches('/')
            .to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stalled = Arc::new(AtomicBool::new(false));
        let links = Arc::new(Mutex::new(Vec::new()));
        let (flag, held) = (stalled.clone(), links.clone());
        let accepting = tokio::spawn(async move {
            loop {
                let Ok((mut client, _)) = listener.accept().await else {
                    return;
                };
                let target = target.clone();
                let flag = flag.clone();
                let link = tokio::spawn(async move {
                    let mut server = tokio::net::TcpStream::connect(&target).await.unwrap();
                    let (mut from_client, mut to_client) = client.split();
                    let (mut from_server, mut to_server) = server.split();
                    let up = forward(&mut from_client, &mut to_server, &flag);
                    let down = forward(&mut from_server, &mut to_client, &flag);
                    let _ = tokio::join!(up, down);
                });
                held.lock().unwrap().push(link);
            }
        });
        Self {
            port,
            stalled,
            accepting,
            links,
        }
    }

    fn url(&self) -> String {
        format!("redis://127.0.0.1:{}", self.port)
    }

    /// Stop answering, with the connections left open.
    fn stall(&self) {
        self.stalled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Close everything and refuse new connections.
    fn cut(&self) {
        self.accepting.abort();
        for link in self.links.lock().unwrap().iter() {
            link.abort();
        }
    }
}

async fn forward<R, W>(
    from: &mut R,
    to: &mut W,
    stalled: &std::sync::atomic::AtomicBool,
) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buffer = [0_u8; 4096];
    loop {
        let read = from.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        while stalled.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        to.write_all(&buffer[..read]).await?;
    }
}

/// How long an outage may cost one request: one time limit, and some slack.
const OUTAGE_LIMIT: Duration = Duration::from_secs(2);

async fn loads_in_time(cache: &Cache) -> u32 {
    let load = cache.get_or_load("outage", || async { Ok::<_, std::io::Error>(7) });
    tokio::time::timeout(OUTAGE_LIMIT, load)
        .await
        .expect("the cache held the request up")
        .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn an_outage_does_not_fail_get_or_load() {
    let proxy = Proxy::start().await;
    let config = CacheConfig::new(proxy.url(), prefix("outage"), MINUTE);
    let cache = Cache::connect(&config).await.unwrap();
    cache.ping().await.unwrap();
    proxy.cut();
    assert_eq!(loads_in_time(&cache).await, 7);
    let read = cache.get::<u32>("outage").await;
    assert!(read.is_err(), "{read:?}");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn every_call_is_bounded_while_the_client_reconnects() {
    let proxy = Proxy::start().await;
    let config = CacheConfig::new(proxy.url(), prefix("reconnect"), MINUTE);
    let cache = Cache::connect(&config).await.unwrap();
    cache.ping().await.unwrap();
    proxy.cut();
    // The first call finds the connection gone. Later ones meet the client's
    // retries, which run for seconds without a limit of ours.
    for _ in 0..4 {
        let write = tokio::time::timeout(OUTAGE_LIMIT, cache.set("a", &1_u32)).await;
        assert!(write.expect("a call waited out the retries").is_err());
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_REDIS_URL"]
async fn a_frozen_redis_does_not_hold_a_request_up() {
    let proxy = Proxy::start().await;
    let config = CacheConfig::new(proxy.url(), prefix("frozen"), MINUTE);
    let cache = Cache::connect(&config).await.unwrap();
    cache.ping().await.unwrap();
    proxy.stall();
    assert_eq!(loads_in_time(&cache).await, 7);
    // The client's own response timeout may answer first, so either error will do.
    let read = cache.get::<u32>("outage").await;
    assert!(read.is_err(), "{read:?}");
}
