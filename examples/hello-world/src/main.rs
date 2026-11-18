//! The smallest Axa service: one route, the health probes, and the standard middleware.
//!
//! ```sh
//! cargo run
//! curl http://localhost:1241/
//! ```

use axa::{
    Environment, Result,
    config::{Config, Defaults},
    logger,
    prelude::*,
    server,
};

async fn hello() -> &'static str {
    "Hello, World!"
}

fn routes() -> AppRoutes {
    AppRoutes::with_default_routes().add_route(Routes::new().add("/", get(hello)))
}

/// The settings for `environment`, with no database.
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
    logger::init("hello_world", &config.logger);
    let server_config = config.server.clone();
    let router = routes().into_app(AppContext::new(environment, (), config))?;
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

    #[tokio::test]
    async fn it_says_hello() {
        let config = config(&Environment::Test).unwrap();
        let app = routes()
            .into_app(AppContext::new(Environment::Test, (), config))
            .unwrap();
        let request = Request::builder().uri("/").body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"Hello, World!");
    }
}
