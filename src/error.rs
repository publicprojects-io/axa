//! The error type handlers return, and its HTTP mapping.
//!
//! Internal failures always become a generic `500`: a database or transport
//! error is logged, never sent to the client.
//!
//! ```
//! use axa::{Error, Result};
//! use axum::{http::StatusCode, response::IntoResponse};
//!
//! fn find(id: u32) -> Result<&'static str> {
//!     if id == 1 { Ok("found") } else { Err(Error::NotFound) }
//! }
//!
//! let response = find(2).unwrap_err().into_response();
//! assert_eq!(response.status(), StatusCode::NOT_FOUND);
//! ```

use axum::{
    Json,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Result alias using [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// JSON body of an error response.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    /// Short machine-readable reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Structured detail, such as per-field validation errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<serde_json::Value>,
}

impl ErrorDetail {
    /// Build a detail from a reason and a description. An empty description
    /// is omitted from the body.
    #[must_use]
    pub fn new<A: Into<String>, B: Into<String> + AsRef<str>>(error: A, description: B) -> Self {
        let description = (!description.as_ref().is_empty()).then(|| description.into());
        Self {
            error: Some(error.into()),
            description,
            errors: None,
        }
    }

    /// Build a detail carrying only a reason.
    #[must_use]
    pub fn with_reason<A: Into<String>>(error: A) -> Self {
        Self {
            error: Some(error.into()),
            description: None,
            errors: None,
        }
    }
}

/// Everything a handler or service function can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A free-form internal failure. Becomes a generic `500`.
    #[error("{0}")]
    Message(String),
    /// The resource does not exist. Becomes `404`.
    #[error("not found")]
    NotFound,
    /// The caller is not authenticated. Becomes `401`; the text is logged only.
    #[error("{0}")]
    Unauthorized(String),
    /// The request is invalid. Becomes `400` and returns the text.
    #[error("{0}")]
    BadRequest(String),
    /// A generic internal failure. Becomes `500`.
    #[error("internal server error")]
    InternalServerError,
    /// An explicit status and body.
    #[error("{0} {1:?}")]
    CustomError(StatusCode, ErrorDetail),
    /// A rejected JSON body.
    #[error(transparent)]
    JsonRejection(#[from] JsonRejection),
    /// An Axum failure.
    #[error(transparent)]
    Axum(#[from] axum::Error),
    /// A malformed HTTP response or header.
    #[error(transparent)]
    Http(#[from] axum::http::Error),
    /// An I/O failure.
    #[error(transparent)]
    IO(#[from] std::io::Error),
    /// A JSON encoding failure.
    #[error(transparent)]
    JSON(#[from] serde_json::Error),
    /// A background-job failure.
    #[error("worker error: {0}")]
    Worker(String),
    /// A model-layer failure.
    #[cfg(feature = "sea-orm")]
    #[error(transparent)]
    Model(#[from] crate::model::ModelError),
    /// A database failure.
    #[cfg(feature = "sea-orm")]
    #[error(transparent)]
    DB(#[from] sea_orm::DbErr),
    /// A queue database failure.
    #[cfg(feature = "queue")]
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Any other error.
    #[error(transparent)]
    Any(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    /// Build a [`Error::Message`] from text.
    #[must_use]
    pub fn string(message: &str) -> Self {
        Self::Message(message.to_string())
    }

    /// Wrap any error as [`Error::Any`].
    #[must_use]
    pub fn wrap(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Any(Box::new(err))
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}

/// The body every internal failure shares.
fn internal() -> (StatusCode, ErrorDetail) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorDetail::new("internal_server_error", "Internal Server Error"),
    )
}

/// The body every missing resource shares.
fn resource_not_found() -> (StatusCode, ErrorDetail) {
    (
        StatusCode::NOT_FOUND,
        ErrorDetail::new("not_found", "Resource was not found"),
    )
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        tracing::error!(error.msg = %self, error.details = ?self, "controller_error");
        let (status, detail) = match self {
            Self::NotFound => resource_not_found(),
            Self::Unauthorized(_) => (
                StatusCode::UNAUTHORIZED,
                ErrorDetail::new(
                    "unauthorized",
                    "You do not have permission to access this resource",
                ),
            ),
            Self::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                ErrorDetail::new("Bad Request", message),
            ),
            Self::CustomError(status, detail) => (status, detail),
            Self::JsonRejection(rejection) => {
                (rejection.status(), ErrorDetail::with_reason("Bad Request"))
            }
            #[cfg(feature = "sea-orm")]
            Self::Model(crate::model::ModelError::EntityNotFound) => resource_not_found(),
            #[cfg(feature = "sea-orm")]
            Self::Model(crate::model::ModelError::EntityAlreadyExists) => (
                StatusCode::CONFLICT,
                ErrorDetail::new("conflict", "Resource already exists"),
            ),
            // Spelled out rather than `_` so a new variant has to choose.
            Self::Message(_)
            | Self::InternalServerError
            | Self::Axum(_)
            | Self::Http(_)
            | Self::IO(_)
            | Self::JSON(_)
            | Self::Worker(_)
            | Self::Any(_) => internal(),
            #[cfg(feature = "sea-orm")]
            Self::Model(_) | Self::DB(_) => internal(),
            #[cfg(feature = "queue")]
            Self::Sqlx(_) => internal(),
        };
        (status, Json(detail)).into_response()
    }
}

/// Fail with `404`.
///
/// # Errors
///
/// Always returns [`Error::NotFound`].
pub fn not_found<T>() -> Result<T> {
    Err(Error::NotFound)
}

/// Fail with `401`.
///
/// # Errors
///
/// Always returns [`Error::Unauthorized`].
pub fn unauthorized<T>(message: &str) -> Result<T> {
    Err(Error::Unauthorized(message.to_string()))
}

/// Fail with `400`.
///
/// # Errors
///
/// Always returns [`Error::BadRequest`].
pub fn bad_request<T>(message: &str) -> Result<T> {
    Err(Error::BadRequest(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn internal_errors_do_not_leak_their_text() {
        let response = Error::Message("connection to 10.0.0.1 refused".into()).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_text(response).await;
        assert!(!body.contains("10.0.0.1"), "{body}");
        assert!(body.contains("internal_server_error"));
    }

    #[tokio::test]
    async fn unauthorized_text_is_logged_but_not_returned() {
        let response = Error::Unauthorized("token signature invalid".into()).into_response();
        assert!(!body_text(response).await.contains("signature"));
    }

    #[tokio::test]
    async fn bad_request_text_is_returned() {
        let body = body_text(Error::BadRequest("name is required".into()).into_response()).await;
        assert!(body.contains("name is required"));
    }

    #[tokio::test]
    async fn custom_errors_keep_their_status_and_body() {
        let error = Error::CustomError(StatusCode::CONFLICT, ErrorDetail::with_reason("duplicate"));
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(body_text(response).await.contains("duplicate"));
    }

    #[test]
    fn helpers_build_the_matching_variants() {
        assert!(matches!(not_found::<()>(), Err(Error::NotFound)));
        assert!(matches!(
            unauthorized::<()>("x"),
            Err(Error::Unauthorized(_))
        ));
        assert!(matches!(bad_request::<()>("x"), Err(Error::BadRequest(_))));
    }

    #[test]
    fn text_converts_to_a_message() {
        assert!(matches!(Error::from("x"), Error::Message(_)));
        assert!(matches!(Error::from("x".to_string()), Error::Message(_)));
        assert!(matches!(Error::string("x"), Error::Message(_)));
    }

    #[test]
    fn wrap_keeps_the_source_text() {
        let error = Error::wrap(std::io::Error::other("disk full"));
        assert_eq!(error.to_string(), "disk full");
    }

    #[test]
    fn statuses_match_their_variants() {
        assert_eq!(Error::NotFound.into_response().status(), 404);
        assert_eq!(Error::BadRequest("x".into()).into_response().status(), 400);
        assert_eq!(
            Error::Unauthorized("x".into()).into_response().status(),
            401
        );
    }

    #[test]
    fn empty_description_is_omitted() {
        assert_eq!(ErrorDetail::new("x", "").description, None);
    }
}
