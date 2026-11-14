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

use axa_problem::Problem;
use axum::{
    extract::rejection::{JsonRejection, PathRejection, QueryRejection},
    http::{HeaderValue, StatusCode, header},
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
    /// A model-layer failure, from any database.
    #[cfg(feature = "model")]
    #[error(transparent)]
    Model(#[from] crate::model::ModelError),
    /// A `SeaORM` failure, from any database. Becomes `409` or `422` for a violated
    /// constraint, `503` when the database cannot serve for now, and otherwise `500`.
    #[cfg(has_seaorm)]
    #[error(transparent)]
    DB(#[from] sea_orm::DbErr),
    /// A sqlx failure, from any database. Becomes `409` or `422` for a violated
    /// constraint, `503` when the database cannot serve for now, and otherwise `500`. A
    /// service that means something else by one maps it first, as with
    /// `axa_postgres_sqlx::is_unique_violation`.
    #[cfg(has_sqlx)]
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// A Diesel failure, from any database's Diesel crate. Becomes `409` or `422` for a
    /// violated constraint, `503` when the database cannot serve for now, and otherwise
    /// `500`.
    #[cfg(has_diesel)]
    #[error(transparent)]
    Diesel(#[from] DieselError),
    /// A Toasty failure, from any database. Becomes `409` or `422` for a violated
    /// constraint, `503` when the database cannot serve for now, and otherwise `500`.
    #[cfg(has_toasty)]
    #[error(transparent)]
    Toasty(#[from] toasty_crate::Error),
    /// Any other error.
    #[error(transparent)]
    Any(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// The `diesel` and `diesel_async` crates, from whichever Diesel crate is on. They are the
/// same crates for every database.
#[cfg(has_diesel)]
pub(crate) mod diesel_crates {
    #[cfg(all(feature = "mysql-diesel", not(feature = "postgres-diesel")))]
    pub use axa_mysql_diesel::{diesel, diesel_async};
    #[cfg(feature = "postgres-diesel")]
    pub use axa_postgres_diesel::{diesel, diesel_async};
    #[cfg(all(
        feature = "sqlite-diesel",
        not(any(feature = "postgres-diesel", feature = "mysql-diesel"))
    ))]
    pub use axa_sqlite_diesel::{diesel, diesel_async};
}

#[cfg(all(feature = "mysql-toasty", not(feature = "postgres-toasty")))]
pub(crate) use axa_mysql_toasty::toasty as toasty_crate;
/// The `toasty` crate, from whichever Toasty crate is on. It is the same crate for every
/// database.
#[cfg(feature = "postgres-toasty")]
pub(crate) use axa_postgres_toasty::toasty as toasty_crate;

/// A failure from Diesel or from one of Axa's Diesel crates, whichever database it is
/// for, kept as what `axa` needs to answer it.
#[cfg(has_diesel)]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DieselError {
    /// A query failed.
    #[error(transparent)]
    Query(#[from] diesel_crates::diesel::result::Error),
    /// No connection could be had.
    #[error(transparent)]
    Pool(#[from] diesel_crates::diesel_async::pooled_connection::deadpool::PoolError),
    /// Anything else, such as a pool that could not be built or a migration that
    /// failed.
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

/// One `From` for each Diesel crate's error, into [`DieselError`].
#[cfg(has_diesel)]
macro_rules! from_diesel_crate {
    ($($feature:literal => $krate:ident),*) => {$(
        #[cfg(feature = $feature)]
        impl From<$krate::Error> for DieselError {
            fn from(error: $krate::Error) -> Self {
                match error {
                    $krate::Error::Query(error) => Self::Query(error),
                    $krate::Error::Pool(error) => Self::Pool(error),
                    other => Self::Other(Box::new(other)),
                }
            }
        }

        #[cfg(feature = $feature)]
        impl From<$krate::Error> for Error {
            fn from(error: $krate::Error) -> Self {
                Self::Diesel(error.into())
            }
        }
    )*};
}
#[cfg(has_diesel)]
from_diesel_crate!(
    "postgres-diesel" => axa_postgres_diesel,
    "mysql-diesel" => axa_mysql_diesel,
    "sqlite-diesel" => axa_sqlite_diesel
);

/// A Diesel query's own error, as Diesel's functions return it.
#[cfg(has_diesel)]
impl From<diesel_crates::diesel::result::Error> for Error {
    fn from(error: diesel_crates::diesel::result::Error) -> Self {
        Self::Diesel(DieselError::Query(error))
    }
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

/// The longest key a pointer names.
const MAX_POINTER_KEY_BYTES: usize = 64;

/// Where in a JSON body a value did not fit its type, as a JSON Pointer in a URI
/// fragment (RFC 6901), such as `#/items/2/name`, or `None` at the top of the body.
///
/// Only a body that parsed gets one, as only then do the keys come from the type the
/// route reads: in a body that did not parse, the parser may have been walking keys
/// nothing reads. It names keys and indexes, never a value. A key is still the caller's
/// text when the type is a map rather than a struct, so the pointer stops before any
/// key that is not a short name of ASCII letters, digits, `_`, and `-`.
fn json_pointer(rejection: &JsonRejection) -> Option<String> {
    use serde_path_to_error::Segment;
    let JsonRejection::JsonDataError(rejection) = rejection else {
        return None;
    };
    let mut source = std::error::Error::source(rejection);
    let error = loop {
        let error = source?;
        if let Some(found) = error.downcast_ref::<serde_path_to_error::Error<serde_json::Error>>() {
            break found;
        }
        source = error.source();
    };
    let mut pointer = String::from("#");
    for segment in error.path() {
        match segment {
            Segment::Seq { index } => {
                pointer.push('/');
                pointer.push_str(&index.to_string());
            }
            Segment::Map { key } | Segment::Enum { variant: key }
                if key.len() <= MAX_POINTER_KEY_BYTES
                    && !key.is_empty()
                    && key.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                    }) =>
            {
                pointer.push('/');
                pointer.push_str(key);
            }
            _ => break,
        }
    }
    (pointer.len() > 1).then_some(pointer)
}

/// A constraint the caller's write broke, which is the caller's doing and not a fault
/// here: the status, the problem's `code`, and what the caller is told. The database's
/// own text goes nowhere, because it names the row, the column, and the value.
type Broken = (StatusCode, &'static str, &'static str);

#[cfg(any(has_sqlx, has_seaorm, has_diesel, has_toasty))]
const ALREADY_EXISTS: Broken = (StatusCode::CONFLICT, "conflict", "Resource already exists");
#[cfg(any(has_sqlx, has_seaorm, has_diesel, has_toasty))]
const REFERENCE: Broken = (
    StatusCode::CONFLICT,
    "conflict",
    "It refers to a resource that does not exist, or is referred to",
);
#[cfg(any(has_sqlx, has_seaorm, has_diesel, has_toasty))]
const OUT_OF_RANGE: Broken = (
    StatusCode::UNPROCESSABLE_ENTITY,
    "unprocessable_entity",
    "A value is outside what this field allows",
);
#[cfg(any(has_sqlx, has_seaorm, has_diesel, has_toasty))]
const MISSING: Broken = (
    StatusCode::UNPROCESSABLE_ENTITY,
    "unprocessable_entity",
    "A required value is missing",
);

/// The constraint a Postgres error code names, for an error that reaches `axa` with no
/// kind of its own.
#[cfg(feature = "postgres-toasty")]
fn broken_by_postgres_code(code: &str) -> Option<Broken> {
    match code {
        "23505" => Some(ALREADY_EXISTS),
        "23503" => Some(REFERENCE),
        "23514" => Some(OUT_OF_RANGE),
        "23502" => Some(MISSING),
        _ => None,
    }
}

/// The constraint a MySQL error number names, for an error that reaches `axa` with no
/// kind of its own.
#[cfg(feature = "mysql-toasty")]
fn broken_by_mysql_number(number: u16) -> Option<Broken> {
    match number {
        1062 | 1586 => Some(ALREADY_EXISTS),
        1216 | 1217 | 1451 | 1452 => Some(REFERENCE),
        3819 => Some(OUT_OF_RANGE),
        1048 | 1364 => Some(MISSING),
        _ => None,
    }
}

/// How long a client is told to wait before trying again, when the database cannot
/// serve for now.
pub const RETRY_AFTER_SECONDS: u64 = 5;

/// The Postgres codes for a database that cannot serve for now, rather than one that
/// refused this request: a read-only transaction, a lost connection, a server shutting
/// down or not yet up, or too many connections.
#[cfg(any(has_sqlx_postgres, feature = "postgres-toasty"))]
fn postgres_unavailable(code: &str) -> bool {
    code == "25006" || code == "53300" || code.starts_with("08") || code.starts_with("57P")
}

/// The MySQL error numbers for a database that cannot serve for now: too many
/// connections, a server shutting down, a read-only server or transaction, or a
/// connection lost.
#[cfg(any(has_sqlx_mysql, feature = "mysql-toasty"))]
fn mysql_unavailable(number: u16) -> bool {
    matches!(
        number,
        1040 | 1053 | 1290 | 1792 | 1836 | 2002 | 2003 | 2006 | 2013
    )
}

/// The SQLite result codes for a database that cannot serve for now: busy, locked,
/// read-only, or full. Extended codes keep the primary code in their low byte.
#[cfg(has_sqlx_sqlite)]
fn sqlite_unavailable(code: &str) -> bool {
    code.parse::<i32>()
        .is_ok_and(|code| matches!(code & 0xff, 5 | 6 | 8 | 13))
}

/// Whether a sqlx error, from any database, is one that cannot serve for now: a pool
/// with no free connection or one closed, a connection lost, the database's own
/// code for it, or, on the `postgres-sqlx` pool, a statement a migration made stale.
#[cfg(has_sqlx)]
fn sqlx_unavailable(error: &sqlx::Error) -> bool {
    // A statement a migration made stale: the pool retires the connections that cached
    // it, so the next attempt runs on a fresh one, and the caller may try again.
    #[cfg(feature = "postgres-sqlx")]
    if axa_postgres_sqlx::note_stale_plan(error) {
        return true;
    }
    match error {
        sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::Io(_)
        | sqlx::Error::WorkerCrashed => true,
        sqlx::Error::Database(error) => {
            #[cfg(has_sqlx_postgres)]
            if let Some(error) = error.try_downcast_ref::<sqlx::postgres::PgDatabaseError>() {
                return postgres_unavailable(error.code());
            }
            #[cfg(has_sqlx_mysql)]
            if let Some(error) = error.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>() {
                return mysql_unavailable(error.number());
            }
            #[cfg(has_sqlx_sqlite)]
            if error
                .try_downcast_ref::<sqlx::sqlite::SqliteError>()
                .is_some()
            {
                return error.code().is_some_and(|code| sqlite_unavailable(&code));
            }
            false
        }
        _ => false,
    }
}

/// What a sqlx error, from any database, that broke a constraint answers. sqlx reads
/// each database's own codes into these kinds.
#[cfg(has_sqlx)]
fn broken_sqlx(error: &sqlx::Error) -> Option<Broken> {
    match error.as_database_error()?.kind() {
        sqlx::error::ErrorKind::UniqueViolation => Some(ALREADY_EXISTS),
        sqlx::error::ErrorKind::ForeignKeyViolation => Some(REFERENCE),
        sqlx::error::ErrorKind::CheckViolation => Some(OUT_OF_RANGE),
        sqlx::error::ErrorKind::NotNullViolation => Some(MISSING),
        _ => None,
    }
}

/// The sqlx error under a `SeaORM` error, when there is one.
#[cfg(has_seaorm)]
fn seaorm_sqlx(error: &sea_orm::DbErr) -> Option<&sqlx::Error> {
    use sea_orm::{DbErr, RuntimeErr};
    match error {
        DbErr::Exec(RuntimeErr::SqlxError(error)) | DbErr::Query(RuntimeErr::SqlxError(error)) => {
            Some(error.as_ref())
        }
        _ => None,
    }
}

/// Whether a `SeaORM` error is a database that cannot serve for now.
#[cfg(has_seaorm)]
fn seaorm_unavailable(error: &sea_orm::DbErr) -> bool {
    matches!(
        error,
        sea_orm::DbErr::ConnectionAcquire(_) | sea_orm::DbErr::Conn(_)
    ) || seaorm_sqlx(error).is_some_and(sqlx_unavailable)
}

/// Whether a Diesel error is a database that cannot serve for now.
#[cfg(has_diesel)]
fn diesel_unavailable(error: &DieselError) -> bool {
    use diesel_crates::diesel::result::{DatabaseErrorKind, Error as QueryError};
    matches!(
        error,
        DieselError::Pool(_)
            | DieselError::Query(QueryError::DatabaseError(
                DatabaseErrorKind::ReadOnlyTransaction | DatabaseErrorKind::ClosedConnection,
                _
            ))
    )
}

/// What a Diesel error that broke a constraint answers. Diesel reads each database's own
/// codes into these kinds.
#[cfg(has_diesel)]
fn broken_diesel(error: &DieselError) -> Option<Broken> {
    use diesel_crates::diesel::result::{DatabaseErrorKind, Error as QueryError};
    let DieselError::Query(QueryError::DatabaseError(kind, _)) = error else {
        return None;
    };
    match kind {
        DatabaseErrorKind::UniqueViolation => Some(ALREADY_EXISTS),
        DatabaseErrorKind::ForeignKeyViolation => Some(REFERENCE),
        DatabaseErrorKind::CheckViolation => Some(OUT_OF_RANGE),
        DatabaseErrorKind::NotNullViolation => Some(MISSING),
        _ => None,
    }
}

/// Whether a Toasty error is a database that cannot serve for now, from Toasty's own
/// kinds or the database's code.
#[cfg(has_toasty)]
fn toasty_unavailable(error: &toasty_crate::Error) -> bool {
    if error.is_connection_pool() || error.is_connection_lost() || error.is_read_only_transaction()
    {
        return true;
    }
    #[cfg(feature = "postgres-toasty")]
    if axa_postgres_toasty::sql_state(error).is_some_and(postgres_unavailable) {
        return true;
    }
    #[cfg(feature = "mysql-toasty")]
    if axa_mysql_toasty::error_number(error).is_some_and(mysql_unavailable) {
        return true;
    }
    false
}

/// What a Toasty error that broke a constraint answers, from the database's code.
#[cfg(has_toasty)]
fn broken_toasty(error: &toasty_crate::Error) -> Option<Broken> {
    #[cfg(feature = "postgres-toasty")]
    if let Some(broken) = axa_postgres_toasty::sql_state(error).and_then(broken_by_postgres_code) {
        return Some(broken);
    }
    #[cfg(feature = "mysql-toasty")]
    if let Some(broken) = axa_mysql_toasty::error_number(error).and_then(broken_by_mysql_number) {
        return Some(broken);
    }
    let _ = error;
    None
}

/// Whether `error` is the database unable to serve for now, which is the service's
/// trouble, and passes, so it answers `503` with `Retry-After`.
#[cfg(any(has_sqlx, has_seaorm, has_diesel, has_toasty))]
fn unavailable(error: &Error) -> bool {
    match error {
        #[cfg(has_sqlx)]
        Error::Sqlx(error) => sqlx_unavailable(error),
        #[cfg(all(feature = "model", has_sqlx))]
        Error::Model(crate::model::ModelError::Sqlx(error)) => sqlx_unavailable(error),
        #[cfg(has_seaorm)]
        Error::DB(error) | Error::Model(crate::model::ModelError::DbErr(error)) => {
            seaorm_unavailable(error)
        }
        #[cfg(has_diesel)]
        Error::Diesel(error) | Error::Model(crate::model::ModelError::Diesel(error)) => {
            diesel_unavailable(error)
        }
        #[cfg(has_toasty)]
        Error::Toasty(error) | Error::Model(crate::model::ModelError::Toasty(error)) => {
            toasty_unavailable(error)
        }
        _ => false,
    }
}

/// With no database, nothing can be unavailable.
#[cfg(not(any(has_sqlx, has_seaorm, has_diesel, has_toasty)))]
fn unavailable(_error: &Error) -> bool {
    false
}

/// What a write that broke a unique, foreign key, check, or not-null constraint
/// answers. Any other database error is not the caller's, and gets `None`.
#[cfg(any(has_sqlx, has_seaorm, has_diesel, has_toasty))]
fn broken(error: &Error) -> Option<Broken> {
    match error {
        #[cfg(has_sqlx)]
        Error::Sqlx(error) => broken_sqlx(error),
        #[cfg(all(feature = "model", has_sqlx))]
        Error::Model(crate::model::ModelError::Sqlx(error)) => broken_sqlx(error),
        #[cfg(has_seaorm)]
        Error::DB(error) | Error::Model(crate::model::ModelError::DbErr(error)) => {
            broken_sqlx(seaorm_sqlx(error)?)
        }
        #[cfg(has_diesel)]
        Error::Diesel(error) | Error::Model(crate::model::ModelError::Diesel(error)) => {
            broken_diesel(error)
        }
        #[cfg(has_toasty)]
        Error::Toasty(error) | Error::Model(crate::model::ModelError::Toasty(error)) => {
            broken_toasty(error)
        }
        _ => None,
    }
}

/// With no database, nothing can break a constraint.
#[cfg(not(any(has_sqlx, has_seaorm, has_diesel, has_toasty)))]
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
            #[cfg(feature = "model")]
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
        if unavailable(&self) {
            let detail = ErrorDetail::new(
                "unavailable",
                "The service cannot reach its database. Try again shortly",
            );
            let mut response = problem(StatusCode::SERVICE_UNAVAILABLE, detail).into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(RETRY_AFTER_SECONDS));
            return response;
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
                    errors: json_pointer(&rejection)
                        .map(|pointer| serde_json::json!([{ "pointer": pointer }])),
                };
                (status, detail)
            }
            #[cfg(feature = "model")]
            Self::Model(crate::model::ModelError::EntityNotFound) => resource_not_found(),
            #[cfg(feature = "model")]
            Self::Model(crate::model::ModelError::EntityAlreadyExists) => (
                StatusCode::CONFLICT,
                ErrorDetail::new("conflict", "Resource already exists"),
            ),
            #[cfg(feature = "model")]
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
            #[cfg(feature = "model")]
            Self::Model(_) => internal(),
            #[cfg(has_seaorm)]
            Self::DB(_) => internal(),
            #[cfg(has_sqlx)]
            Self::Sqlx(_) => internal(),
            #[cfg(has_diesel)]
            Self::Diesel(_) => internal(),
            #[cfg(has_toasty)]
            Self::Toasty(_) => internal(),
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

    use axa_problem::PROBLEM_JSON;
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

    #[cfg(any(has_sqlx, has_seaorm, has_diesel, has_toasty))]
    /// The answer to a database that cannot serve for now: a `503` problem with the
    /// `code` `unavailable` and a `retry-after`, quoting none of the driver's text.
    async fn assert_unavailable(error: Error) {
        let response = error.into_response();
        assert_eq!(
            response.headers()[axum::http::header::RETRY_AFTER],
            RETRY_AFTER_SECONDS.to_string()
        );
        let (status, body) = problem_of_response(response).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "unavailable");
        assert!(!body.to_string().contains("secret"), "{body}");
    }

    #[cfg(any(has_sqlx, has_seaorm, has_diesel, has_toasty))]
    async fn problem_of_response(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        assert_eq!(response.headers()[CONTENT_TYPE], PROBLEM_JSON);
        let body: Value = serde_json::from_str(&body_text(response).await).unwrap();
        (status, body)
    }

    #[cfg(has_sqlx)]
    #[tokio::test]
    async fn a_sqlx_pool_with_no_connection_is_unavailable() {
        for error in [
            sqlx::Error::PoolTimedOut,
            sqlx::Error::PoolClosed,
            sqlx::Error::Io(std::io::Error::other("secret host refused")),
        ] {
            assert_unavailable(error.into()).await;
        }
        let (status, _) = problem_of(sqlx::Error::RowNotFound.into()).await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "a bug, not an outage"
        );
    }

    #[cfg(has_seaorm)]
    #[tokio::test]
    async fn a_sea_orm_pool_with_no_connection_is_unavailable() {
        let error = sea_orm::DbErr::ConnectionAcquire(sea_orm::error::ConnAcquireErr::Timeout);
        assert_unavailable(error.into()).await;
    }

    #[cfg(has_diesel)]
    #[tokio::test]
    async fn a_read_only_diesel_database_is_unavailable() {
        use super::diesel_crates::diesel::result::{DatabaseErrorKind, Error as QueryError};
        for kind in [
            DatabaseErrorKind::ReadOnlyTransaction,
            DatabaseErrorKind::ClosedConnection,
        ] {
            let error = QueryError::DatabaseError(kind, Box::new("secret".to_string()));
            assert_unavailable(error.into()).await;
        }
    }

    #[cfg(has_toasty)]
    #[tokio::test]
    async fn a_read_only_toasty_database_is_unavailable() {
        assert_unavailable(toasty_crate::Error::read_only_transaction("secret").into()).await;
    }

    #[cfg(has_sqlx_sqlite)]
    #[test]
    fn sqlite_busy_locked_read_only_and_full_are_unavailable() {
        // SQLITE_BUSY, SQLITE_LOCKED, SQLITE_READONLY, SQLITE_FULL, and two extended
        // codes: SQLITE_BUSY_SNAPSHOT and SQLITE_READONLY_DBMOVED.
        for code in ["5", "6", "8", "13", "517", "1032"] {
            assert!(sqlite_unavailable(code), "{code}");
        }
        // SQLITE_CONSTRAINT and SQLITE_ERROR are not.
        for code in ["19", "1", "2067", "not a number"] {
            assert!(!sqlite_unavailable(code), "{code}");
        }
    }

    #[cfg(any(has_sqlx_mysql, feature = "mysql-toasty"))]
    #[test]
    fn mysql_read_only_and_lost_connections_are_unavailable() {
        for number in [1040, 1053, 1290, 1792, 1836, 2006, 2013] {
            assert!(mysql_unavailable(number), "{number}");
        }
        // A duplicate and a syntax error are not.
        for number in [1062, 1064] {
            assert!(!mysql_unavailable(number), "{number}");
        }
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

    /// The problem for a JSON `body` rejected on its way into `T`.
    async fn json_problem<T: serde::de::DeserializeOwned>(body: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .header(CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let rejection = Json::<T>::from_request(request, &())
            .await
            .map(|_| ())
            .unwrap_err();
        problem_of(rejection.into()).await
    }

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code, reason = "only deserialized")]
    struct Order {
        items: Vec<Item>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code, reason = "only deserialized")]
    struct Item {
        name: String,
        count: u8,
    }

    #[tokio::test]
    async fn a_value_that_does_not_fit_is_pointed_at_and_not_quoted() {
        let body = r#"{"items":[{"name":"a","count":1},{"name":"secret","count":"secret"}]}"#;
        let (status, problem) = json_problem::<Order>(body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(problem["errors"], json!([{ "pointer": "#/items/1/count" }]));
        assert!(!problem.to_string().contains("secret"), "{problem}");

        let (_, problem) = json_problem::<Order>(r#"{"items":[{"name":"a"}]}"#).await;
        assert_eq!(problem["errors"], json!([{ "pointer": "#/items/0" }]));
        let (_, problem) = json_problem::<Order>("[]").await;
        assert!(problem.get("errors").is_none(), "{problem}");
    }

    #[tokio::test]
    async fn a_pointer_stops_before_a_key_that_is_not_a_short_name() {
        type Counts = std::collections::HashMap<String, std::collections::HashMap<String, u8>>;
        let (_, problem) = json_problem::<Counts>(r#"{"a":{"hunter 2":"x"}}"#).await;
        assert_eq!(problem["errors"], json!([{ "pointer": "#/a" }]));
        let long = "k".repeat(65);
        let (_, problem) = json_problem::<Counts>(&format!(r#"{{"{long}":{{"b":"x"}}}}"#)).await;
        assert!(problem.get("errors").is_none(), "{problem}");
        assert!(!problem.to_string().contains(&long), "{problem}");
    }

    #[cfg(feature = "model")]
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

    #[cfg(feature = "model")]
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

    #[cfg(has_sqlx)]
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
