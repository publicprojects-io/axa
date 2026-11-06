//! The standard middleware stack.
//!
//! From the outside in: request id, tracing, panic catching, security
//! headers, CORS, timeout, compression, body limit.

use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    extract::{DefaultBodyLimit, MatchedPath, Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{Next, from_fn, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer},
    set_header::SetResponseHeaderLayer,
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use uuid::Uuid;

use crate::{AppContext, Error, Result, config::Http};

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
    let value = HeaderValue::from_str(&id).ok();
    // Inner layers read the header too, so it must carry this id and no other.
    if let Some(value) = &value {
        request.headers_mut().insert(X_REQUEST_ID, value.clone());
    }
    let mut response = next.run(request).await;
    if let Some(value) = value {
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

/// What logs call a request that matched no route. The path itself is kept out,
/// because anyone can put anything in it.
const UNMATCHED: &str = "unmatched";

/// The tracing span for a request, so every log line written while serving it
/// carries its request id. It records the route's template and not the path or
/// query string: `/unsubscribe/{token}`, not the token, because a path can hold a
/// secret or a person's identifier just as a query string can.
fn request_span(request: &Request) -> tracing::Span {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map_or("", |id| id.0.as_str());
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map_or(UNMATCHED, MatchedPath::as_str);
    tracing::info_span!(
        "http-request",
        method = %request.method(),
        path = %path,
        request_id = %request_id,
    )
}

/// The answer to a panic, in the shape of any other internal failure. The panic's
/// text is left out of this line, as it can quote anything, and Rust's panic hook
/// has already written it to stderr. The line itself carries the request id.
fn panicked(_panic: Box<dyn Any + Send>) -> Response {
    tracing::error!("handler_panicked");
    crate::error::internal_response()
}

/// A body limit for one route, replacing the service-wide limit there.
///
/// Keep the service-wide limit small and raise it only on the routes that need
/// it, rather than accepting large bodies everywhere:
///
/// ```
/// use axa::middleware::body_limit;
/// use axum::routing::{MethodRouter, post};
///
/// async fn upload() {}
///
/// let route: MethodRouter = post(upload).layer(body_limit(70_000_000));
/// ```
pub fn body_limit(bytes: usize) -> DefaultBodyLimit {
    DefaultBodyLimit::max(bytes)
}

/// The timeout for `path`, or `None` for no timeout.
fn timeout_for(config: &Http, path: &str) -> Option<Duration> {
    let path = path.trim_end_matches('/');
    let seconds = config
        .timeout_overrides
        .iter()
        .find(|timeout| path.ends_with(&timeout.path_suffix))
        .map_or(config.timeout_seconds, |timeout| timeout.seconds);
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// Set by a route with its own limit, to tell [`timeout`] that the request is
/// now that route's to time.
#[derive(Clone, Default)]
struct RouteLimit(Arc<AtomicBool>);

/// Answer `408` when a request runs past its timeout.
///
/// An inner layer cannot outlive an outer one, so a route with a longer limit
/// of its own needs this layer to stand aside, and this layer runs before
/// routing. So it leaves a flag on the request. When its time is up it looks at
/// the flag, and if a route has taken over it waits for that route, which
/// enforces its own limit.
async fn timeout(State(config): State<Arc<Http>>, mut request: Request, next: Next) -> Response {
    let Some(limit) = timeout_for(&config, request.uri().path()) else {
        return next.run(request).await;
    };
    let taken_over = RouteLimit::default();
    request.extensions_mut().insert(taken_over.clone());
    let run = next.run(request);
    tokio::pin!(run);
    tokio::select! {
        response = &mut run => return response,
        () = tokio::time::sleep(limit) => {}
    }
    if taken_over.0.load(Ordering::SeqCst) {
        run.await
    } else {
        StatusCode::REQUEST_TIMEOUT.into_response()
    }
}

/// A route's own limit, in place of [`timeout`]'s.
async fn route_timeout(limit: Duration, request: Request, next: Next) -> Response {
    if let Some(taken_over) = request.extensions().get::<RouteLimit>() {
        taken_over.0.store(true, Ordering::SeqCst);
    }
    if limit.is_zero() {
        return next.run(request).await;
    }
    tokio::time::timeout(limit, next.run(request))
        .await
        .unwrap_or_else(|_| StatusCode::REQUEST_TIMEOUT.into_response())
}

/// `method` with its own time limit, in place of the stack's. Zero means no
/// limit, as `REQUEST_TIMEOUT_SECONDS=0` does.
pub(crate) fn limit_route(
    method: MethodRouter<AppContext>,
    limit: Duration,
) -> MethodRouter<AppContext> {
    method.layer(from_fn(move |request, next| {
        route_timeout(limit, request, next)
    }))
}

fn set_default_header(
    name: HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    default_header(name, HeaderValue::from_static(value))
}

fn default_header(name: HeaderName, value: HeaderValue) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, value)
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
    if config.timeout_seconds > 0 || !config.timeout_overrides.is_empty() {
        router = router.layer(from_fn_with_state(Arc::new(config.clone()), timeout));
    }
    if let Some(layer) = cors(&config.cors_origins)? {
        router = router.layer(layer);
    }
    if config.hsts {
        let value = format!("max-age={}; includeSubDomains", config.hsts_max_age);
        router = router.layer(default_header(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_str(&value).map_err(|_| Error::string("invalid HSTS max age"))?,
        ));
    }
    if let Some(policy) = &config.csp {
        router = router.layer(default_header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(policy)
                .map_err(|_| Error::string("HTTP_CSP is not a valid header value"))?,
        ));
    }
    Ok(router
        .layer(set_default_header(
            header::X_CONTENT_TYPE_OPTIONS,
            "nosniff",
        ))
        .layer(set_default_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(set_default_header(header::REFERRER_POLICY, "no-referrer"))
        .layer(CatchPanicLayer::custom(panicked))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request| request_span(request))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
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
            timeout_overrides: Vec::new(),
            cors_origins: Vec::new(),
            hsts: true,
            hsts_max_age: 31_536_000,
            csp: Some("default-src 'none'".to_string()),
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
    async fn the_request_header_carries_the_id_the_response_does() {
        async fn echo(headers: http::HeaderMap) -> String {
            headers[&X_REQUEST_ID].to_str().unwrap().to_string()
        }
        for caller in [None, Some("abc<>123")] {
            let app = apply(Router::new().route("/", get(echo)), &config()).unwrap();
            let mut request = http::Request::builder().uri("/");
            if let Some(caller) = caller {
                request = request.header(&X_REQUEST_ID, caller);
            }
            let response = app
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let id = response.headers()[&X_REQUEST_ID]
                .to_str()
                .unwrap()
                .to_string();
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(body, id.as_bytes());
        }
    }

    #[tokio::test]
    async fn responses_carry_request_id_and_security_headers() {
        let response = app().oneshot(get_request("/")).await.unwrap();
        let headers = response.headers();
        assert!(headers.contains_key(&X_REQUEST_ID));
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(
            headers[header::STRICT_TRANSPORT_SECURITY],
            "max-age=31536000; includeSubDomains"
        );
        assert_eq!(
            headers[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'"
        );
    }

    fn apply_to(config: &Http) -> Result<Router> {
        apply(Router::new().route("/", get(|| async { "ok" })), config)
    }

    #[tokio::test]
    async fn the_hsts_max_age_is_configurable() {
        let mut http = config();
        http.hsts_max_age = 63_072_000;
        let response = apply_to(&http)
            .unwrap()
            .oneshot(get_request("/"))
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::STRICT_TRANSPORT_SECURITY],
            "max-age=63072000; includeSubDomains"
        );
    }

    #[tokio::test]
    async fn no_policy_means_no_csp_header() {
        let mut http = config();
        http.csp = None;
        let response = apply_to(&http)
            .unwrap()
            .oneshot(get_request("/"))
            .await
            .unwrap();
        assert!(
            !response
                .headers()
                .contains_key(header::CONTENT_SECURITY_POLICY)
        );
    }

    #[tokio::test]
    async fn a_handler_may_set_its_own_policy() {
        let router = Router::new().route(
            "/",
            get(|| async {
                (
                    [(header::CONTENT_SECURITY_POLICY, "default-src 'self'")],
                    "ok",
                )
            }),
        );
        let response = apply(router, &config())
            .unwrap()
            .oneshot(get_request("/"))
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            "default-src 'self'"
        );
    }

    #[test]
    fn a_policy_that_is_not_a_header_value_refuses_to_boot() {
        let mut http = config();
        http.csp = Some("line\nbreak".to_string());
        assert!(apply_to(&http).is_err());
    }

    #[tokio::test]
    async fn security_headers_do_not_replace_a_handlers_own() {
        let router = Router::new().route(
            "/",
            get(|| async { ([(header::REFERRER_POLICY, "same-origin")], "ok") }),
        );
        let app = apply(router, &config()).unwrap();
        let response = app.oneshot(get_request("/")).await.unwrap();
        assert_eq!(response.headers()[header::REFERRER_POLICY], "same-origin");
        let response = app_with_defaults().await;
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    }

    async fn app_with_defaults() -> Response {
        app().oneshot(get_request("/")).await.unwrap()
    }

    fn slow_app(config: &Http) -> Router {
        let router = Router::new()
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    "ok"
                }),
            )
            .route(
                "/import",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    "ok"
                }),
            );
        apply(router, config).unwrap()
    }

    fn with_timeout(seconds: u64) -> Http {
        Http {
            timeout_seconds: seconds,
            ..config()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_past_its_timeout_is_a_408() {
        let response = slow_app(&with_timeout(1))
            .oneshot(get_request("/slow"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn a_trailing_slash_still_matches_an_override() {
        let config = Http {
            timeout_overrides: vec![crate::config::TimeoutOverride {
                path_suffix: "/import".to_string(),
                seconds: 30,
            }],
            ..with_timeout(1)
        };
        assert_eq!(
            timeout_for(&config, "/api/x/import/"),
            Some(Duration::from_secs(30))
        );
        assert_eq!(timeout_for(&config, "/api/x"), Some(Duration::from_secs(1)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_timeout_means_none() {
        let response = slow_app(&with_timeout(0))
            .oneshot(get_request("/slow"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test(start_paused = true)]
    async fn a_route_can_have_a_longer_timeout() {
        let config = Http {
            timeout_overrides: vec![crate::config::TimeoutOverride {
                path_suffix: "/import".to_string(),
                seconds: 30,
            }],
            ..with_timeout(1)
        };
        let app = slow_app(&config);
        let response = app.clone().oneshot(get_request("/import")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app.oneshot(get_request("/slow")).await.unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn an_override_can_turn_the_timeout_off() {
        let config = Http {
            timeout_overrides: vec![crate::config::TimeoutOverride {
                path_suffix: "/import".to_string(),
                seconds: 0,
            }],
            ..with_timeout(1)
        };
        let response = slow_app(&config)
            .oneshot(get_request("/import"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn one_route_can_accept_a_larger_body() {
        let router = Router::new()
            .route(
                "/small",
                axum::routing::post(|body: String| async move { body }),
            )
            .route(
                "/large",
                axum::routing::post(|body: String| async move { body }).layer(body_limit(64)),
            );
        let app = apply(router, &config()).unwrap();
        let post = |path: &str| {
            http::Request::builder()
                .method("POST")
                .uri(path)
                .body(Body::from(vec![b'a'; 32]))
                .unwrap()
        };
        let response = app.clone().oneshot(post("/large")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app.oneshot(post("/small")).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_panic_becomes_a_500() {
        let response = app().oneshot(get_request("/panic")).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn a_panic_answers_like_any_other_internal_failure() {
        let response = app().oneshot(get_request("/panic")).await.unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("internal_server_error"), "{body}");
        assert!(!body.contains("boom"), "{body}");
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
