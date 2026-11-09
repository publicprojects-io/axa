//! [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) problem details.
//!
//! An error response is a JSON object with the media type
//! `application/problem+json`:
//!
//! ```
//! use axa_problem_details_for_http_apis::Problem;
//! use axum::http::StatusCode;
//!
//! let problem = Problem::new(StatusCode::NOT_FOUND)
//!     .with_detail("No folder has id 42.")
//!     .with_instance("/api/folders/42");
//! assert_eq!(problem.title, "Not Found");
//! assert_eq!(problem.status, 404);
//! ```

use std::collections::BTreeMap;

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

/// The media type of a problem response.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// The default `type`, meaning the status code says everything needed.
pub const ABOUT_BLANK: &str = "about:blank";

/// Member names RFC 9457 defines, which an extension must not reuse.
const RESERVED: [&str; 5] = ["type", "title", "status", "detail", "instance"];

/// One problem, as RFC 9457 defines it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    /// A URI identifying the problem type.
    #[serde(rename = "type")]
    pub problem_type: String,
    /// A short summary that does not change between occurrences.
    pub title: String,
    /// The HTTP status code.
    pub status: u16,
    /// An explanation specific to this occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// A URI identifying this occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// Extra members, written at the top level of the object.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl Problem {
    /// A problem for `status`, titled with the status's standard reason.
    #[must_use]
    pub fn new(status: StatusCode) -> Self {
        Self {
            problem_type: ABOUT_BLANK.to_string(),
            title: status.canonical_reason().unwrap_or("Error").to_string(),
            status: status.as_u16(),
            detail: None,
            instance: None,
            extensions: BTreeMap::new(),
        }
    }

    /// Set the problem type URI.
    #[must_use]
    pub fn with_type(mut self, problem_type: impl Into<String>) -> Self {
        self.problem_type = problem_type.into();
        self
    }

    /// Set the title.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the occurrence-specific detail. Never put internal failure text
    /// here: it is sent to the client.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Set the occurrence URI.
    #[must_use]
    pub fn with_instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    /// Add an extension member. A name RFC 9457 reserves is ignored, because
    /// flattening it would corrupt the standard member.
    #[must_use]
    pub fn with_extension(mut self, name: &str, value: serde_json::Value) -> Self {
        if !RESERVED.contains(&name) {
            self.extensions.insert(name.to_string(), value);
        }
        self
    }

    /// The status as a [`StatusCode`], or `500` if it is not a valid one.
    #[must_use]
    pub fn status_code(&self) -> StatusCode {
        StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let mut response = (status, Json(self)).into_response();
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_follow_the_rfc() {
        let value = serde_json::to_value(Problem::new(StatusCode::NOT_FOUND)).unwrap();
        assert_eq!(
            value,
            json!({ "type": "about:blank", "title": "Not Found", "status": 404 })
        );
    }

    #[test]
    fn optional_members_and_extensions_are_top_level() {
        let problem = Problem::new(StatusCode::CONFLICT)
            .with_type("https://example.org/problems/duplicate")
            .with_title("Duplicate record")
            .with_detail("Two records match.")
            .with_instance("/api/items/1")
            .with_extension("candidates", json!([1, 2]));
        let value = serde_json::to_value(problem).unwrap();
        assert_eq!(value["type"], "https://example.org/problems/duplicate");
        assert_eq!(value["title"], "Duplicate record");
        assert_eq!(value["detail"], "Two records match.");
        assert_eq!(value["instance"], "/api/items/1");
        assert_eq!(value["candidates"], json!([1, 2]));
    }

    #[test]
    fn reserved_names_cannot_be_overwritten_by_extensions() {
        let problem = Problem::new(StatusCode::NOT_FOUND).with_extension("status", json!(200));
        let value = serde_json::to_value(problem).unwrap();
        assert_eq!(value["status"], 404);
    }

    #[test]
    fn unknown_members_round_trip_as_extensions() {
        let problem: Problem = serde_json::from_value(json!({
            "type": "about:blank", "title": "x", "status": 400, "balance": 30
        }))
        .unwrap();
        assert_eq!(problem.extensions["balance"], 30);
    }

    #[test]
    fn an_invalid_status_falls_back_to_500() {
        let mut problem = Problem::new(StatusCode::OK);
        problem.status = 9999;
        assert_eq!(problem.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn responses_use_the_problem_media_type() {
        let response = Problem::new(StatusCode::IM_A_TEAPOT).into_response();
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(response.headers()[CONTENT_TYPE], PROBLEM_JSON);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["status"], 418);
    }
}
