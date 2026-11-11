//! Service Template
//!
//! A template for building production-ready services with axa.
//!
//! This template follows axa's conventions:
//! - Library, not framework: main.rs stays ordinary Axum
//! - Spec-first: Document behavior before implementing
//! - Fail-closed: Production refuses unsafe configurations
//! - Audit-friendly: Every request path is traceable

use axa::{error::AppError, start_app};
use std::net::SocketAddr;
use tracing::info;

mod app;
mod config;
mod error;
mod health;

use app::app;
use config::Config;

/// Application entry point
///
/// Initializes logging, loads configuration, builds the app, and starts the server.
#[tokio::main]
async fn main() {
    // Initialize structured logging
    // In production, use JSON format; in development, use pretty format
    if std::env::var("LOG_FORMAT").as_deref() == Ok("json") {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
            .init();
    } else {
        tracing_subscriber::fmt()
            .pretty()
            .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
            .init();
    }

    // Load configuration from environment variables
    // In production, these come from the environment or secrets
    // In development, they can come from a .env file
    let config = Config::from_env().expect("Failed to load configuration");

    info!(
        version = env!("CARGO_PKG_VERSION"),
        name = env!("CARGO_PKG_NAME"),
        "Starting service"
    );

    // Build the application router with shared state
    let app = app::app(config.clone());

    // Determine the address to listen on
    let addr = SocketAddr::from((
        [0, 0, 0, 0],
        config.port.unwrap_or(3000),
    ));

    info!(address = %addr, "Listening");

    // Start the application using axa's start_app helper
    // This provides:
    // - Connection limits
    // - Request timeouts
    // - Graceful shutdown
    // - Consistent startup behavior
    start_app(app, addr)
        .await
        .expect("Failed to start service");
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::app;
    use axum::body::Body;
    use axum::http::Request;
    use config::Config;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_check() {
        // Create test configuration
        let config = Config::default();

        // Build the app
        let app = app(config);

        // Send a request to the health endpoint
        let request = Request::builder()
            .uri("/_health/live")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
