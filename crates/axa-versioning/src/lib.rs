//! Header-based API version negotiation.
//!
//! URLs carry no version. A client asks for one with an `Accepts-version`
//! header, and every `/api` response says which version served it.
//!
//! | Request | Result |
//! |---|---|
//! | No header, or an empty one | The current version |
//! | A supported version, or its bare major such as `1` | That version |
//! | Anything else | `406 Not Acceptable`, naming what is supported |
//!
//! ```
//! use axa_versioning::resolve_version;
//!
//! assert_eq!(resolve_version(None), Ok("1.0"));
//! assert_eq!(resolve_version(Some("1")), Ok("1.0"));
//! assert!(resolve_version(Some("9.9")).is_err());
//! ```

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// The request and response header.
pub const VERSION_HEADER: &str = "accepts-version";

/// The version served when a request names none.
pub const CURRENT_API_VERSION: &str = "1.0";

/// Every version served.
pub const SUPPORTED_API_VERSIONS: &[&str] = &["1.0"];

/// Resolve a requested version to a supported one.
///
/// # Errors
///
/// Returns the trimmed, lowercased request when no supported version matches.
pub fn resolve_version(requested: Option<&str>) -> Result<&'static str, String> {
    let Some(raw) = requested else {
        return Ok(CURRENT_API_VERSION);
    };
    let want = raw.trim().to_ascii_lowercase();
    if want.is_empty() {
        return Ok(CURRENT_API_VERSION);
    }
    SUPPORTED_API_VERSIONS
        .iter()
        .copied()
        .find(|&version| {
            version.eq_ignore_ascii_case(&want)
                || version
                    .split_once('.')
                    .is_some_and(|(major, _)| major == want)
        })
        .ok_or(want)
}

/// Whether `path` is under the versioned `/api` surface. FHIR, health, and
/// documentation routes are not.
#[must_use]
pub fn is_versioned_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// Middleware that negotiates the version for `/api` paths.
pub async fn require_version_mw(req: Request, next: Next) -> Response {
    if !is_versioned_path(req.uri().path()) {
        return next.run(req).await;
    }
    let requested = req
        .headers()
        .get(VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    match resolve_version(requested.as_deref()) {
        Ok(resolved) => {
            let mut response = next.run(req).await;
            response.headers_mut().insert(
                HeaderName::from_static(VERSION_HEADER),
                HeaderValue::from_static(resolved),
            );
            response
        }
        Err(bad) => (
            StatusCode::NOT_ACCEPTABLE,
            axum::Json(serde_json::json!({
                "error": "unsupported_api_version",
                "requested": bad,
                "supported": SUPPORTED_API_VERSIONS,
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http, middleware::from_fn, routing::get};
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/api/things", get(|| async { "ok" }))
            .route("/fhir/metadata", get(|| async { "ok" }))
            .layer(from_fn(require_version_mw))
    }

    async fn call(path: &str, version: Option<&str>) -> Response {
        let mut request = http::Request::builder().uri(path);
        if let Some(version) = version {
            request = request.header(VERSION_HEADER, version);
        }
        app()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[test]
    fn no_header_means_the_current_version() {
        assert_eq!(resolve_version(None), Ok(CURRENT_API_VERSION));
        assert_eq!(resolve_version(Some("   ")), Ok(CURRENT_API_VERSION));
    }

    #[test]
    fn a_bare_major_is_an_alias() {
        assert_eq!(resolve_version(Some(" 1 ")), Ok("1.0"));
    }

    #[test]
    fn an_unsupported_version_is_returned_for_the_error() {
        assert_eq!(resolve_version(Some(" 2.0 ")), Err("2.0".to_string()));
    }

    #[test]
    fn only_api_paths_are_versioned() {
        assert!(is_versioned_path("/api"));
        assert!(is_versioned_path("/api/things"));
        assert!(!is_versioned_path("/apiary"));
        assert!(!is_versioned_path("/fhir/metadata"));
    }

    #[tokio::test]
    async fn the_resolved_version_is_echoed() {
        let response = call("/api/things", None).await;
        assert_eq!(response.headers()[VERSION_HEADER], "1.0");
    }

    #[tokio::test]
    async fn an_unsupported_version_is_a_406() {
        let response = call("/api/things", Some("9.9")).await;
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    }

    #[tokio::test]
    async fn paths_outside_api_ignore_the_header() {
        let response = call("/fhir/metadata", Some("9.9")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(VERSION_HEADER));
    }
}
