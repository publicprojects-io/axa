//! Small response helpers for handlers that return [`Result<Response>`].

use axum::{
    Json,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Serialize;

use crate::Result;

/// A `200` with a JSON body.
///
/// # Errors
///
/// Infallible in practice; fallible to match how handlers compose it.
pub fn json<T: Serialize>(value: T) -> Result<Response> {
    Ok(Json(value).into_response())
}

/// A `200` with an empty JSON object.
///
/// # Errors
///
/// Infallible in practice.
pub fn empty_json() -> Result<Response> {
    json(serde_json::json!({}))
}

/// A `200` with no body.
///
/// # Errors
///
/// Infallible in practice.
pub fn empty() -> Result<Response> {
    Ok(StatusCode::OK.into_response())
}

/// A `200` with a plain-text body.
///
/// # Errors
///
/// Infallible in practice.
pub fn text(body: &str) -> Result<Response> {
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body.to_string(),
    )
        .into_response())
}

/// A `200` with an HTML body.
///
/// # Errors
///
/// Infallible in practice.
pub fn html(body: &str) -> Result<Response> {
    Ok(Html(body.to_string()).into_response())
}

/// A `303` redirect.
///
/// # Errors
///
/// Infallible in practice.
pub fn redirect(to: &str) -> Result<Response> {
    Ok(Redirect::to(to).into_response())
}
