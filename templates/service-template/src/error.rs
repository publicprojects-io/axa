//! Error Handling
//!
//! This module provides error handling utilities for the service.
//! It builds on axa's error handling with RFC 9457 Problem Details.

use axa::error::AppError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Custom error type for the service
///
/// Wraps axa's AppError to add service-specific error handling.
/// In most cases, you can just use AppError directly.
pub type ServiceError = AppError;

/// Problem Details response as per RFC 9457
///
/// This is already provided by axa::problem, but re-exported here for convenience.
pub use axa::problem::{Problem, ProblemBuilder};

/// Custom error variants for the service
///
/// Add your service-specific errors here.
#[derive(Debug)]
pub enum ServiceSpecificError {
    /// Validation error
    ValidationError(String),
    /// Not found error
    NotFound(String),
    /// Conflict error
    Conflict(String),
    /// Rate limit exceeded
    RateLimitExceeded,
    /// Service unavailable
    ServiceUnavailable(String),
}

impl std::fmt::Display for ServiceSpecificError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceSpecificError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            ServiceSpecificError::NotFound(resource) => {
                write!(f, "{} not found", resource)
            }
            ServiceSpecificError::Conflict(msg) => write!(f, "Conflict: {}", msg),
            ServiceSpecificError::RateLimitExceeded => {
                write!(f, "Rate limit exceeded")
            }
            ServiceSpecificError::ServiceUnavailable(msg) => {
                write!(f, "Service unavailable: {}", msg)
            }
        }
    }
}

impl std::error::Error for ServiceSpecificError {}

impl From<ServiceSpecificError> for ServiceError {
    fn from(err: ServiceSpecificError) -> Self {
        match err {
            ServiceSpecificError::ValidationError(msg) => {
                AppError::bad_request(msg)
            }
            ServiceSpecificError::NotFound(resource) => {
                AppError::not_found(resource)
            }
            ServiceSpecificError::Conflict(msg) => {
                AppError::conflict(msg)
            }
            ServiceSpecificError::RateLimitExceeded => {
                AppError::rate_limit_exceeded()
            }
            ServiceSpecificError::ServiceUnavailable(msg) => {
                AppError::service_unavailable(msg)
            }
        }
    }
}

/// Convert various error types to ServiceError
pub trait IntoServiceError<T> {
    fn into_service_error(self) -> Result<T, ServiceError>;
}

impl<T, E> IntoServiceError<T> for Result<T, E>
where
    E: Into<ServiceError>,
{
    fn into_service_error(self) -> Result<T, ServiceError> {
        self.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_error() {
        let err = ServiceSpecificError::ValidationError("Invalid email".to_string());
        let service_err: ServiceError = err.into();
        assert_eq!(service_err.status_code(), Some(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn test_not_found_error() {
        let err = ServiceSpecificError::NotFound("User 123".to_string());
        let service_err: ServiceError = err.into();
        assert_eq!(service_err.status_code(), Some(StatusCode::NOT_FOUND));
    }

    #[test]
    fn test_conflict_error() {
        let err = ServiceSpecificError::Conflict("Email already exists".to_string());
        let service_err: ServiceError = err.into();
        assert_eq!(service_err.status_code(), Some(StatusCode::CONFLICT));
    }

    #[test]
    fn test_rate_limit_error() {
        let err = ServiceSpecificError::RateLimitExceeded;
        let service_err: ServiceError = err.into();
        assert_eq!(service_err.status_code(), Some(StatusCode::TOO_MANY_REQUESTS));
    }

    #[test]
    fn test_service_unavailable_error() {
        let err = ServiceSpecificError::ServiceUnavailable("Database connection failed".to_string());
        let service_err: ServiceError = err.into();
        assert_eq!(service_err.status_code(), Some(StatusCode::SERVICE_UNAVAILABLE));
    }
}
