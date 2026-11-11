//! Application Router and State
//!
//! This module contains the main router and application state for the service.

use axum::routing::get;
use axum::{Router, Server};
use std::sync::Arc;

use crate::config::Config;
use crate::health::{health_live, health_ready, health_startup};

/// Application state shared across all routes
#[derive(Clone, Debug)]
pub struct AppState {
    /// Application configuration
    pub config: Config,
}

impl AppState {
    /// Create a new AppState from configuration
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

/// Create the application router
///
/// This function builds the router with all routes and middleware.
/// Add your routes here as the service grows.
pub fn app(config: Config) -> Router {
    // Create shared state
    let state = Arc::new(AppState::new(config));

    // Build the router
    Router::new()
        // Health check endpoints (required for all services)
        .route("/_health/live", get(health_live))
        .route("/_health/ready", get(health_ready))
        .route("/_health/startup", get(health_startup))
        // Add your routes here:
        // .route("/api/v1/endpoint", get(your_handler))
        // .route("/api/v1/endpoint", post(your_handler))
        .with_state(state)
}
