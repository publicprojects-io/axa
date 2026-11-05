//! The standard middleware stack.
//!
//! From the outside in: request id, tracing, panic catching, security
//! headers, CORS, timeout, compression, body limit.

use std::time::Duration;

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{Next, from_fn},
    response::Response,
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use uuid::Uuid;

use crate::{Error, Result, config::Http};

/// The request id header.
pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

const MAX_REQUEST_ID_LEN: usize = 255;

/// The id assigned to a request, available as a request extension.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Keep a caller's id only after removing unexpected characters, so it is
/// safe in logs, or mint one when nothing usable remains.
fn sanitize_request_id(value: Option<&HeaderValue>) -> String {
    value
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '@'))
                .take(MAX_REQUEST_ID_LEN)
                .collect::<String>()
        })
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

async fn request_id(mut request: Request, next: Next) -> Response {
    let id = sanitize_request_id(request.headers().get(&X_REQUEST_ID));
    request.extensions_mut().insert(RequestId(id.clone()));
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(X_REQUEST_ID, value);
    }
    response
}

/// The CORS layer for `origins`: none for an empty list, any origin for
/// `["*"]`, otherwise exactly the listed origins.
///
/// # Errors
///
/// Fails when an origin is not a valid header value.
pub fn cors(origins: &[String]) -> Result<Option<CorsLayer>> {
    if origins.is_empty() {
        return Ok(None);
    }
    if origins.iter().any(|origin| origin == "*") {
        return Ok(Some(CorsLayer::permissive()));
    }
    let origins = origins
        .iter()
        .map(|origin| {
            HeaderValue::from_str(origin)
                .map_err(|_| Error::string(&format!("invalid CORS origin: {origin}")))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods(AllowMethods::list([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ]))
            .allow_headers(AllowHeaders::mirror_request()),
    ))
}

fn set_default_header(
    name: HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

/// Apply the standard stack to `router`.
///
/// # Errors
///
/// Fails when a CORS origin is invalid.
pub fn apply(router: Router, config: &Http) -> Result<Router> {
    let mut router = router.layer(DefaultBodyLimit::max(config.body_limit));
    if config.compression {
        router = router.layer(CompressionLayer::new());
    }
    if config.timeout_seconds > 0 {
        router = router.layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(config.timeout_seconds),
        ));
    }
    if let Some(layer) = cors(&config.cors_origins)? {
        router = router.layer(layer);
    }
    if config.hsts {
        router = router.layer(set_default_header(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000; includeSubDomains",
        ));
    }
    Ok(router
        .layer(set_default_header(
            header::X_CONTENT_TYPE_OPTIONS,
            "nosniff",
        ))
        .layer(set_default_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(set_default_header(header::REFERRER_POLICY, "no-referrer"))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(from_fn(request_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http, routing::get};
    use tower::ServiceExt;

    fn config() -> Http {
        Http {
            body_limit: 16,
            compression: false,
            timeout_seconds: 0,
            cors_origins: Vec::new(),
            hsts: true,
        }
    }

    async fn panics() -> &'static str {
        panic!("boom")
    }

    fn app() -> Router {
        let router = Router::new()
            .route("/", get(|| async { "ok" }))
            .route("/panic", get(panics))
            .route(
                "/echo",
                axum::routing::post(|body: String| async move { body }),
            );
        apply(router, &config()).unwrap()
    }

    fn get_request(path: &str) -> http::Request<Body> {
        http::Request::builder()
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn request_id_keeps_safe_text_and_strips_the_rest() {
        let value = HeaderValue::from_static("abc-123_x@y;\tdrop");
        assert_eq!(sanitize_request_id(Some(&value)), "abc-123_x@ydrop");
    }

    #[test]
    fn request_id_is_minted_when_nothing_usable_remains() {
        let value = HeaderValue::from_static(";;;");
        assert_eq!(sanitize_request_id(Some(&value)).len(), 36);
        assert_eq!(sanitize_request_id(None).len(), 36);
    }

    #[tokio::test]
    async fn responses_carry_request_id_and_security_headers() {
        let response = app().oneshot(get_request("/")).await.unwrap();
        let headers = response.headers();
        assert!(headers.contains_key(&X_REQUEST_ID));
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert!(headers.contains_key(header::STRICT_TRANSPORT_SECURITY));
    }

    #[tokio::test]
    async fn a_panic_becomes_a_500() {
        let response = app().oneshot(get_request("/panic")).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused() {
        let request = http::Request::builder()
            .method("POST")
            .uri("/echo")
            .body(Body::from(vec![b'a'; 17]))
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn an_empty_origin_list_means_no_cors_layer() {
        assert!(cors(&[]).unwrap().is_none());
    }

    #[test]
    fn listed_origins_are_validated() {
        assert!(
            cors(&["https://example.org".to_string()])
                .unwrap()
                .is_some()
        );
        assert!(cors(&["bad\norigin".to_string()]).is_err());
    }
}
