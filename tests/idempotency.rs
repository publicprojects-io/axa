//! `Idempotency-Key` behind the standard middleware stack, as a service mounts it. These
//! check where the layer sits: inside compression, inside the guards, and inside what
//! cuts a request off.
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test --features idempotency --test idempotency -- --ignored --test-threads=1
//! ```

#![cfg(feature = "idempotency")]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axa::{
    config::Http,
    idempotency::{HEADER, Idempotency, REPLAYED, idempotent, setup},
    middleware,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{StatusCode, header},
    middleware::{Next, from_fn, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

fn http() -> Http {
    Http {
        body_limit: 1024,
        compression: true,
        timeout_seconds: 0,
        timeout_overrides: Vec::new(),
        cross_site_paths: Vec::new(),
        cors_origins: Vec::new(),
        hsts: false,
        hsts_max_age: 0,
        csp: None,
        permissions_policy: None,
        trusted_proxies: Vec::new(),
    }
}

async fn pool() -> PgPool {
    let url = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let pool = PgPool::connect(&url).await.expect("connect");
    setup(&pool).await.expect("setup");
    sqlx::query("TRUNCATE axa_idempotency")
        .execute(&pool)
        .await
        .expect("truncate");
    pool
}

/// The caller, as a guard would leave it.
#[derive(Clone)]
struct Caller(String);

fn keys(pool: &PgPool) -> Idempotency {
    Idempotency::new(pool.clone(), |request| {
        request
            .extensions()
            .get::<Caller>()
            .map(|caller| caller.0.clone())
    })
}

/// A guard: `x-caller` names the caller, and `refused` is sent away.
async fn guard(mut request: Request, next: Next) -> Response {
    let caller = request
        .headers()
        .get("x-caller")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    match caller.as_deref() {
        None | Some("refused") => StatusCode::UNAUTHORIZED.into_response(),
        Some(caller) => {
            let caller = Caller(caller.to_string());
            request.extensions_mut().insert(caller);
            next.run(request).await
        }
    }
}

/// An answer long enough to compress.
fn large_answer() -> Value {
    json!({ "note": "this answer repeats itself so that compression has work to do ".repeat(20) })
}

fn routes(runs: &Arc<AtomicUsize>) -> Router {
    let runs = runs.clone();
    Router::new().route(
        "/things",
        post(move |_: String| async move {
            runs.fetch_add(1, Ordering::SeqCst);
            (StatusCode::CREATED, axum::Json(large_answer()))
        }),
    )
}

/// The order a service uses: routes, then its layers with the guard outside this one,
/// then the standard stack.
fn app(pool: &PgPool, runs: &Arc<AtomicUsize>, http: &Http) -> Router {
    let routes = routes(runs)
        .layer(from_fn_with_state(keys(pool), idempotent))
        .layer(from_fn(guard));
    middleware::apply(routes, http).unwrap()
}

fn write(caller: &str, encoding: Option<&str>) -> Request {
    let mut request = Request::post("/things")
        .header("x-caller", caller)
        .header(HEADER, "k");
    if let Some(encoding) = encoding {
        request = request.header(header::ACCEPT_ENCODING, encoding);
    }
    request.body(Body::from("{}")).unwrap()
}

async fn send(app: &Router, request: Request) -> Response {
    app.clone().oneshot(request).await.unwrap()
}

async fn bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn stored_body(pool: &PgPool) -> Option<Vec<u8>> {
    sqlx::query_scalar("SELECT body FROM axa_idempotency")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn inside_compression_the_plain_body_is_stored_and_each_caller_gets_what_it_asked_for() {
    let pool = pool().await;
    let runs = Arc::new(AtomicUsize::new(0));
    let app = app(&pool, &runs, &http());

    let first = send(&app, write("ana", Some("gzip"))).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    assert_eq!(first.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(bytes(first).await[..2], GZIP_MAGIC);

    let stored = stored_body(&pool).await.expect("an answer is stored");
    let stored: Value = serde_json::from_slice(&stored).expect("the stored body is plain JSON");
    assert_eq!(stored, large_answer());

    let plain = send(&app, write("ana", None)).await;
    assert_eq!(plain.headers()[REPLAYED], "true");
    assert!(!plain.headers().contains_key(header::CONTENT_ENCODING));
    let plain: Value = serde_json::from_slice(&bytes(plain).await).expect("a plain replay");
    assert_eq!(plain, large_answer());

    let gzip = send(&app, write("ana", Some("gzip"))).await;
    assert_eq!(gzip.headers()[REPLAYED], "true");
    assert_eq!(gzip.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(bytes(gzip).await[..2], GZIP_MAGIC);

    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// The defect one service shipped: mounted outside the stack, the layer read gzip, and
/// replayed it to a caller that never asked for gzip, with no header to say what it was.
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn outside_compression_a_compressed_answer_is_never_stored_or_replayed() {
    let pool = pool().await;
    let runs = Arc::new(AtomicUsize::new(0));
    let inner = routes(&runs).layer(from_fn(guard));
    let stack = middleware::apply(inner, &http()).unwrap();
    let keys = Idempotency::new(pool.clone(), |request| {
        let caller = request.headers().get("x-caller")?;
        Some(caller.to_str().ok()?.to_string())
    });
    let misplaced = stack.layer(from_fn_with_state(keys, idempotent));

    let first = send(&misplaced, write("ana", Some("gzip"))).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    assert_eq!(first.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(bytes(first).await[..2], GZIP_MAGIC);
    assert_eq!(stored_body(&pool).await, None);

    let retry = send(&misplaced, write("ana", None)).await;
    assert_eq!(retry.status(), StatusCode::CONFLICT);
    let body: Value = serde_json::from_slice(&bytes(retry).await).expect("a plain problem");
    assert_eq!(body["code"], "idempotency_answer_not_kept");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_refusal_gets_the_standard_headers() {
    let pool = pool().await;
    let runs = Arc::new(AtomicUsize::new(0));
    let app = app(&pool, &runs, &http());
    let request = Request::post("/things")
        .header("x-caller", "ana")
        .header(HEADER, "not a key")
        .body(Body::empty())
        .unwrap();

    let refused = send(&app, request).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(refused.headers().contains_key(&middleware::X_REQUEST_ID));
    assert_eq!(refused.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_guard_outside_the_layer_refuses_before_a_replay_can_answer() {
    let pool = pool().await;
    let runs = Arc::new(AtomicUsize::new(0));
    let app = app(&pool, &runs, &http());

    send(&app, write("ana", None)).await;
    let replay = send(&app, write("ana", None)).await;
    assert_eq!(replay.headers()[REPLAYED], "true");

    let refused = send(&app, write("refused", None)).await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert!(!refused.headers().contains_key(REPLAYED));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn the_service_body_limit_still_applies_and_its_refusal_frees_the_key() {
    let pool = pool().await;
    let runs = Arc::new(AtomicUsize::new(0));
    let small = Http {
        body_limit: 4,
        ..http()
    };
    let app = app(&pool, &runs, &small);

    let refused = send(&app, write_body("too long for the limit")).await;
    assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM axa_idempotency")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(held, 0);
}

fn write_body(body: &'static str) -> Request {
    Request::post("/things")
        .header("x-caller", "ana")
        .header(HEADER, "k")
        .body(Body::from(body))
        .unwrap()
}

/// Routes whose first run never answers: it panics, or waits to be cut off.
fn routes_ending_badly(keys: Idempotency, http: &Http, panics: bool) -> Router {
    let runs = Arc::new(AtomicUsize::new(0));
    let routes = Router::new().route(
        "/things",
        post(move || async move {
            if runs.fetch_add(1, Ordering::SeqCst) == 0 {
                assert!(!panics, "the first run panics");
                std::future::pending::<()>().await;
            }
            "ran"
        }),
    );
    let routes = routes
        .layer(from_fn_with_state(keys, idempotent))
        .layer(from_fn(guard));
    middleware::apply(routes, http).unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_panic_is_answered_and_its_key_comes_free_after_the_lease() {
    let pool = pool().await;
    let lease = Duration::from_millis(200);
    let app = routes_ending_badly(keys(&pool).rerun_abandoned_after(lease), &http(), true);

    let panicked = send(&app, write("ana", None)).await;
    assert_eq!(panicked.status(), StatusCode::INTERNAL_SERVER_ERROR);

    tokio::time::sleep(lease * 2).await;
    let retry = send(&app, write("ana", None)).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(bytes(retry).await, b"ran");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn by_default_a_panic_holds_its_key_and_a_retry_is_told_to_wait() {
    let pool = pool().await;
    let app = routes_ending_badly(keys(&pool), &http(), true);

    let panicked = send(&app, write("ana", None)).await;
    assert_eq!(panicked.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // The run may have done its work, so the key is held for its ttl, not run again.
    let retry = send(&app, write("ana", None)).await;
    assert_eq!(retry.status(), StatusCode::CONFLICT);
    let problem: Value = serde_json::from_slice(&bytes(retry).await).unwrap();
    assert_eq!(problem["code"], "idempotency_in_progress");
    let held: bool = sqlx::query_scalar(
        "SELECT lease_until >= now() + interval '23 hours' FROM axa_idempotency",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(held, "the claim does not last the key's ttl");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_request_the_stack_cuts_off_frees_its_key_after_the_lease() {
    let pool = pool().await;
    let lease = Duration::from_millis(200);
    let timed = Http {
        timeout_seconds: 1,
        ..http()
    };
    let app = routes_ending_badly(keys(&pool).rerun_abandoned_after(lease), &timed, false);

    let cut_off = send(&app, write("ana", None)).await;
    assert_eq!(cut_off.status(), StatusCode::REQUEST_TIMEOUT);

    tokio::time::sleep(lease * 2).await;
    let retry = send(&app, write("ana", None)).await;
    assert_eq!(retry.status(), StatusCode::OK);
}

/// A service on `SeaORM` reaches the store through the pool under its connection.
#[cfg(feature = "postgres-seaorm")]
#[test]
fn a_sea_orm_connection_gives_a_pool_this_takes() {
    fn pool(connection: &sea_orm::DatabaseConnection) -> PgPool {
        connection.get_postgres_connection_pool().clone()
    }
    let _ = pool;
}
