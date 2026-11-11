//! Basic axa service example
//!
//! This is a minimal example of an Axum service using axa infrastructure.
//! It demonstrates:
//! - Standard axa startup pattern
//! - Health check endpoint
//! - JSON API endpoint
//! - Error handling with axa::error
//!
//! Run with: cargo run
//! Test with: curl http://localhost:3000/health

use axa::{error::AppError, start_app};
use axum::{
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::net::SocketAddr;
use tracing::info;

/// Simple health check response
#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

/// Simple greeting response
#[derive(Serialize)]
struct GreetingResponse {
    message: String,
    service: String,
}

/// Create the application router
fn app() -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/", get(root))
        .route("/greet/:name", get(greet))
}

/// Health check endpoint
/// 
/// Returns 200 OK with service health status.
/// This is a standard endpoint that should be present in all services.
async fn health_check() -> Result<Json<HealthResponse>, AppError> {
    Ok(Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }))
}

/// Root endpoint
/// 
/// Returns basic service information.
async fn root() -> Result<Json<GreetingResponse>, AppError> {
    Ok(Json(GreetingResponse {
        message: "Welcome to axa basic service".to_string(),
        service: "basic-service".to_string(),
    }))
}

/// Greeting endpoint with parameter
/// 
/// Returns a personalized greeting.
async fn greet(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<GreetingResponse>, AppError> {
    Ok(Json(GreetingResponse {
        message: format!("Hello, {}!", name),
        service: "basic-service".to_string(),
    }))
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("basic_service=info,tower_http=info,axa=info")
        .init();

    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    info!("Starting basic-service on {}", addr);

    // Start the application using axa's start_app helper
    // This provides consistent startup behavior across all axa services
    start_app(app(), addr).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_check() {
        let app = app();

        let request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_root() {
        let app = app();

        let request = Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_greet() {
        let app = app();

        let request = Request::builder()
            .uri("/greet/World")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
