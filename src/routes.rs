//! Route tables that become an ordinary Axum [`Router`].

use axum::{Router, routing::MethodRouter};

use crate::{AppContext, Result, config::Config, health, middleware};

/// A group of routes under one prefix.
#[derive(Clone, Default, Debug)]
pub struct Routes {
    prefix: String,
    handlers: Vec<(String, MethodRouter<AppContext>)>,
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
        self.prefix = prefix.to_string();
        self
    }

    /// Add a handler. `"/"` means the prefix itself.
    #[must_use]
    pub fn add(mut self, path: &str, handler: MethodRouter<AppContext>) -> Self {
        self.handlers.push((path.to_string(), handler));
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
                let prefix = group.prefix;
                group
                    .handlers
                    .into_iter()
                    .map(move |(path, handler)| (join(&[&prefix, &path]), handler))
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

    #[test]
    fn join_normalises_slashes() {
        assert_eq!(join(&["/api/folders", "/"]), "/api/folders");
        assert_eq!(join(&["/api/folders", "/{id}"]), "/api/folders/{id}");
        assert_eq!(join(&["", "/healthz"]), "/healthz");
        assert_eq!(join(&["/", "/"]), "/");
    }
}
