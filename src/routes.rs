//! Route tables that become an ordinary Axum [`Router`].
//!
//! ```
//! use axa::prelude::*;
//!
//! async fn list() -> &'static str { "[]" }
//! async fn show(Path(id): Path<u32>) -> String { id.to_string() }
//!
//! let folders = Routes::new()
//!     .prefix("/api/folders")
//!     .add("/", get(list))
//!     .add("/{id}", get(show));
//! # let _ = AppRoutes::empty().add_route(folders);
//! ```
//!
//! [`AppRoutes::mounted`] lists what is served, for a route census test.
//!
//! [`Routes::timeout`] and [`Routes::add_with_timeout`] give a route its own
//! time limit in place of `REQUEST_TIMEOUT_SECONDS`:
//!
//! ```
//! use std::time::Duration;
//! use axa::prelude::*;
//!
//! async fn export() -> &'static str { "..." }
//!
//! let reports = Routes::new()
//!     .prefix("/api/reports")
//!     .add_with_timeout("/export", get(export), Duration::from_mins(10));
//! # let _ = AppRoutes::empty().add_route(reports);
//! ```

use std::time::Duration;

use axum::{Router, routing::MethodRouter};

use crate::{AppContext, Result, config::Config, health, middleware};

/// One route in a [`Routes`] group.
#[derive(Clone, Debug)]
pub struct Handler {
    /// The path, relative to the group's prefix.
    pub uri: String,
    /// The Axum method router serving it.
    pub method: MethodRouter<AppContext>,
    /// Its own time limit, in place of the group's and the standard stack's.
    pub timeout: Option<Duration>,
}

impl Handler {
    /// The methods this route answers, in upper case. `HEAD` is left out
    /// beside `GET`, and a handler for every method shows as `ANY`.
    #[must_use]
    pub fn methods(&self) -> Vec<String> {
        methods_of(&self.method)
    }
}

/// The methods a method router answers, in upper case. `get` also answers
/// `HEAD`, which no route documents, so `HEAD` is left out beside `GET`. A
/// handler for every method, such as `any` or a `fallback`, shows as `ANY`.
///
/// Axum keeps this private, and its debug text is the one place that says, so
/// this reads that. `the_debug_text_still_says_what_is_mounted` fails if it
/// changes.
fn methods_of(router: &MethodRouter<AppContext>) -> Vec<String> {
    const FIELDS: [&str; 9] = [
        "get", "head", "delete", "options", "patch", "post", "put", "trace", "connect",
    ];
    let text = format!("{router:?}");
    let value = |field: &str| text.split(&format!("{field}: ")).nth(1);
    let answers = |field: &str| value(field).is_some_and(|rest| !rest.starts_with("None"));
    let get = answers("get");
    let mut methods: Vec<String> = FIELDS
        .iter()
        .filter(|field| answers(field) && !(get && **field == "head"))
        .map(|field| field.to_ascii_uppercase())
        .collect();
    if value("fallback").is_some_and(|rest| !rest.starts_with("Default")) {
        methods.push("ANY".to_string());
    }
    methods
}

/// A group of routes under one prefix.
///
/// The fields are public so a service's tests can check what is mounted
/// without booting it.
#[derive(Clone, Default, Debug)]
pub struct Routes {
    /// The prefix shared by the group, if any.
    pub prefix: Option<String>,
    /// The group's routes, in declaration order.
    pub handlers: Vec<Handler>,
    /// A time limit for the whole group, in place of the standard stack's.
    pub timeout: Option<Duration>,
}

impl Routes {
    /// An empty group.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the prefix shared by the group.
    #[must_use]
    pub fn prefix(mut self, prefix: &str) -> Self {
        self.prefix = Some(prefix.to_string());
        self
    }

    /// Give the whole group its own time limit, in place of
    /// `REQUEST_TIMEOUT_SECONDS`. It may be longer or shorter, and it applies
    /// even where that setting is `0`. Zero here means no limit.
    #[must_use]
    pub fn timeout(mut self, limit: Duration) -> Self {
        self.timeout = Some(limit);
        self
    }

    /// Add a handler. `"/"` means the prefix itself.
    #[must_use]
    pub fn add(mut self, path: &str, handler: MethodRouter<AppContext>) -> Self {
        self.handlers.push(Handler {
            uri: path.to_string(),
            method: handler,
            timeout: None,
        });
        self
    }

    /// Add a handler with its own time limit, which beats the group's. See
    /// [`Routes::timeout`].
    #[must_use]
    pub fn add_with_timeout(
        mut self,
        path: &str,
        handler: MethodRouter<AppContext>,
        limit: Duration,
    ) -> Self {
        self.handlers.push(Handler {
            uri: path.to_string(),
            method: handler,
            timeout: Some(limit),
        });
        self
    }

    /// Every route in the group as `(method, full path)`, in declaration
    /// order. See [`AppRoutes::mounted`].
    #[must_use]
    pub fn mounted(&self) -> Vec<(String, String)> {
        let prefix = self.prefix.as_deref().unwrap_or_default();
        self.handlers
            .iter()
            .flat_map(|handler| {
                let path = join(&[prefix, &handler.uri]);
                handler
                    .methods()
                    .into_iter()
                    .map(move |method| (method, path.clone()))
            })
            .collect()
    }
}

/// Join path parts, collapsing repeated slashes and dropping a trailing one.
fn join(prefixes: &[&str]) -> String {
    let joined = prefixes.join("/");
    let path = joined
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    format!("/{path}")
}

/// All of a service's route groups.
#[derive(Clone, Default, Debug)]
pub struct AppRoutes {
    groups: Vec<Routes>,
}

impl AppRoutes {
    /// Start with the `/_ping`, `/_health`, and `/_readiness` routes.
    #[must_use]
    pub fn with_default_routes() -> Self {
        Self::empty().add_route(health::routes())
    }

    /// Start with no routes.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a group.
    #[must_use]
    pub fn add_route(mut self, group: Routes) -> Self {
        self.groups.push(group);
        self
    }

    /// Every mounted route as `(method, full path)`, in declaration order, so
    /// a census test can compare what is served with what is written down.
    /// `HEAD` is left out beside `GET`, and a handler for every method shows
    /// as `ANY`.
    ///
    /// ```
    /// use axa::prelude::*;
    ///
    /// async fn list() -> &'static str { "[]" }
    ///
    /// let routes = AppRoutes::empty()
    ///     .add_route(Routes::new().prefix("/api/things").add("/", get(list).post(list)));
    /// assert_eq!(
    ///     routes.mounted(),
    ///     [
    ///         ("GET".to_string(), "/api/things".to_string()),
    ///         ("POST".to_string(), "/api/things".to_string()),
    ///     ]
    /// );
    /// ```
    #[must_use]
    pub fn mounted(&self) -> Vec<(String, String)> {
        self.groups.iter().flat_map(Routes::mounted).collect()
    }

    /// Every full path, with each group's handler, in declaration order. A
    /// handler's own limit beats its group's.
    fn collect(self) -> Vec<(String, MethodRouter<AppContext>)> {
        self.groups
            .into_iter()
            .flat_map(|group| {
                let prefix = group.prefix.unwrap_or_default();
                let group_limit = group.timeout;
                group.handlers.into_iter().map(move |handler| {
                    let path = join(&[&prefix, &handler.uri]);
                    match handler.timeout.or(group_limit) {
                        Some(limit) => (path, middleware::limit_route(handler.method, limit)),
                        None => (path, handler.method),
                    }
                })
            })
            .collect()
    }

    /// The routes as a stateless router with no middleware applied.
    pub fn into_router(self, ctx: AppContext) -> Router {
        let mut router = Router::new();
        for (path, handler) in self.collect() {
            router = router.route(&path, handler);
        }
        router.with_state(ctx)
    }

    /// The routes as a router with the standard middleware applied.
    ///
    /// # Errors
    ///
    /// Fails when a configured CORS origin is invalid.
    pub fn into_app(self, ctx: AppContext) -> Result<Router> {
        let config: std::sync::Arc<Config> = ctx.config.clone();
        middleware::apply(self.into_router(ctx), &config.http)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Environment, config::Defaults};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::{any, connect, delete, get, head, options, patch, post, put, trace},
    };
    use std::time::Duration;
    use tower::ServiceExt;

    fn context() -> AppContext {
        context_with(0)
    }

    /// A context whose standard stack times out after `seconds`.
    fn context_with(seconds: u64) -> AppContext {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let mut config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        config.http.timeout_seconds = seconds;
        #[cfg(not(feature = "sea-orm"))]
        return AppContext::for_test(Environment::Test, config);
        #[cfg(feature = "sea-orm")]
        return AppContext::new(
            Environment::Test,
            sea_orm::DatabaseConnection::default(),
            config,
        );
    }

    async fn status(router: Router, method: &str, path: &str) -> StatusCode {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        router.oneshot(request).await.unwrap().status()
    }

    fn table() -> AppRoutes {
        AppRoutes::empty().add_route(
            Routes::new()
                .prefix("/api/things")
                .add("/", get(|| async { "list" }))
                .add("/", post(|| async { "create" }))
                .add("/{id}", get(|| async { "show" })),
        )
    }

    #[test]
    fn groups_expose_what_they_mount() {
        let group = Routes::new()
            .prefix("/fhir")
            .add("/metadata", get(|| async { "" }));
        assert_eq!(group.prefix.as_deref(), Some("/fhir"));
        assert!(group.handlers.iter().any(|h| h.uri == "/metadata"));
        assert_eq!(Routes::new().prefix, None);
    }

    #[tokio::test]
    async fn a_trailing_slash_route_is_served_at_the_prefix() {
        let router = table().into_router(context());
        assert_eq!(status(router, "GET", "/api/things").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn two_methods_on_one_path_merge() {
        let router = table().into_router(context());
        assert_eq!(
            status(router.clone(), "GET", "/api/things").await,
            StatusCode::OK
        );
        assert_eq!(status(router, "POST", "/api/things").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn path_parameters_route() {
        let router = table().into_router(context());
        assert_eq!(status(router, "GET", "/api/things/7").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn an_unknown_path_is_404() {
        let router = table().into_router(context());
        assert_eq!(status(router, "GET", "/nope").await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn into_app_applies_the_standard_middleware() {
        let router = table().into_app(context()).unwrap();
        let request = Request::builder()
            .uri("/api/things")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert!(response.headers().contains_key(middleware::X_REQUEST_ID));
    }

    fn method_names(router: &MethodRouter<AppContext>) -> Vec<String> {
        methods_of(router)
    }

    #[test]
    fn the_debug_text_still_says_what_is_mounted() {
        async fn answer() {}
        let cases: [(MethodRouter<AppContext>, &str); 8] = [
            (get(answer), "GET"),
            (delete(answer), "DELETE"),
            (options(answer), "OPTIONS"),
            (patch(answer), "PATCH"),
            (post(answer), "POST"),
            (put(answer), "PUT"),
            (trace(answer), "TRACE"),
            (connect(answer), "CONNECT"),
        ];
        for (router, method) in cases {
            assert_eq!(method_names(&router), [method]);
        }
        assert_eq!(method_names(&head(answer)), ["HEAD"]);
        assert_eq!(method_names(&any(answer)), ["ANY"]);
        // The shape everything above depends on, so that a change in Axum
        // fails here by name.
        let text = format!("{:?}", get::<_, _, AppContext>(answer));
        for field in [
            "get: BoxedHandler",
            "head: None",
            "delete: None",
            "options: None",
            "patch: None",
            "post: None",
            "put: None",
            "trace: None",
            "connect: None",
            "fallback: Default(",
        ] {
            assert!(text.contains(field), "{field} is not in {text}");
        }
    }

    #[test]
    fn methods_survive_a_layer() {
        async fn answer() {}
        let router: MethodRouter<AppContext> = get(answer).post(answer);
        let layered = middleware::limit_route(router, Duration::from_secs(1));
        assert_eq!(method_names(&layered), ["GET", "POST"]);
    }

    #[test]
    fn head_is_left_out_beside_get() {
        async fn answer() {}
        let router: MethodRouter<AppContext> = get(answer).head(answer).post(answer);
        assert_eq!(method_names(&router), ["GET", "POST"]);
    }

    #[test]
    fn mounted_lists_every_method_and_full_path() {
        let group = Routes::new()
            .prefix("/fhir")
            .add("/metadata", get(|| async { "" }).delete(|| async { "" }));
        let routes = table().add_route(group);
        let want = [
            ("GET", "/api/things"),
            ("POST", "/api/things"),
            ("GET", "/api/things/{id}"),
            ("GET", "/fhir/metadata"),
            ("DELETE", "/fhir/metadata"),
        ]
        .map(|(method, path)| (method.to_string(), path.to_string()));
        assert_eq!(routes.mounted(), want);
    }

    #[test]
    fn mounted_includes_the_health_routes_and_a_handler_for_any_method() {
        let routes = AppRoutes::with_default_routes()
            .add_route(Routes::new().add("/hook", any(|| async { "" })));
        let all = routes.mounted();
        assert!(all.contains(&("GET".to_string(), "/_readiness".to_string())));
        assert!(all.contains(&("ANY".to_string(), "/hook".to_string())));
    }

    /// One slow route under a limit of its own, one not, behind a 60-second
    /// stack. Every handler takes 90 seconds. Time is paused, so this runs at
    /// once.
    fn slow_table(limit: Duration) -> AppRoutes {
        async fn slow() -> &'static str {
            tokio::time::sleep(Duration::from_secs(90)).await;
            "done"
        }
        AppRoutes::empty()
            .add_route(Routes::new().add("/plain", get(slow)).add_with_timeout(
                "/export",
                get(slow),
                limit,
            ))
            .add_route(
                Routes::new()
                    .prefix("/reports")
                    .timeout(limit)
                    .add("/one", get(slow))
                    .add_with_timeout("/two", get(slow), Duration::from_secs(1)),
            )
    }

    #[tokio::test(start_paused = true)]
    async fn a_route_with_a_longer_limit_outlives_the_stack_and_others_do_not() {
        let app = slow_table(Duration::from_mins(2))
            .into_app(context_with(60))
            .unwrap();
        assert_eq!(status(app.clone(), "GET", "/export").await, StatusCode::OK);
        assert_eq!(
            status(app, "GET", "/plain").await,
            StatusCode::REQUEST_TIMEOUT
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_route_is_still_cut_off_at_its_own_limit() {
        let app = slow_table(Duration::from_secs(75))
            .into_app(context_with(60))
            .unwrap();
        assert_eq!(
            status(app, "GET", "/export").await,
            StatusCode::REQUEST_TIMEOUT
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_route_limit_answers_as_a_problem() {
        let app = slow_table(Duration::from_secs(75))
            .into_app(context_with(60))
            .unwrap();
        let request = Request::builder()
            .uri("/export")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            axa_problem::PROBLEM_JSON
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_group_limit_applies_and_a_routes_own_limit_beats_it() {
        let app = slow_table(Duration::from_mins(2))
            .into_app(context_with(60))
            .unwrap();
        assert_eq!(
            status(app.clone(), "GET", "/reports/one").await,
            StatusCode::OK
        );
        assert_eq!(
            status(app, "GET", "/reports/two").await,
            StatusCode::REQUEST_TIMEOUT
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_shorter_route_limit_applies_with_the_stack_timeout_off() {
        let app = slow_table(Duration::from_secs(1))
            .into_app(context_with(0))
            .unwrap();
        assert_eq!(
            status(app, "GET", "/export").await,
            StatusCode::REQUEST_TIMEOUT
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_route_limit_means_no_limit() {
        let app = slow_table(Duration::ZERO)
            .into_app(context_with(60))
            .unwrap();
        assert_eq!(status(app, "GET", "/export").await, StatusCode::OK);
    }

    #[tokio::test(start_paused = true)]
    async fn a_routes_own_limit_beats_a_timeout_override() {
        let mut ctx = context_with(60);
        let mut config = (*ctx.config).clone();
        config.http.timeout_overrides = vec![crate::config::TimeoutOverride {
            path_suffix: "/export".to_string(),
            seconds: 1,
        }];
        ctx.config = std::sync::Arc::new(config);
        let app = slow_table(Duration::from_mins(2)).into_app(ctx).unwrap();
        assert_eq!(status(app, "GET", "/export").await, StatusCode::OK);
    }

    #[tokio::test(start_paused = true)]
    async fn a_route_limit_covers_one_method_on_a_shared_path() {
        async fn slow() -> &'static str {
            tokio::time::sleep(Duration::from_secs(90)).await;
            "done"
        }
        let group = Routes::new()
            .add_with_timeout("/thing", get(slow), Duration::from_mins(2))
            .add("/thing", post(slow));
        let app = AppRoutes::empty()
            .add_route(group)
            .into_app(context_with(60))
            .unwrap();
        assert_eq!(status(app.clone(), "GET", "/thing").await, StatusCode::OK);
        assert_eq!(
            status(app, "POST", "/thing").await,
            StatusCode::REQUEST_TIMEOUT
        );
    }

    #[test]
    fn join_normalises_slashes() {
        assert_eq!(join(&["/api/folders", "/"]), "/api/folders");
        assert_eq!(join(&["/api/folders", "/{id}"]), "/api/folders/{id}");
        assert_eq!(join(&["", "/healthz"]), "/healthz");
        assert_eq!(join(&["/", "/"]), "/");
    }
}
