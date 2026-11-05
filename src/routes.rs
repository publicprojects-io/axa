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

use axum::{Router, routing::MethodRouter};

use crate::{AppContext, Result, config::Config, health, middleware};

/// One route in a [`Routes`] group.
#[derive(Clone, Debug)]
pub struct Handler {
    /// The path, relative to the group's prefix.
    pub uri: String,
    /// The Axum method router serving it.
    pub method: MethodRouter<AppContext>,
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

    /// Add a handler. `"/"` means the prefix itself.
    #[must_use]
    pub fn add(mut self, path: &str, handler: MethodRouter<AppContext>) -> Self {
        self.handlers.push(Handler {
            uri: path.to_string(),
            method: handler,
        });
        self
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

    /// Every full path, with each group's handler, in declaration order.
    fn collect(self) -> Vec<(String, MethodRouter<AppContext>)> {
        self.groups
            .into_iter()
            .flat_map(|group| {
                let prefix = group.prefix.unwrap_or_default();
                group
                    .handlers
                    .into_iter()
                    .map(move |handler| (join(&[&prefix, &handler.uri]), handler.method))
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
        routing::{get, post},
    };
    use tower::ServiceExt;

    fn context() -> AppContext {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        #[cfg(not(feature = "sea-orm"))]
        return AppContext::new(Environment::Test, (), config);
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

    #[test]
    fn join_normalises_slashes() {
        assert_eq!(join(&["/api/folders", "/"]), "/api/folders");
        assert_eq!(join(&["/api/folders", "/{id}"]), "/api/folders/{id}");
        assert_eq!(join(&["", "/healthz"]), "/healthz");
        assert_eq!(join(&["/", "/"]), "/");
    }
}
