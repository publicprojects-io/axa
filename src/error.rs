//! The error type handlers return, and its HTTP mapping.
//!
//! Every error answers as an [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) problem,
//! with the media type `application/problem+json`. The machine-readable reason is the
//! `code` extension member, and field errors are the `errors` one. Successes stay bare
//! JSON.
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

use axa_problem_details_for_http_apis::Problem;
use axum::{
    extract::rejection::{JsonRejection, PathRejection, QueryRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Result alias using [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What an error says beyond its status, sent as members of the problem.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    /// Short machine-readable reason, sent as the `code` extension member.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Human-readable explanation of this occurrence, sent as `detail`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Structured detail, such as per-field validation errors, sent as the
    /// `errors` extension member.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<serde_json::Value>,
}

impl ErrorDetail {
    /// Build a detail from a reason and an explanation. An empty explanation
    /// is omitted from the body.
    #[must_use]
    pub fn new<A: Into<String>, B: Into<String> + AsRef<str>>(code: A, detail: B) -> Self {
        let detail = (!detail.as_ref().is_empty()).then(|| detail.into());
        Self {
            code: Some(code.into()),
            detail,
            errors: None,
        }
    }

    /// Build a detail carrying only a reason.
    #[must_use]
    pub fn with_reason<A: Into<String>>(code: A) -> Self {
        Self {
            code: Some(code.into()),
            detail: None,
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
    /// A rejected JSON body. Becomes its status, with a description that quotes
    /// nothing the caller sent.
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
    /// A database failure. Becomes `409` for a violated unique or foreign key
    /// constraint, and otherwise a generic `500`.
    #[cfg(feature = "sea-orm")]
    #[error(transparent)]
    DB(#[from] sea_orm::DbErr),
    /// A sqlx database failure. Becomes `409` for a violated unique or foreign key
    /// constraint, and otherwise a generic `500`. A service that means something
    /// else by one maps it first, as with `axa_sqlx::is_unique_violation`.
    #[cfg(any(feature = "idempotency", feature = "queue", feature = "sqlx"))]
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

/// A rejected value in the URL. The rejection's own text quotes the value, so it is
/// not used.
impl From<PathRejection> for Error {
    fn from(rejection: PathRejection) -> Self {
        // A `Path` that does not fit its route is a bug here, and quotes no value.
        if rejection.status().is_server_error() {
            return Self::Message(rejection.body_text());
        }
        Self::CustomError(
            rejection.status(),
            ErrorDetail::new("bad_request", "A value in the URL is not valid"),
        )
    }
}

/// A rejected query string. The rejection's own text quotes the value, so it is not
/// used.
impl From<QueryRejection> for Error {
    fn from(rejection: QueryRejection) -> Self {
        Self::CustomError(
            rejection.status(),
            ErrorDetail::new("bad_request", "A query parameter is not valid"),
        )
    }
}

/// A bad limit or cursor, which is the caller's to fix: a `422` whose `detail`
/// quotes nothing the caller sent.
#[cfg(feature = "cursor-pagination")]
impl From<axa_cursor_pagination::PageError> for Error {
    fn from(error: axa_cursor_pagination::PageError) -> Self {
        let status = error.status();
        Self::CustomError(
            status,
            ErrorDetail {
                code: reason_code(status),
                detail: Some(error.to_string()),
                errors: None,
            },
        )
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

/// `detail` as members of a problem for `status`.
fn problem(status: StatusCode, detail: ErrorDetail) -> Problem {
    let mut problem = Problem::new(status);
    problem.detail = detail.detail;
    if let Some(code) = detail.code {
        problem = problem.with_extension("code", code.into());
    }
    if let Some(errors) = detail.errors {
        problem = problem.with_extension("errors", errors);
    }
    problem
}

/// The response to a handler that panicked: the same as any internal failure.
pub(crate) fn internal_response() -> Response {
    let (status, detail) = internal();
    problem(status, detail).into_response()
}

/// The response to a request that ran past its time limit.
pub(crate) fn timeout_response() -> Response {
    problem(
        StatusCode::REQUEST_TIMEOUT,
        ErrorDetail::with_reason("request_timeout"),
    )
    .into_response()
}

/// The problem for a refusal that came with nothing to say, which reads as if `axa`
/// had written it: a `404` is [`Error::NotFound`]'s, and any other has the status's reason
/// as its `code`.
pub(crate) fn refusal_response(status: StatusCode) -> Response {
    if status == StatusCode::NOT_FOUND {
        let (status, detail) = resource_not_found();
        return problem(status, detail).into_response();
    }
    let detail = ErrorDetail {
        code: reason_code(status),
        detail: None,
        errors: None,
    };
    problem(status, detail).into_response()
}

/// A status's reason, as a `code`.
fn reason_code(status: StatusCode) -> Option<String> {
    let reason = status.canonical_reason()?.to_ascii_lowercase();
    Some(reason.replace(' ', "_").replace('\'', ""))
}

/// The body every internal failure shares. It carries no `detail`, as the title
/// says all a client may be told.
fn internal() -> (StatusCode, ErrorDetail) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorDetail::with_reason("internal_server_error"),
    )
}

/// The body every missing resource shares.
fn resource_not_found() -> (StatusCode, ErrorDetail) {
    (
        StatusCode::NOT_FOUND,
        ErrorDetail::new("not_found", "Resource was not found"),
    )
}

/// What to tell a caller about a rejected JSON body. The rejection's own text can quote
/// the body, so it is not used.
fn json_rejection_text(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "The request body is not valid JSON",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "Send the request body as application/json",
        StatusCode::PAYLOAD_TOO_LARGE => "The request body is too large",
        _ => "The request body does not fit this route",
    }
}

/// A constraint the caller's write broke, which is the caller's doing and not a fault
/// here: the status, the problem's `code`, and what the caller is told. The database's
/// own text goes nowhere, because it names the row, the column, and the value.
type Broken = (StatusCode, &'static str, &'static str);

#[cfg(any(
    feature = "idempotency",
    feature = "queue",
    feature = "sqlx",
    feature = "sea-orm"
))]
const ALREADY_EXISTS: Broken = (StatusCode::CONFLICT, "conflict", "Resource already exists");
#[cfg(any(
    feature = "idempotency",
    feature = "queue",
    feature = "sqlx",
    feature = "sea-orm"
))]
const REFERENCE: Broken = (
    StatusCode::CONFLICT,
    "conflict",
    "It refers to a resource that does not exist, or is referred to",
);
#[cfg(any(
    feature = "idempotency",
    feature = "queue",
    feature = "sqlx",
    feature = "sea-orm"
))]
const OUT_OF_RANGE: Broken = (
    StatusCode::UNPROCESSABLE_ENTITY,
    "unprocessable_entity",
    "A value is outside what this field allows",
);
#[cfg(any(
    feature = "idempotency",
    feature = "queue",
    feature = "sqlx",
    feature = "sea-orm"
))]
const MISSING: Broken = (
    StatusCode::UNPROCESSABLE_ENTITY,
    "unprocessable_entity",
    "A required value is missing",
);

/// The constraint a Postgres error code names, for an error that reaches `axa` with no
/// kind of its own.
#[cfg(feature = "sea-orm")]
fn broken_by_code(code: &str) -> Option<Broken> {
    match code {
        "23505" => Some(ALREADY_EXISTS),
        "23503" => Some(REFERENCE),
        "23514" => Some(OUT_OF_RANGE),
        "23502" => Some(MISSING),
        _ => None,
    }
}

/// What a write that broke a unique, foreign key, check, or not-null constraint
/// answers. Any other database error is not the caller's, and gets `None`.
#[cfg(any(
    feature = "idempotency",
    feature = "queue",
    feature = "sqlx",
    feature = "sea-orm"
))]
fn broken(error: &Error) -> Option<Broken> {
    match error {
        #[cfg(any(feature = "idempotency", feature = "queue", feature = "sqlx"))]
        Error::Sqlx(error) => match error.as_database_error()?.kind() {
            sqlx::error::ErrorKind::UniqueViolation => Some(ALREADY_EXISTS),
            sqlx::error::ErrorKind::ForeignKeyViolation => Some(REFERENCE),
            sqlx::error::ErrorKind::CheckViolation => Some(OUT_OF_RANGE),
            sqlx::error::ErrorKind::NotNullViolation => Some(MISSING),
            _ => None,
        },
        #[cfg(feature = "sea-orm")]
        Error::DB(error) | Error::Model(crate::model::ModelError::DbErr(error)) => {
            // SeaORM names only unique and foreign key violations, so the code is read
            // from the driver's error, as SeaORM's own documentation advises.
            use sea_orm::{DbErr, RuntimeErr};
            let (DbErr::Exec(RuntimeErr::SqlxError(error))
            | DbErr::Query(RuntimeErr::SqlxError(error))) = error
            else {
                return None;
            };
            broken_by_code(&error.as_database_error()?.code()?)
        }
        _ => None,
    }
}

/// With no database, nothing can break a constraint.
#[cfg(not(any(
    feature = "idempotency",
    feature = "queue",
    feature = "sqlx",
    feature = "sea-orm"
)))]
fn broken(_error: &Error) -> Option<Broken> {
    None
}

impl Error {
    /// Log what a service needs to act on. A client's mistake is not an incident,
    /// and its text can quote what the client sent, which may be personal data, so
    /// it gets one debug line and no text.
    fn log(&self) {
        let client_mistake = match self {
            Self::NotFound | Self::BadRequest(_) | Self::JsonRejection(_) => true,
            Self::CustomError(status, _) => status.is_client_error(),
            #[cfg(feature = "sea-orm")]
            Self::Model(
                crate::model::ModelError::EntityNotFound
                | crate::model::ModelError::EntityAlreadyExists
                | crate::model::ModelError::Validation(_),
            ) => true,
            _ => broken(self).is_some(),
        };
        if client_mistake {
            tracing::debug!("controller_client_error");
        } else if let Self::Unauthorized(reason) = self {
            tracing::warn!(error.msg = %reason, "controller_unauthorized");
        } else {
            // Never the error's `Debug` text: a database error's holds the row's values.
            tracing::error!(error.msg = %self, "controller_error");
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        self.log();
        if let Some((status, code, description)) = broken(&self) {
            return problem(status, ErrorDetail::new(code, description)).into_response();
        }
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
                ErrorDetail::new("bad_request", message),
            ),
            Self::CustomError(status, detail) => (status, detail),
            Self::JsonRejection(rejection) => {
                let status = rejection.status();
                let detail = ErrorDetail {
                    code: reason_code(status),
                    detail: Some(json_rejection_text(status).to_string()),
                    errors: None,
                };
                (status, detail)
            }
            #[cfg(feature = "sea-orm")]
            Self::Model(crate::model::ModelError::EntityNotFound) => resource_not_found(),
            #[cfg(feature = "sea-orm")]
            Self::Model(crate::model::ModelError::EntityAlreadyExists) => (
                StatusCode::CONFLICT,
                ErrorDetail::new("conflict", "Resource already exists"),
            ),
            #[cfg(feature = "sea-orm")]
            Self::Model(crate::model::ModelError::Validation(errors)) => (
                StatusCode::BAD_REQUEST,
                ErrorDetail {
                    code: Some("validation".to_string()),
                    detail: None,
                    errors: Some(serde_json::to_value(&errors.errors).unwrap_or_default()),
                },
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
            #[cfg(any(feature = "idempotency", feature = "queue", feature = "sqlx"))]
            Self::Sqlx(_) => internal(),
        };
        problem(status, detail).into_response()
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

    use axa_problem_details_for_http_apis::PROBLEM_JSON;
    use axum::{
        Json,
        extract::{FromRequest, Request},
        http::header::CONTENT_TYPE,
    };
    use serde_json::{Value, json};

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// The body of a response, which must be a problem and agree with its status.
    async fn problem_of(error: Error) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        assert_eq!(response.headers()[CONTENT_TYPE], PROBLEM_JSON);
        let body: Value = serde_json::from_str(&body_text(response).await).unwrap();
        assert_eq!(body["status"], status.as_u16());
        (status, body)
    }

    #[tokio::test]
    async fn internal_errors_do_not_leak_their_text() {
        let error = Error::Message("connection to 10.0.0.1 refused".into());
        let (status, body) = problem_of(error).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body,
            json!({
                "type": "about:blank",
                "title": "Internal Server Error",
                "status": 500,
                "code": "internal_server_error",
            })
        );
    }

    #[tokio::test]
    async fn unauthorized_text_is_logged_but_not_returned() {
        let error = Error::Unauthorized("token signature invalid".into());
        let (status, body) = problem_of(error).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthorized");
        assert!(!body.to_string().contains("signature"), "{body}");
    }

    #[tokio::test]
    async fn bad_request_text_is_returned() {
        let (status, body) = problem_of(Error::BadRequest("name is required".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "bad_request");
        assert_eq!(body["detail"], "name is required");
    }

    #[tokio::test]
    async fn custom_errors_keep_their_status_and_body() {
        let detail = ErrorDetail::new("duplicate", "That name is taken");
        let (status, body) = problem_of(Error::CustomError(StatusCode::CONFLICT, detail)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["title"], "Conflict");
        assert_eq!(body["code"], "duplicate");
        assert_eq!(body["detail"], "That name is taken");
    }

    #[tokio::test]
    async fn a_status_with_no_standard_reason_is_still_a_problem() {
        let status = StatusCode::from_u16(599).unwrap();
        let (kept, body) =
            problem_of(Error::CustomError(status, ErrorDetail::with_reason("odd"))).await;
        assert_eq!(kept, status);
        assert_eq!(body["title"], "Error");
    }

    /// Pinned as text, as this is what a client parses and what the upgrade guide shows.
    #[tokio::test]
    async fn the_bodies_are_exactly_these() {
        let bad = Error::BadRequest("name is required".into()).into_response();
        assert_eq!(
            body_text(bad).await,
            r#"{"type":"about:blank","title":"Bad Request","status":400,"detail":"name is required","code":"bad_request"}"#
        );
        assert_eq!(
            body_text(Error::NotFound.into_response()).await,
            r#"{"type":"about:blank","title":"Not Found","status":404,"detail":"Resource was not found","code":"not_found"}"#
        );
        assert_eq!(
            body_text(Error::InternalServerError.into_response()).await,
            r#"{"type":"about:blank","title":"Internal Server Error","status":500,"code":"internal_server_error"}"#
        );
    }

    #[tokio::test]
    async fn field_errors_are_an_extension_member() {
        let detail = ErrorDetail {
            code: Some("validation".to_string()),
            detail: None,
            errors: Some(json!({ "name": ["is required"] })),
        };
        let error = Error::CustomError(StatusCode::UNPROCESSABLE_ENTITY, detail);
        let (status, body) = problem_of(error).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["errors"], json!({ "name": ["is required"] }));
    }

    #[tokio::test]
    async fn a_rejected_json_body_keeps_its_status_and_hides_its_text() {
        let request = Request::builder()
            .header(CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from("{\"secret\": oops"))
            .unwrap();
        let rejection = Json::<Value>::from_request(request, &())
            .await
            .map(|_| ())
            .unwrap_err();
        let (status, body) = problem_of(rejection.into()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "bad_request");
        assert_eq!(body["detail"], "The request body is not valid JSON");
        assert!(!body.to_string().contains("secret"), "{body}");
    }

    #[cfg(feature = "sea-orm")]
    #[tokio::test]
    async fn a_validation_failure_keeps_its_field_errors() {
        use crate::{
            model::ModelError,
            validation::{ModelValidationErrors, ValidationError},
        };

        let failure = ValidationError {
            code: "length".to_string(),
            message: Some("is required".to_string()),
            params: std::collections::HashMap::new(),
        };
        let error = ModelError::Validation(ModelValidationErrors {
            errors: [("name".to_string(), vec![failure])].into(),
        });
        let response = Error::Model(error).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[CONTENT_TYPE], PROBLEM_JSON);
        let text = body_text(response).await;
        assert_eq!(
            text,
            r#"{"type":"about:blank","title":"Bad Request","status":400,"code":"validation","errors":{"name":[{"code":"length","message":"is required"}]}}"#
        );
    }

    #[cfg(feature = "sea-orm")]
    #[tokio::test]
    async fn model_errors_answer_as_problems() {
        use crate::model::ModelError;

        let (status, body) = problem_of(ModelError::EntityNotFound.into()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
        let (status, body) = problem_of(ModelError::EntityAlreadyExists.into()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "conflict");
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

    #[cfg(any(feature = "idempotency", feature = "queue", feature = "sqlx"))]
    #[tokio::test]
    async fn a_sqlx_error_is_a_generic_500() {
        let error: Error = sqlx::Error::Protocol("secret detail".to_string()).into();
        let (status, body) = problem_of(error).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["code"], "internal_server_error");
        assert!(!body.to_string().contains("secret"), "{body}");
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
        assert_eq!(ErrorDetail::new("x", "").detail, None);
    }

    #[cfg(feature = "cursor-pagination")]
    #[tokio::test]
    async fn a_bad_page_is_a_422_problem_that_quotes_nothing() {
        use axa_cursor_pagination::{CursorKey, CursorLimits, CursorPage};
        let key = CursorKey::new(&[3; 32]).unwrap();
        let limits = CursorLimits {
            default_limit: 10,
            max_limit: 20,
        };
        for uri in ["/things?limit=secret", "/things?cursor=secret"] {
            let error = CursorPage::new(&key, limits, &uri.parse().unwrap()).unwrap_err();
            let response = Error::from(error).into_response();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(
                response.headers()["content-type"],
                "application/problem+json"
            );
            let body = body_text(response).await;
            assert!(body.contains("unprocessable_entity"), "{body}");
            assert!(!body.contains("secret"), "{body}");
        }
    }
}
