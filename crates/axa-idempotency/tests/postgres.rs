//! The layer against a real Postgres. Each test starts from an empty table, so run
//! these with `--test-threads=1`.
//!
//! ```sh
//! AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-idempotency -- --ignored --test-threads=1
//! ```

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axa_idempotency::{HEADER, Idempotency, REPLAYED, idempotent, setup};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode, header},
    middleware::from_fn_with_state,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::Notify;
use tower::ServiceExt;

fn url() -> String {
    std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL")
}

async fn connect() -> PgPool {
    PgPoolOptions::new()
        .max_connections(8)
        .connect(&url())
        .await
        .expect("connect")
}

/// A pool on an empty table.
async fn pool() -> PgPool {
    let pool = connect().await;
    setup(&pool).await.expect("setup");
    sqlx::query("TRUNCATE axa_idempotency")
        .execute(&pool)
        .await
        .expect("truncate");
    pool
}

/// Who a request is from, read as a service reads what authentication left. The test
/// names the caller in a header, and `nobody` means there is none.
fn caller(request: &Request) -> Option<String> {
    let name = request.headers().get("x-caller")?.to_str().ok()?;
    (name != "nobody").then(|| name.to_string())
}

fn keys(pool: &PgPool) -> Idempotency {
    Idempotency::new(pool.clone(), caller)
}

/// How many times the handlers ran.
#[derive(Clone, Default)]
struct Runs(Arc<AtomicUsize>);

impl Runs {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    fn add(&self) -> usize {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// Routes that count their runs and say which run answered.
fn routes(runs: &Runs) -> Router {
    let (created, other, failed, teapot) = (runs.clone(), runs.clone(), runs.clone(), runs.clone());
    let (read, huge) = (runs.clone(), runs.clone());
    let stream = runs.clone();
    Router::new()
        .route(
            "/things",
            post(move |body: String| async move {
                let run = created.add();
                (
                    StatusCode::CREATED,
                    [
                        (header::LOCATION, format!("/things/{run}")),
                        (header::SET_COOKIE, "session=first".to_string()),
                    ],
                    axum::Json(json!({ "run": run, "body": body })),
                )
            })
            .get(move || async move {
                read.add();
                "read"
            }),
        )
        .route(
            "/other",
            post(move || async move { axum::Json(json!({ "run": other.add() })) }),
        )
        .route(
            "/failing",
            post(move |body: String| async move {
                let run = failed.add();
                match body.as_str() {
                    "server" if run == 1 => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    "client" if run == 1 => StatusCode::BAD_REQUEST.into_response(),
                    _ => axum::Json(json!({ "run": run })).into_response(),
                }
            }),
        )
        .route(
            "/huge",
            post(move || async move {
                huge.add();
                "x".repeat(2048)
            }),
        )
        .route(
            "/stream",
            post(move || async move {
                stream.add();
                Body::from_stream(Body::from("streamed").into_data_stream())
            }),
        )
        .route(
            "/encoded",
            post(move || async move {
                teapot.add();
                ([(header::CONTENT_ENCODING, "gzip")], "not really gzip")
            }),
        )
}

fn app(keys: Idempotency, runs: &Runs) -> Router {
    routes(runs).layer(from_fn_with_state(keys, idempotent))
}

fn write(path: &str, who: &str, key: Option<&str>, body: &'static str) -> Request {
    let mut request = Request::post(path).header("x-caller", who);
    if let Some(key) = key {
        request = request.header(HEADER, key);
    }
    request.body(Body::from(body)).unwrap()
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

async fn json_of(response: Response) -> Value {
    serde_json::from_slice(&bytes(response).await).unwrap()
}

/// That a response is the problem named, with its media type.
async fn assert_problem(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    assert_eq!(json_of(response).await["code"], code);
}

async fn rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM axa_idempotency")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_retry_gets_the_first_answer_and_runs_nothing() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    let first = send(&app, write("/things", "ana", Some("k1"), "hello")).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    assert!(!first.headers().contains_key(REPLAYED));
    let location = first.headers()[header::LOCATION].clone();
    let body = bytes(first).await;

    let second = send(&app, write("/things", "ana", Some("k1"), "hello")).await;
    assert_eq!(second.status(), StatusCode::CREATED);
    assert_eq!(second.headers()[REPLAYED], "true");
    assert_eq!(second.headers()[header::LOCATION], location);
    assert_eq!(second.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(bytes(second).await, body);
    assert_eq!(runs.count(), 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_replay_hands_out_no_cookie() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    let first = send(&app, write("/things", "ana", Some("k"), "x")).await;
    assert!(first.headers().contains_key(header::SET_COOKIE));
    let replay = send(&app, write("/things", "ana", Some("k"), "x")).await;
    assert!(!replay.headers().contains_key(header::SET_COOKIE));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn with_no_key_nothing_is_kept_and_every_request_runs() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    for expected in 1..=2 {
        let response = send(&app, write("/things", "ana", None, "x")).await;
        assert!(!response.headers().contains_key(REPLAYED));
        assert_eq!(runs.count(), expected);
    }
    assert_eq!(rows(&pool).await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_method_that_changes_nothing_ignores_its_key() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    for expected in 1..=2 {
        let request = Request::get("/things")
            .header(HEADER, "k")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&app, request).await.status(), StatusCode::OK);
        assert_eq!(runs.count(), expected);
    }
    assert_eq!(rows(&pool).await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_same_key_on_a_different_request_is_refused() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    send(&app, write("/things", "ana", Some("k"), "first")).await;
    assert_eq!(runs.count(), 1);

    for different in [
        write("/things", "ana", Some("k"), "second"),
        write("/other", "ana", Some("k"), "first"),
        write("/things?page=2", "ana", Some("k"), "first"),
        Request::patch("/things")
            .header("x-caller", "ana")
            .header(HEADER, "k")
            .body(Body::from("first"))
            .unwrap(),
    ] {
        let refused = send(&app, different).await;
        assert_problem(
            refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "idempotency_key_reused",
        )
        .await;
    }
    assert_eq!(runs.count(), 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn one_key_is_two_keys_for_two_callers() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    let ana = json_of(send(&app, write("/things", "ana", Some("shared"), "x")).await).await;
    let bo = send(&app, write("/things", "bo", Some("shared"), "x")).await;
    assert!(!bo.headers().contains_key(REPLAYED), "bo read ana's answer");
    let bo = json_of(bo).await;
    assert_ne!(ana["run"], bo["run"]);
    assert_eq!(runs.count(), 2);

    let ana_again = send(&app, write("/things", "ana", Some("shared"), "x")).await;
    assert_eq!(json_of(ana_again).await["run"], ana["run"]);
    let bo_again = send(&app, write("/things", "bo", Some("shared"), "x")).await;
    assert_eq!(json_of(bo_again).await["run"], bo["run"]);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn with_no_caller_a_keyed_request_is_refused_and_does_not_run() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    let refused = send(&app, write("/things", "nobody", Some("k"), "x")).await;
    assert_problem(refused, StatusCode::UNAUTHORIZED, "idempotency_no_caller").await;
    assert_eq!(runs.count(), 0);
    assert_eq!(rows(&pool).await, 0);

    let unnamed = Idempotency::new(pool.clone(), |_: &Request| Some(String::new()));
    let unnamed = self::app(unnamed, &runs);
    let refused = send(&unnamed, write("/things", "ana", Some("k"), "x")).await;
    assert_problem(refused, StatusCode::UNAUTHORIZED, "idempotency_no_caller").await;
    assert_eq!(runs.count(), 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_duplicate_while_the_first_runs_is_told_to_wait() {
    let pool = pool().await;
    let (started, finish) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let runs = Runs::default();
    let routes = {
        let (started, finish, runs) = (started.clone(), finish.clone(), runs.clone());
        Router::new().route(
            "/slow",
            post(move || async move {
                let run = runs.add();
                started.notify_one();
                finish.notified().await;
                axum::Json(json!({ "run": run }))
            }),
        )
    };
    let app = routes.layer(from_fn_with_state(keys(&pool), idempotent));

    let first = tokio::spawn({
        let app = app.clone();
        async move { send(&app, write("/slow", "ana", Some("k"), "x")).await }
    });
    started.notified().await;

    let waiting = send(&app, write("/slow", "ana", Some("k"), "x")).await;
    assert_eq!(waiting.headers()[header::RETRY_AFTER], "1");
    assert_problem(waiting, StatusCode::CONFLICT, "idempotency_in_progress").await;
    let different = send(&app, write("/slow", "ana", Some("k"), "y")).await;
    assert_eq!(different.status(), StatusCode::UNPROCESSABLE_ENTITY);

    finish.notify_one();
    let first = first.await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let replay = send(&app, write("/slow", "ana", Some("k"), "x")).await;
    assert_eq!(replay.headers()[REPLAYED], "true");
    assert_eq!(runs.count(), 1);
}

/// Replicas share nothing but the database, so each request here has its own pool.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn of_many_requests_at_once_on_two_replicas_exactly_one_runs() {
    const REQUESTS: usize = 40;
    let pool = pool().await;
    sqlx::query("CREATE TABLE IF NOT EXISTS axa_idempotency_effects (id BIGSERIAL PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE axa_idempotency_effects")
        .execute(&pool)
        .await
        .unwrap();

    let handler = {
        let pool = pool.clone();
        post(move || async move {
            sqlx::query("INSERT INTO axa_idempotency_effects DEFAULT VALUES")
                .execute(&pool)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            (StatusCode::CREATED, "made")
        })
    };
    let replica = |keys: Idempotency| {
        Router::new()
            .route("/race", handler.clone())
            .layer(from_fn_with_state(keys, idempotent))
    };
    let (one, two) = (replica(keys(&pool)), replica(keys(&connect().await)));

    let requests = (0..REQUESTS).map(|index| {
        let app = if index % 2 == 0 { &one } else { &two }.clone();
        tokio::spawn(async move {
            send(&app, write("/race", "ana", Some("raced"), "x"))
                .await
                .status()
        })
    });
    let mut statuses = Vec::new();
    for request in requests.collect::<Vec<_>>() {
        statuses.push(request.await.unwrap());
    }

    let effects: i64 = sqlx::query_scalar("SELECT count(*) FROM axa_idempotency_effects")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(effects, 1, "{statuses:?}");
    let created = statuses
        .iter()
        .filter(|status| **status == StatusCode::CREATED)
        .count();
    assert!(created >= 1, "{statuses:?}");
    assert!(
        statuses
            .iter()
            .all(|status| [StatusCode::CREATED, StatusCode::CONFLICT].contains(status)),
        "{statuses:?}"
    );
    sqlx::query("DROP TABLE axa_idempotency_effects")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_failure_releases_the_key_so_a_retry_runs() {
    let pool = pool().await;
    for body in ["server", "client"] {
        let runs = Runs::default();
        let app = app(keys(&pool), &runs);
        let failed = send(&app, write("/failing", "ana", Some(body), body)).await;
        assert!(!failed.status().is_success());
        assert_eq!(rows(&pool).await, 0, "the failure left the key held");

        let retry = send(&app, write("/failing", "ana", Some(body), body)).await;
        assert_eq!(retry.status(), StatusCode::OK);
        assert!(!retry.headers().contains_key(REPLAYED));
        assert_eq!(json_of(retry).await["run"], 2);
        sqlx::query("TRUNCATE axa_idempotency")
            .execute(&pool)
            .await
            .unwrap();
    }
}

/// A claim as a crashed process leaves it.
async fn abandoned_claim(pool: &PgPool, caller: &str, key: &str, fingerprint: &str) {
    sqlx::query(
        "INSERT INTO axa_idempotency (caller, key, fingerprint, claim, lease_until, expires_at)
         VALUES ($1, $2, $3, gen_random_uuid(), now() - interval '1 second',
                 now() + interval '1 day')",
    )
    .bind(caller)
    .bind(key)
    .bind(fingerprint)
    .execute(pool)
    .await
    .unwrap();
}

/// The fingerprint a request left, read back so that a test crashes the real request.
async fn fingerprint_of(pool: &PgPool, key: &str) -> String {
    sqlx::query_scalar("SELECT fingerprint FROM axa_idempotency WHERE key = $1")
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_claim_abandoned_by_a_crash_is_taken_over_by_the_same_request() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);
    // The hash of this request, from a run of it under another key.
    send(&app, write("/things", "ana", Some("probe"), "x")).await;
    let print = fingerprint_of(&pool, "probe").await;
    abandoned_claim(&pool, "ana", "crashed", &print).await;

    let retry = send(&app, write("/things", "ana", Some("crashed"), "x")).await;
    assert_eq!(retry.status(), StatusCode::CREATED);
    assert!(!retry.headers().contains_key(REPLAYED));
    assert_eq!(runs.count(), 2);

    let replay = send(&app, write("/things", "ana", Some("crashed"), "x")).await;
    assert_eq!(replay.headers()[REPLAYED], "true");
    assert_eq!(runs.count(), 2);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_different_request_cannot_take_over_an_abandoned_claim() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);
    abandoned_claim(&pool, "ana", "crashed", "the lost run's request").await;

    let refused = send(&app, write("/things", "ana", Some("crashed"), "x")).await;
    assert_problem(
        refused,
        StatusCode::UNPROCESSABLE_ENTITY,
        "idempotency_key_reused",
    )
    .await;
    assert_eq!(runs.count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn of_many_retries_of_an_abandoned_claim_exactly_one_takes_it() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);
    send(&app, write("/things", "ana", Some("probe"), "x")).await;
    let print = fingerprint_of(&pool, "probe").await;
    abandoned_claim(&pool, "ana", "crashed", &print).await;

    let retries = (0..20).map(|_| {
        let app = app.clone();
        tokio::spawn(async move { send(&app, write("/things", "ana", Some("crashed"), "x")).await })
    });
    for retry in retries.collect::<Vec<_>>() {
        retry.await.unwrap();
    }
    assert_eq!(runs.count(), 2, "the probe, and one take-over");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_run_that_reports_in_outlives_its_lease() {
    let pool = pool().await;
    let runs = Runs::default();
    let finish = Arc::new(Notify::new());
    let routes = {
        let (finish, runs) = (finish.clone(), runs.clone());
        Router::new().route(
            "/long",
            post(move || async move {
                // Only the first run waits, so that a take-over fails below and does not hang.
                let run = runs.add();
                if run == 1 {
                    finish.notified().await;
                }
                run.to_string()
            }),
        )
    };
    let lease = Duration::from_millis(300);
    let app = routes.layer(from_fn_with_state(keys(&pool).lease(lease), idempotent));

    let first = tokio::spawn({
        let app = app.clone();
        async move { send(&app, write("/long", "ana", Some("k"), "x")).await }
    });
    // Several leases go by while the first still runs.
    for _ in 0..4 {
        tokio::time::sleep(lease).await;
        let waiting = send(&app, write("/long", "ana", Some("k"), "x")).await;
        assert_eq!(waiting.status(), StatusCode::CONFLICT);
    }
    assert_eq!(runs.count(), 1, "a live run was taken over");
    finish.notify_waiters();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_cancelled_run_stops_reporting_in_and_its_key_comes_free() {
    let pool = pool().await;
    let runs = Runs::default();
    let routes = {
        let runs = runs.clone();
        Router::new().route(
            "/hangs",
            post(move || async move {
                if runs.add() == 1 {
                    std::future::pending::<()>().await;
                }
                "ran"
            }),
        )
    };
    let lease = Duration::from_millis(200);
    let app = routes.layer(from_fn_with_state(keys(&pool).lease(lease), idempotent));

    let hung_up = tokio::time::timeout(
        Duration::from_millis(100),
        send(&app, write("/hangs", "ana", Some("k"), "x")),
    )
    .await;
    assert!(hung_up.is_err());
    let waiting = send(&app, write("/hangs", "ana", Some("k"), "x")).await;
    assert_eq!(waiting.status(), StatusCode::CONFLICT);

    tokio::time::sleep(lease * 2).await;
    let retry = send(&app, write("/hangs", "ana", Some("k"), "x")).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(runs.count(), 2);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_run_that_lost_its_claim_cannot_overwrite_the_next_runs_answer() {
    let pool = pool().await;
    let (started, finish) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let runs = Runs::default();
    let routes = {
        let (started, finish, runs) = (started.clone(), finish.clone(), runs.clone());
        Router::new().route(
            "/slow",
            post(move || async move {
                let run = runs.add();
                if run == 1 {
                    started.notify_one();
                    finish.notified().await;
                }
                run.to_string()
            }),
        )
    };
    // A lease long enough that only this test takes the first run's claim away.
    let app = routes.layer(from_fn_with_state(
        keys(&pool).lease(Duration::from_mins(1)),
        idempotent,
    ));

    let first = tokio::spawn({
        let app = app.clone();
        async move { send(&app, write("/slow", "ana", Some("k"), "x")).await }
    });
    started.notified().await;
    sqlx::query("UPDATE axa_idempotency SET lease_until = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let second = send(&app, write("/slow", "ana", Some("k"), "x")).await;
    assert_eq!(bytes(second).await, b"2");

    finish.notify_one();
    assert_eq!(bytes(first.await.unwrap()).await, b"1");

    let replay = send(&app, write("/slow", "ana", Some("k"), "x")).await;
    assert_eq!(replay.headers()[REPLAYED], "true");
    assert_eq!(bytes(replay).await, b"2", "the lost run wrote its answer");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_expired_answer_is_not_replayed_and_the_key_runs_afresh() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool).ttl(Duration::from_millis(50)), &runs);

    send(&app, write("/things", "ana", Some("k"), "x")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Not swept: the row is there, and it is still not an answer. Nor does its old
    // request hold the key against a new one.
    assert_eq!(rows(&pool).await, 1);
    let fresh = send(&app, write("/things", "ana", Some("k"), "another")).await;
    assert_eq!(fresh.status(), StatusCode::CREATED);
    assert!(!fresh.headers().contains_key(REPLAYED));
    assert_eq!(runs.count(), 2);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_sweep_deletes_what_has_expired_and_nothing_else() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);
    send(&app, write("/things", "ana", Some("live"), "x")).await;
    send(&app, write("/things", "ana", Some("old"), "x")).await;
    sqlx::query("UPDATE axa_idempotency SET expires_at = now() WHERE key = 'old'")
        .execute(&pool)
        .await
        .unwrap();
    // An expired row whose run still reports in is that run's.
    sqlx::query(
        "INSERT INTO axa_idempotency (caller, key, fingerprint, claim, lease_until, expires_at)
         VALUES ('ana', 'running', 'x', gen_random_uuid(), now() + interval '1 minute', now())",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(keys(&pool).sweep().await.unwrap(), 1);
    let left: Vec<String> = sqlx::query_scalar("SELECT key FROM axa_idempotency ORDER BY key")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, ["live", "running"]);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_store_being_down_stops_the_request_before_it_runs() {
    let pool = connect().await;
    setup(&pool).await.unwrap();
    pool.close().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    let refused = send(&app, write("/things", "ana", Some("k"), "x")).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = bytes(refused).await;
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "idempotency_unavailable");
    assert_eq!(problem.get("detail"), None);
    assert_eq!(runs.count(), 0);

    // No key, no store, and the request is unaffected.
    let keyless = send(&app, write("/things", "ana", None, "x")).await;
    assert_eq!(keyless.status(), StatusCode::CREATED);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_store_failing_after_the_work_still_answers_the_caller() {
    let store = pool().await;
    let routes = {
        let store = store.clone();
        Router::new().route(
            "/closes",
            post(move || async move {
                store.close().await;
                (StatusCode::CREATED, "done")
            }),
        )
    };
    let app = routes.layer(from_fn_with_state(keys(&store), idempotent));

    let answered = send(&app, write("/closes", "ana", Some("k"), "x")).await;
    assert_eq!(answered.status(), StatusCode::CREATED);
    assert_eq!(bytes(answered).await, b"done");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_body_over_the_limit_is_refused_before_it_runs() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool).max_request_bytes(4), &runs);

    let refused = send(&app, write("/things", "ana", Some("k"), "12345")).await;
    assert_problem(
        refused,
        StatusCode::PAYLOAD_TOO_LARGE,
        "idempotency_body_too_large",
    )
    .await;
    assert_eq!(runs.count(), 0);
    assert_eq!(rows(&pool).await, 0);

    // With no key, this limit is not this layer's to apply.
    let keyless = send(&app, write("/things", "ana", None, "12345")).await;
    assert_eq!(keyless.status(), StatusCode::CREATED);
}

/// A write whose answer cannot be stored ran once, and must not run twice.
async fn assert_ran_once_and_is_not_repeated(path: &str, sent: &[u8]) {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool).max_response_bytes(1024), &runs);

    let first = send(&app, write(path, "ana", Some("k"), "x")).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(bytes(first).await, sent, "the caller gets the whole answer");

    let body: Option<Vec<u8>> = sqlx::query_scalar("SELECT body FROM axa_idempotency")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(body, None);

    let retry = send(&app, write(path, "ana", Some("k"), "x")).await;
    assert_problem(retry, StatusCode::CONFLICT, "idempotency_answer_not_kept").await;
    assert_eq!(runs.count(), 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_answer_over_the_limit_is_sent_and_not_kept() {
    assert_ran_once_and_is_not_repeated("/huge", "x".repeat(2048).as_bytes()).await;
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_stream_is_passed_on_and_not_kept() {
    assert_ran_once_and_is_not_repeated("/stream", b"streamed").await;
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_answer_a_handler_compressed_is_sent_and_not_kept() {
    assert_ran_once_and_is_not_repeated("/encoded", b"not really gzip").await;
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_key_that_is_not_one_clean_value_is_refused() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);

    let long = "x".repeat(256);
    for key in ["has space", "   ", long.as_str()] {
        let refused = send(&app, write("/things", "ana", Some(key), "x")).await;
        assert_problem(refused, StatusCode::BAD_REQUEST, "idempotency_key_invalid").await;
    }
    let two = Request::post("/things")
        .header("x-caller", "ana")
        .header(HEADER, "one")
        .header(HEADER, "two")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, two).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(runs.count(), 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn only_narrows_what_is_covered_and_cannot_widen_it() {
    let pool = pool().await;
    let runs = Runs::default();
    let keys = keys(&pool).only(|method, path| *method == Method::GET || path == "/other");
    let app = app(keys, &runs);

    // Left out: the key means nothing here, so a route of its own can read it.
    for expected in 1..=2 {
        send(&app, write("/things", "ana", Some("k"), "x")).await;
        assert_eq!(runs.count(), expected);
    }
    // Covered.
    send(&app, write("/other", "ana", Some("k"), "x")).await;
    send(&app, write("/other", "ana", Some("k"), "x")).await;
    assert_eq!(runs.count(), 3);
    // A read stays a read, whatever the closure says.
    let read = Request::get("/things").header(HEADER, "r");
    let read = read.body(Body::empty()).unwrap();
    send(&app, read).await;
    assert_eq!(rows(&pool).await, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn forget_lets_a_retry_run_once_a_job_has_failed() {
    let pool = pool().await;
    let runs = Runs::default();
    let keys = keys(&pool);
    let app = app(keys.clone(), &runs);

    send(&app, write("/things", "ana", Some("job"), "x")).await;
    // An answer is only ever the first one, until the service says otherwise.
    let replay = send(&app, write("/things", "ana", Some("job"), "x")).await;
    assert_eq!(replay.headers()[REPLAYED], "true");

    assert!(!keys.forget("bo", "job").await.unwrap(), "not bo's to free");
    assert!(keys.forget("ana", "job").await.unwrap());
    let retry = send(&app, write("/things", "ana", Some("job"), "x")).await;
    assert!(!retry.headers().contains_key(REPLAYED));
    assert_eq!(runs.count(), 2);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn forget_leaves_a_run_in_progress_alone() {
    let pool = pool().await;
    let keys = keys(&pool);
    abandoned_claim(&pool, "ana", "running", "print").await;
    assert!(!keys.forget("ana", "running").await.unwrap());
    assert_eq!(rows(&pool).await, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn erasing_a_caller_deletes_what_is_held_for_them_and_for_nobody_else() {
    let pool = pool().await;
    let runs = Runs::default();
    let keys = keys(&pool);
    let app = app(keys.clone(), &runs);
    send(&app, write("/things", "ana", Some("a1"), "x")).await;
    send(&app, write("/things", "ana", Some("a2"), "x")).await;
    send(&app, write("/things", "bo", Some("b1"), "x")).await;

    assert_eq!(keys.forget_caller("ana").await.unwrap(), 2);
    let left: Vec<String> = sqlx::query_scalar("SELECT caller FROM axa_idempotency")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, ["bo"]);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_row_holds_no_request_and_a_copy_of_the_answer() {
    let pool = pool().await;
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);
    let request = Request::post("/things?email=ana@example.com")
        .header("x-caller", "ana")
        .header(HEADER, "k")
        .body(Body::from("a secret in the request"))
        .unwrap();
    send(&app, request).await;

    let text: String =
        sqlx::query_scalar("SELECT (to_jsonb(t) - 'body')::text FROM axa_idempotency t")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!text.contains("example.com"), "{text}");
    assert!(!text.contains("secret"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn services_booting_together_all_set_up() {
    let pool = connect().await;
    sqlx::query("DROP TABLE IF EXISTS axa_idempotency")
        .execute(&pool)
        .await
        .unwrap();
    let boots = (0..6).map(|_| {
        let pool = pool.clone();
        tokio::spawn(async move { setup(&pool).await })
    });
    for boot in boots.collect::<Vec<_>>() {
        boot.await.unwrap().expect("setup");
    }
    keys(&pool).check().await.expect("the table is complete");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn check_finds_a_missing_table() {
    let pool = connect().await;
    setup(&pool).await.unwrap();
    keys(&pool).check().await.expect("a table that is there");

    // A connection whose search path holds no such table.
    let bare = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path TO pg_temp")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url())
        .await
        .unwrap();
    assert!(keys(&bare).check().await.is_err());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn the_setup_statements_alone_make_a_working_table() {
    let pool = connect().await;
    sqlx::query("DROP TABLE IF EXISTS axa_idempotency")
        .execute(&pool)
        .await
        .unwrap();
    for statement in axa_idempotency::SETUP_STATEMENTS {
        sqlx::query(statement).execute(&pool).await.unwrap();
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    let runs = Runs::default();
    let app = app(keys(&pool), &runs);
    send(&app, write("/things", "ana", Some("k"), "x")).await;
    let replay = send(&app, write("/things", "ana", Some("k"), "x")).await;
    assert_eq!(replay.headers()[REPLAYED], "true");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_redirect_is_kept_as_the_write_it_answers() {
    let pool = pool().await;
    let runs = Runs::default();
    let routes = {
        let runs = runs.clone();
        Router::new().route(
            "/form",
            post(move || async move {
                let run = runs.add();
                axum::response::Redirect::to(&format!("/done/{run}"))
            }),
        )
    };
    let app = routes.layer(from_fn_with_state(keys(&pool), idempotent));

    send(&app, write("/form", "ana", Some("k"), "x")).await;
    let replay = send(&app, write("/form", "ana", Some("k"), "x")).await;
    assert_eq!(replay.status(), StatusCode::SEE_OTHER);
    assert_eq!(replay.headers()[header::LOCATION], "/done/1");
    assert_eq!(replay.headers()[REPLAYED], "true");
    assert_eq!(runs.count(), 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn an_empty_answer_replays_as_an_empty_answer() {
    let pool = pool().await;
    let runs = Runs::default();
    let routes = {
        let runs = runs.clone();
        Router::new().route(
            "/none",
            get(|| async {}).post(move || async move {
                runs.add();
                StatusCode::NO_CONTENT
            }),
        )
    };
    let app = routes.layer(from_fn_with_state(keys(&pool), idempotent));

    send(&app, write("/none", "ana", Some("k"), "")).await;
    let replay = send(&app, write("/none", "ana", Some("k"), "")).await;
    assert_eq!(replay.status(), StatusCode::NO_CONTENT);
    assert_eq!(replay.headers()[REPLAYED], "true");
    assert_eq!(bytes(replay).await, b"");
    assert_eq!(runs.count(), 1);
}
