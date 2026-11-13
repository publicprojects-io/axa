//! A minimal Axa service with no database.
//!
//! It serves `/api/hello` and `/api/greet/{name}` under the standard middleware, with
//! the three health routes, and stops cleanly on `SIGTERM` or Ctrl-C.
//!
//! ```sh
//! cargo run
//! curl http://localhost:1241/api/greet/World
//! curl http://localhost:1241/_health/ready
//! ```

use axa::{
    Environment, Result,
    config::{Config, Defaults},
    logger,
    prelude::*,
    server,
};
use axa::extract::Path;
use serde::Serialize;

/// A greeting.
#[derive(Serialize)]
struct Greeting {
    message: String,
}

async fn hello() -> Result<Response> {
    format::json(Greeting {
        message: "Hello from basic-service".to_string(),
    })
}

async fn greet(Path(name): Path<String>) -> Result<Response> {
    if name.chars().count() > 100 {
        return axa::error::bad_request("name is too long");
    }
    format::json(Greeting {
        message: format!("Hello, {name}!"),
    })
}

/// The service's routes, with the health routes Axa adds.
fn routes() -> AppRoutes {
    AppRoutes::with_default_routes().add_route(
        Routes::new()
            .prefix("/api")
            .add("/hello", get(hello))
            .add("/greet/{name}", get(greet)),
    )
}

/// The settings for `environment`. An empty database name means no database.
fn config(environment: &Environment) -> Result<Config> {
    let defaults = Defaults {
        database: "",
        ..Defaults::default()
    };
    Config::from_env(environment, &defaults)
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = config(&environment)?;
    logger::init("basic_service", &config.logger);

    let server_config = config.server.clone();
    let ctx = AppContext::new(environment, (), config);
    let router = routes().into_app(ctx)?;
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

    fn app() -> axum::Router {
        let config = config(&Environment::Test).unwrap();
        routes()
            .into_app(AppContext::new(Environment::Test, (), config))
            .unwrap()
    }

    async fn get_path(path: &str) -> (StatusCode, String) {
        let request = Request::builder().uri(path).body(Body::empty()).unwrap();
        let response = app().oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn greet_says_hello_by_name() {
        let (status, body) = get_path("/api/greet/World").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"message":"Hello, World!"}"#);
    }

    #[tokio::test]
    async fn a_name_that_is_too_long_is_a_problem() {
        let (status, body) = get_path(&format!("/api/greet/{}", "a".repeat(101))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("name is too long"));
    }

    #[tokio::test]
    async fn the_health_routes_answer() {
        for path in ["/_health/live", "/_health/ready", "/_health/startup"] {
            assert_eq!(get_path(path).await.0, StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn an_unknown_path_is_a_problem() {
        let (status, body) = get_path("/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("\"status\":404"));
    }
}
