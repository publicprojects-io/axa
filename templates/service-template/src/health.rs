//! Health Check Endpoints
//!
//! This module provides the standard health check endpoints required by axa.
//! All services should have these endpoints for proper monitoring.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::error::ServiceError;

/// Health check response
#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

/// Detailed health check response with version info
#[derive(Serialize)]
struct DetailedHealthResponse {
    status: String,
    version: String,
    service: String,
}

/// Liveness probe
///
/// Returns 200 OK if the service is running.
/// This should always return 200 as long as the process is alive.
pub async fn health_live() -> impl IntoResponse {
    (
        StatusCode::OK,
        axum::Json(HealthResponse {
            status: "live".to_string(),
        }),
    )
}

/// Readiness probe
///
/// Returns 200 OK if the service is ready to accept traffic.
/// This should check dependencies like database connections.
///
/// In this template, it always returns ready. In production, you should
/// check your dependencies here.
pub async fn health_ready() -> impl IntoResponse {
    // TODO: Add dependency checks here
    // Example:
    // if database_connection().await.is_err() {
    //     return (StatusCode::SERVICE_UNAVAILABLE, Json(HealthResponse { status: "not ready".to_string() })).into_response();
    // }

    (
        StatusCode::OK,
        axum::Json(HealthResponse {
            status: "ready".to_string(),
        }),
    )
}

/// Startup probe
///
/// Returns 200 OK once the service has completed startup.
/// This is useful for Kubernetes liveness probes during startup.
///
/// Once the service is started, this should always return 200.
pub async fn health_startup() -> impl IntoResponse {
    (
        StatusCode::OK,
        axum::Json(DetailedHealthResponse {
            status: "started".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            service: env!("CARGO_PKG_NAME").to_string(),
        }),
    )
}

/// Extended health check with full service info
///
/// Returns detailed health information including dependencies.
pub async fn health_extended() -> impl IntoResponse {
    axum::Json(DetailedHealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        service: env!("CARGO_PKG_NAME").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn test_health_endpoint(
        handler: impl IntoResponse + 'static,
        expected_status: StatusCode,
    ) {
        let app = axum::Router::new().route("/test", axum::routing::get(handler));
        let request = Request::builder().uri("/test").body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected_status);
    }

    #[tokio::test]
    async fn test_health_live() {
        test_health_endpoint(health_live(), StatusCode::OK).await;
    }

    #[tokio::test]
    async fn test_health_ready() {
        test_health_endpoint(health_ready(), StatusCode::OK).await;
    }

    #[tokio::test]
    async fn test_health_startup() {
        test_health_endpoint(health_startup(), StatusCode::OK).await;
    }
}
