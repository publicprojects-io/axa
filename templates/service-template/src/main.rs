//! A service built with Axa.
//!
//! `main` stays ordinary Axum: it reads the settings, starts the logger, builds the
//! router, and serves it until `SIGTERM` or Ctrl-C.

use axa::{
    Environment, Result,
    config::{Config, Defaults},
    logger,
    prelude::AppContext,
    server,
};

mod routes;

/// The name the logger and the local database use. Change it to the service's own.
const SERVICE_NAME: &str = "service_template";

/// The settings for `environment`, from the environment's variables.
fn config(environment: &Environment) -> Result<Config> {
    let defaults = Defaults {
        // An empty name means this service has no database. Name one, such as
        // SERVICE_NAME, once a database feature is on.
        database: "",
        ..Defaults::default()
    };
    Config::from_env(environment, &defaults)
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    // Production refuses settings that are unsafe or cannot be read, here, before
    // anything starts.
    let config = config(&environment)?;
    logger::init(SERVICE_NAME, &config.logger);

    let server_config = config.server.clone();
    let ctx = AppContext::new(environment, (), config);
    let router = routes::routes().into_app(ctx)?;
    server::serve(router, &server_config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    /// The service's router under the `test` environment.
    pub(crate) fn app() -> axum::Router {
        let config = config(&Environment::Test).unwrap();
        routes::routes()
            .into_app(AppContext::new(Environment::Test, (), config))
            .unwrap()
    }

    #[tokio::test]
    async fn the_health_routes_answer() {
        for path in ["/_health/live", "/_health/ready", "/_health/startup"] {
            let request = Request::builder().uri(path).body(Body::empty()).unwrap();
            let response = app().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn the_service_answers_its_own_route() {
        let request = Request::builder()
            .uri("/api/v1/status")
            .body(Body::empty())
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("x-request-id"));
    }
}
