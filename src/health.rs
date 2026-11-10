//! The three health routes, one for each of the probes Kubernetes gives a container:
//!
//! | Route | Probe | A failure means | Asks |
//! |---|---|---|---|
//! | [`LIVE`], `/_health/live` | Liveness | Kill and restart the container | Nothing: it answers once the server is running |
//! | [`READY`], `/_health/ready` | Readiness | Send the pod no traffic | The database and queue Axa holds, and the named checks in [`AppContext::health_checks`]. It answers `503` from the moment shutdown begins |
//! | [`STARTUP`], `/_health/startup` | Startup | Not booted yet: hold off the other two | The database Axa holds. Once it passes, it passes for good |
//!
//! No route asks a third party. Restarting a container does not bring back a
//! payment provider, and a pod that is not ready gets no traffic at all, so every
//! visitor sees a `503` where the service could have shown a cached page or a
//! "payments are offline" banner. A named check is for something the service owns,
//! such as its Redis or its search index. A check is an ordinary async closure:
//!
//! ```
//! use axa::health::Checks;
//!
//! let checks = Checks::new().add("search", || async {
//!     // Call the dependency here. The error text is logged, never sent.
//!     Ok::<(), std::io::Error>(())
//! });
//! # let _ = checks;
//! ```
//!
//! The route is public, so one answer is reused for a few seconds and only one
//! round of checks runs at a time. However often the route is called, a
//! dependency is asked at most once per round. A check that outlasts its time
//! limit, or panics, counts as failed.
//!
//! A passing answer is bare JSON. A failing one is an RFC 9457 problem, which still
//! carries `ok` and `failed`.

use std::{fmt, future::Future, pin::Pin, sync::Arc, time::Duration};

use axa_problem::Problem;
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use tokio::{
    sync::Mutex,
    task::JoinSet,
    time::{Instant, timeout},
};

use crate::{AppContext, routes::Routes, server::Shutdown};

/// The liveness route: `200` once the server is running, and nothing asked.
pub const LIVE: &str = "/_health/live";

/// The readiness route: `200` when what the service owns answers.
pub const READY: &str = "/_health/ready";

/// The startup route: `200` once the service has booted and its database answers.
pub const STARTUP: &str = "/_health/startup";

/// How long one answer is reused, unless [`Checks::reuse_for`] says otherwise.
pub const DEFAULT_REUSE: Duration = Duration::from_secs(5);

/// How long a check may run, unless [`Checks::time_limit`] says otherwise.
pub const DEFAULT_TIME_LIMIT: Duration = Duration::from_secs(2);

type Outcome = Result<(), String>;
type Run = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Outcome> + Send>> + Send + Sync>;

/// One named check.
#[derive(Clone)]
struct Check {
    name: String,
    run: Run,
}

impl Check {
    fn new<F, Fut, E>(name: &str, check: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: fmt::Display,
    {
        Self {
            name: name.to_string(),
            run: Arc::new(move || {
                let check = check();
                Box::pin(async move { check.await.map_err(|error| error.to_string()) })
            }),
        }
    }
}

/// What `/_health/ready` or `/_health/startup` says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Answer {
    ok: bool,
    /// The names of the checks that failed. Never their error text.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    failed: Vec<String>,
}

/// The last answer of one route, and when it was given.
type Last = Arc<Mutex<Option<(Instant, Answer)>>>;

/// The named checks behind `/_health/ready`, and the last answers of the readiness
/// and startup routes. A named check is for something the service owns, never a
/// third party: see [the module](self).
///
/// Clones share the last answers, so a clone in each request reuses them.
#[derive(Clone)]
pub struct Checks {
    named: Vec<Check>,
    reuse_for: Duration,
    time_limit: Duration,
    last: Last,
    startup: Last,
}

impl Default for Checks {
    fn default() -> Self {
        Self {
            named: Vec::new(),
            reuse_for: DEFAULT_REUSE,
            time_limit: DEFAULT_TIME_LIMIT,
            last: Arc::default(),
            startup: Arc::default(),
        }
    }
}

impl fmt::Debug for Checks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.named.iter().map(|c| c.name.as_str()).collect();
        f.debug_struct("Checks")
            .field("checks", &names)
            .field("reuse_for", &self.reuse_for)
            .field("time_limit", &self.time_limit)
            .finish_non_exhaustive()
    }
}

impl Checks {
    /// No checks.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a named check. Each round calls `check` once, and `Ok` means the
    /// dependency answered. The name is sent to whoever calls `/_health/ready`,
    /// so make it a label, not an address. The error text is only logged. Check
    /// only what the service owns: a third party that is down must not take the
    /// service out of rotation.
    #[must_use]
    pub fn add<F, Fut, E>(mut self, name: &str, check: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: fmt::Display,
    {
        self.named.push(Check::new(name, check));
        self
    }

    /// How long to reuse one answer, whether it passed or failed.
    #[must_use]
    pub fn reuse_for(mut self, reuse_for: Duration) -> Self {
        self.reuse_for = reuse_for;
        self
    }

    /// How long each check may run before it counts as failed.
    #[must_use]
    pub fn time_limit(mut self, time_limit: Duration) -> Self {
        self.time_limit = time_limit;
        self
    }

    /// The reused answer, or a new round of checks when it has gone stale.
    ///
    /// The round runs in its own task, so a caller that hangs up cannot cancel
    /// it and leave nothing to reuse. The lock is what stops two rounds
    /// overlapping: callers that arrive during a round wait for its answer.
    async fn answer(&self, built_in: Vec<Check>) -> Answer {
        self.answer_in(&self.last, built_in, true, false).await
    }

    /// The startup answer: the built-in checks only, reused while it fails, and kept
    /// for good once it passes.
    async fn startup_answer(&self, built_in: Vec<Check>) -> Answer {
        self.answer_in(&self.startup, built_in, false, true).await
    }

    /// The answer kept in `slot`, or a new round of `built_in`, and of the named
    /// checks when `named`, once it has gone stale. With `keep_passing`, a passing
    /// answer never goes stale.
    async fn answer_in(
        &self,
        slot: &Last,
        built_in: Vec<Check>,
        named: bool,
        keep_passing: bool,
    ) -> Answer {
        let checks = self.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut last = slot.lock().await;
            if let Some((at, answer)) = last.as_ref()
                && ((keep_passing && answer.ok) || at.elapsed() < checks.reuse_for)
            {
                return answer.clone();
            }
            let answer = checks.round(built_in, named).await;
            *last = Some((Instant::now(), answer.clone()));
            answer
        })
        .await
        .unwrap_or_else(|error| {
            tracing::error!(%error, "readiness_round_failed");
            Answer {
                ok: false,
                failed: vec!["readiness".to_string()],
            }
        })
    }

    /// Run every check at once. Each runs in its own task, so a check that
    /// panics is a failed check. Anything that does not report success failed.
    async fn round(&self, built_in: Vec<Check>, named: bool) -> Answer {
        let named = if named { self.named.as_slice() } else { &[] };
        let checks: Vec<Check> = built_in.into_iter().chain(named.iter().cloned()).collect();
        let mut running = JoinSet::new();
        for (index, check) in checks.iter().enumerate() {
            let (run, limit) = (check.run.clone(), self.time_limit);
            running.spawn(async move { (index, timeout(limit, run()).await) });
        }
        let mut passed = vec![false; checks.len()];
        while let Some(finished) = running.join_next().await {
            match finished {
                Ok((index, Ok(Ok(())))) => passed[index] = true,
                Ok((index, Ok(Err(error)))) => {
                    tracing::error!(check = checks[index].name, %error, "readiness_check_failed");
                }
                Ok((index, Err(_))) => {
                    tracing::error!(check = checks[index].name, "readiness_check_timed_out");
                }
                Err(error) => tracing::error!(%error, "readiness_check_panicked"),
            }
        }
        let failed: Vec<String> = checks
            .into_iter()
            .zip(passed)
            .filter(|(_, passed)| !passed)
            .map(|(check, _)| check.name)
            .collect();
        Answer {
            ok: failed.is_empty(),
            failed,
        }
    }
}

#[derive(Serialize)]
struct Health {
    ok: bool,
}

async fn ok() -> Json<Health> {
    Json(Health { ok: true })
}

/// The checks Axa can write itself: the database it holds, and the queue too unless
/// `database_only`.
fn built_in(ctx: &AppContext, database_only: bool) -> Vec<Check> {
    #[cfg_attr(
        all(axa_db = "none", not(feature = "queue")),
        allow(unused_mut, clippy::let_and_return)
    )]
    let mut checks = Vec::new();
    #[cfg(not(axa_db = "none"))]
    {
        let db = ctx.db.clone();
        checks.push(Check::new("database", move || {
            let db = db.clone();
            async move { crate::context::ping(&db).await }
        }));
    }
    #[cfg(feature = "queue")]
    if let Some(queue) = &ctx.queue_provider
        && !database_only
    {
        let queue = queue.clone();
        checks.push(Check::new("queue", move || {
            let queue = queue.clone();
            async move { queue.ping().await }
        }));
    }
    let _ = (ctx, database_only);
    checks
}

async fn readiness(State(ctx): State<AppContext>, shutdown: Shutdown) -> Response {
    // A service shutting down takes itself out of rotation, whatever its checks say.
    if shutdown.has_begun() {
        return respond(
            Answer {
                ok: false,
                failed: Vec::new(),
            },
            "shutting_down",
        );
    }
    let answer = ctx.health_checks.answer(built_in(&ctx, false)).await;
    respond(answer, "not_ready")
}

async fn startup(State(ctx): State<AppContext>) -> Response {
    let answer = ctx.health_checks.startup_answer(built_in(&ctx, true)).await;
    respond(answer, "not_started")
}

/// Bare JSON when `answer` passed, and otherwise a `503` problem with `code`.
fn respond(answer: Answer, code: &'static str) -> Response {
    if answer.ok {
        return Json(answer).into_response();
    }
    Problem::new(StatusCode::SERVICE_UNAVAILABLE)
        .with_extension("code", code.into())
        .with_extension("failed", answer.failed.into())
        .with_extension("ok", false.into())
        .into_response()
}

/// The health routes: [`LIVE`], [`READY`], and [`STARTUP`].
#[must_use]
pub fn routes() -> Routes {
    Routes::new()
        .add(LIVE, get(ok))
        .add(READY, get(readiness))
        .add(STARTUP, get(startup))
}

// With a database in the context, a context that was never opened fails readiness.
#[cfg(all(test, axa_db = "none"))]
mod tests {
    use super::*;
    use crate::{
        Environment,
        config::{Config, Defaults},
        routes::AppRoutes,
    };
    use axum::{body::Body, http::Request};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    fn context() -> AppContext {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        AppContext::for_test(Environment::Test, config)
    }

    async fn answer(router: axum::Router) -> (StatusCode, String) {
        let request = Request::builder().uri(READY).body(Body::empty()).unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    /// The body of a failing answer, naming `failed`.
    fn refused(failed: &str) -> String {
        format!(
            r#"{{"type":"about:blank","title":"Service Unavailable","status":503,"code":"not_ready","failed":{failed},"ok":false}}"#
        )
    }

    fn with_checks(checks: Checks) -> axum::Router {
        let mut ctx = context();
        ctx.health_checks = checks;
        AppRoutes::with_default_routes().into_router(ctx)
    }

    async fn passes() -> Result<(), String> {
        Ok(())
    }

    async fn fails() -> Result<(), String> {
        Err("postgres://user:secret@10.0.0.1 refused".to_string())
    }

    /// A check that counts how often it is called.
    fn counted() -> (Arc<AtomicUsize>, Checks) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let checks = Checks::new().add("counted", move || {
            counter.fetch_add(1, Ordering::SeqCst);
            passes()
        });
        (calls, checks)
    }

    #[tokio::test]
    async fn every_health_route_answers_ok() {
        let router = AppRoutes::with_default_routes().into_router(context());
        for path in [LIVE, READY, STARTUP] {
            let request = Request::builder().uri(path).body(Body::empty()).unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
    }

    /// The status `path` answers with `checks`.
    async fn status_of(checks: Checks, path: &str) -> StatusCode {
        let request = Request::builder().uri(path).body(Body::empty()).unwrap();
        with_checks(checks).oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn a_failing_named_check_holds_back_only_readiness() {
        let failing = || Checks::new().add("cache", fails);
        assert_eq!(
            status_of(failing(), READY).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(status_of(failing(), LIVE).await, StatusCode::OK);
        assert_eq!(status_of(failing(), STARTUP).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn readiness_refuses_once_shutdown_begins_and_liveness_does_not() {
        let (announce, announced) = tokio::sync::watch::channel(true);
        let shutdown = crate::server::Shutdown::from_receiver(announced);
        let router = with_checks(Checks::new());
        let ask = |path: &'static str| {
            let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
            request.extensions_mut().insert(shutdown.clone());
            router.clone().oneshot(request)
        };
        let ready = ask(READY).await.unwrap();
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(ready.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains(r#""code":"shutting_down""#),
            "{body:?}"
        );
        assert_eq!(ask(LIVE).await.unwrap().status(), StatusCode::OK);
        drop(announce);
    }

    #[tokio::test]
    async fn liveness_asks_nothing() {
        let (calls, checks) = counted();
        assert_eq!(status_of(checks, LIVE).await, StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn startup_once_passed_stays_passed() {
        let up = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let database = |up: Arc<std::sync::atomic::AtomicBool>| {
            vec![Check::new("database", move || {
                let up = up.load(Ordering::SeqCst);
                async move { if up { Ok(()) } else { Err("down") } }
            })]
        };
        let checks = Checks::new().reuse_for(Duration::ZERO);
        assert!(checks.startup_answer(database(up.clone())).await.ok);
        up.store(false, Ordering::SeqCst);
        assert!(checks.startup_answer(database(up.clone())).await.ok);
        // Readiness asks again each round, and sees it.
        assert!(!checks.answer(database(up)).await.ok);
    }

    #[tokio::test]
    async fn readiness_with_every_check_passing_is_just_ok() {
        let router = with_checks(Checks::new().add("a", passes).add("b", passes));
        assert_eq!(
            answer(router).await,
            (StatusCode::OK, r#"{"ok":true}"#.to_string())
        );
    }

    #[tokio::test]
    async fn readiness_names_what_failed_in_the_order_added() {
        let checks = Checks::new()
            .add("search", fails)
            .add("mail", passes)
            .add("cache", fails);
        let (status, body) = answer(with_checks(checks)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, refused(r#"["search","cache"]"#));
    }

    #[tokio::test]
    async fn only_a_failing_answer_is_a_problem() {
        let call = |checks| async move {
            let request = Request::builder().uri(READY).body(Body::empty()).unwrap();
            let response = with_checks(checks).oneshot(request).await.unwrap();
            response.headers()[axum::http::header::CONTENT_TYPE].clone()
        };
        let passing = Checks::new().add("a", passes);
        assert_eq!(call(passing).await, "application/json");
        let failing = Checks::new().add("a", fails);
        assert_eq!(call(failing).await, axa_problem::PROBLEM_JSON);
    }

    #[tokio::test]
    async fn readiness_never_sends_a_checks_error_text() {
        let (_, body) = answer(with_checks(Checks::new().add("db", fails))).await;
        assert!(!body.contains("secret"), "{body}");
        assert!(!body.contains("10.0.0.1"), "{body}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_past_its_time_limit_fails() {
        let checks = Checks::new()
            .time_limit(Duration::from_secs(1))
            .add("slow", || async {
                tokio::time::sleep(Duration::from_hours(1)).await;
                Ok::<(), String>(())
            });
        let (status, body) = answer(with_checks(checks)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, refused(r#"["slow"]"#));
    }

    #[tokio::test]
    async fn a_check_that_panics_fails() {
        let checks = Checks::new()
            .add("fine", passes)
            .add("broken", || async { panic!("boom") as Result<(), String> });
        let (status, body) = answer(with_checks(checks)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, refused(r#"["broken"]"#));
    }

    #[tokio::test(start_paused = true)]
    async fn one_answer_is_reused_until_it_goes_stale() {
        let (calls, checks) = counted();
        let router = with_checks(checks.reuse_for(Duration::from_secs(5)));
        for _ in 0..50 {
            assert_eq!(answer(router.clone()).await.0, StatusCode::OK);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(6)).await;
        answer(router).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_answer_is_reused_too() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let router = with_checks(Checks::new().add("down", move || {
            counter.fetch_add(1, Ordering::SeqCst);
            fails()
        }));
        for _ in 0..50 {
            assert_eq!(
                answer(router.clone()).await.0,
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn callers_during_a_round_share_it() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let router = with_checks(Checks::new().add("slow", move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok::<(), String>(())
            }
        }));
        let callers: Vec<_> = (0..20)
            .map(|_| tokio::spawn(answer(router.clone())))
            .collect();
        for caller in callers {
            assert_eq!(caller.await.unwrap().0, StatusCode::OK);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_caller_that_hangs_up_does_not_waste_the_round() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let router = with_checks(Checks::new().add("slow", move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok::<(), String>(())
            }
        }));
        let hung_up = timeout(Duration::from_millis(10), answer(router.clone())).await;
        assert!(hung_up.is_err());
        assert_eq!(answer(router).await.0, StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "queue")]
    #[tokio::test]
    async fn readiness_names_a_queue_that_does_not_answer() {
        let mut ctx = context();
        ctx.queue_provider = Some(Arc::new(crate::queue::Queue::unreachable()));
        let router = AppRoutes::with_default_routes().into_router(ctx);
        let (status, body) = answer(router).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, refused(r#"["queue"]"#));
    }

    #[test]
    fn checks_print_their_names_only() {
        let text = format!("{:?}", Checks::new().add("search", passes));
        assert!(text.contains("search"), "{text}");
    }
}

// Every database but Toasty, which connects when its handle is made.
#[cfg(all(
    test,
    not(any(axa_db = "none", axa_db = "postgres_toasty", axa_db = "mysql_toasty"))
))]
mod database_tests {
    use super::*;
    use crate::{
        Environment,
        config::{Config, Defaults},
        routes::AppRoutes,
    };
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn readiness_fails_when_the_database_does_not_answer() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        // A connection that was never opened cannot answer a ping.
        let ctx = AppContext::for_test(Environment::Test, config);
        let router = AppRoutes::with_default_routes().into_router(ctx);
        let request = Request::builder().uri(READY).body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        // Nor has it started: startup asks for the primary database too.
        let request = Request::builder().uri(STARTUP).body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains(r#""code":"not_started""#),
            "{body:?}"
        );
        // Liveness still passes: the process is up.
        let request = Request::builder().uri(LIVE).body(Body::empty()).unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[cfg(all(
    test,
    not(any(axa_db = "none", axa_db = "postgres_toasty", axa_db = "mysql_toasty"))
))]
mod named_database_tests {
    use super::*;
    use crate::{
        Environment,
        config::{Config, Defaults},
    };

    #[tokio::test]
    async fn readiness_names_the_database() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        let ctx = AppContext::for_test(Environment::Test, config);
        let answer = ctx.health_checks.answer(built_in(&ctx, false)).await;
        assert_eq!(answer.failed, ["database"]);
    }
}

#[cfg(all(test, feature = "postgres-sqlx"))]
mod sqlx_tests {
    use super::*;
    use axa_postgres_sqlx::sqlx::postgres::PgPoolOptions;

    /// `AppContext::db` is the pool, and readiness asks it without being told.
    /// Nothing listens on this port, so the pool cannot answer.
    #[cfg(axa_db = "postgres_sqlx")]
    #[tokio::test]
    async fn the_context_pool_is_checked_without_being_named() {
        let defaults = crate::config::Defaults {
            database: "",
            ..crate::config::Defaults::default()
        };
        let config =
            crate::config::Config::from_lookup(&crate::Environment::Test, &defaults, |_| None)
                .unwrap();
        let ctx = AppContext::for_test(crate::Environment::Test, config);
        let answer = ctx.health_checks.answer(built_in(&ctx, false)).await;
        assert_eq!(answer.failed, ["database"]);
    }

    /// A second pool, or one held beside `SeaORM`, is still a named check.
    #[tokio::test]
    async fn a_sqlx_pool_is_checked_by_name() {
        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy("postgres://axa:axa@127.0.0.1:1/axa")
            .unwrap();
        let checks = Checks::new().add("database", move || {
            let pool = pool.clone();
            async move { axa_postgres_sqlx::ping(&pool).await }
        });
        let answer = checks.answer(Vec::new()).await;
        assert_eq!(answer.failed, ["database"]);
    }
}
