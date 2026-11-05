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
    async fn json_serializes_with_a_json_content_type() {
        let response = json(serde_json::json!({ "a": 1 })).unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(body_text(response).await, r#"{"a":1}"#);
    }

    #[tokio::test]
    async fn empty_json_is_an_empty_object() {
        assert_eq!(body_text(empty_json().unwrap()).await, "{}");
    }

    #[tokio::test]
    async fn empty_has_no_body() {
        let response = empty().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_text(response).await, "");
    }

    #[tokio::test]
    async fn text_and_html_set_their_content_types() {
        let response = text("hi").unwrap();
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/plain")
        );
        let response = html("<p>hi</p>").unwrap();
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
    }

    #[test]
    fn redirect_points_at_the_target() {
        let response = redirect("/next").unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/next");
    }
}
