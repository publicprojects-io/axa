//! The standard middleware stack.
//!
//! From the outside in: request id, tracing, security headers, CORS, timeout,
//! compression, refusal shaping, body limit, panic catching. A panic is caught last
//! in, so that its answer gets everything any other answer gets.

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
    http::{HeaderName, HeaderValue, Method, header},
    middleware::{Next, from_fn, from_fn_with_state},
    response::Response,
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

/// Whether an error's `content-type` says it was never meant to be read as data: there
/// is none, as on an unmatched path, or it is the plain text Axum's rejections use.
fn is_bare(content_type: Option<&HeaderValue>) -> bool {
    content_type.is_none_or(|value| {
        value.to_str().is_ok_and(|value| {
            let media_type = value.split(';').next().unwrap_or(value);
            media_type.trim().eq_ignore_ascii_case("text/plain")
        })
    })
}

/// Turn a bare refusal into a problem, as every other error is. Axum writes these
/// itself: an unmatched path, a wrong method, and an extractor's rejection. The body is
/// dropped and not carried into `detail`, as a rejection's text quotes what the caller
/// sent. The headers stay, as a `405`'s `allow` and a `429`'s `retry-after` mean
/// something.
async fn shape_refusals(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error())
        || !is_bare(response.headers().get(header::CONTENT_TYPE))
    {
        return response;
    }
    let (mut parts, _) = response.into_parts();
    let (problem, body) = crate::error::refusal_response(status).into_parts();
    parts.headers.remove(header::CONTENT_ENCODING);
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.extend(problem.headers);
    Response::from_parts(parts, body)
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
        crate::error::timeout_response()
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
        .unwrap_or_else(|_| crate::error::timeout_response())
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
    // Innermost, so that a panic's answer passes through everything below.
    let mut router = router
        .layer(CatchPanicLayer::custom(panicked))
        .layer(DefaultBodyLimit::max(config.body_limit))
        .layer(from_fn(shape_refusals));
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
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request| request_span(request))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(from_fn_with_state(
            Arc::new(config.trusted_proxies.clone()),
            crate::client::record,
        ))
        .layer(from_fn(request_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http, http::StatusCode, response::IntoResponse, routing::get};
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
            trusted_proxies: Vec::new(),
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

    async fn refuse(_: Request, _: Next) -> Response {
        StatusCode::UNAUTHORIZED.into_response()
    }

    #[tokio::test]
    async fn a_refusal_from_a_layer_inside_the_stack_gets_the_standard_headers() {
        let router = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn(refuse));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(get_request("/"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key(&X_REQUEST_ID));
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
    }

    #[tokio::test]
    async fn a_refusal_from_a_layer_outside_the_stack_gets_none_of_them() {
        let response = app()
            .layer(from_fn(refuse))
            .oneshot(get_request("/"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key(&X_REQUEST_ID));
        assert!(
            !response
                .headers()
                .contains_key(header::X_CONTENT_TYPE_OPTIONS)
        );
    }

    /// What a layer that reads an error's body sees of it.
    async fn read_body(request: Request, next: Next) -> Response {
        let response = next.run(request).await;
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let readable = serde_json::from_slice::<serde_json::Value>(&bytes).is_ok();
        let mut response = Response::from_parts(parts, Body::from(bytes));
        response.extensions_mut().insert(readable);
        response
    }

    async fn an_error() -> impl IntoResponse {
        (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "not_found",
                "description": "Resource was not found, and this body is long enough to compress",
            })),
        )
    }

    fn compressing() -> Http {
        Http {
            compression: true,
            ..config()
        }
    }

    fn accepting_gzip() -> http::Request<Body> {
        http::Request::builder()
            .uri("/")
            .header(header::ACCEPT_ENCODING, "gzip")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn a_layer_outside_the_stack_reads_a_compressed_body() {
        let router = Router::new().route("/", get(an_error));
        let response = apply(router, &compressing())
            .unwrap()
            .layer(from_fn(read_body))
            .oneshot(accepting_gzip())
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(response.extensions().get::<bool>(), Some(&false));
    }

    #[tokio::test]
    async fn a_layer_inside_the_stack_reads_a_plain_body() {
        async fn saw_it_plain(request: Request, next: Next) -> Response {
            let response = read_body(request, next).await;
            let readable = response.extensions().get::<bool>().copied();
            assert_eq!(readable, Some(true));
            response
        }
        let router = Router::new()
            .route("/", get(an_error))
            .layer(from_fn(saw_it_plain));
        let response = apply(router, &compressing())
            .unwrap()
            .oneshot(accepting_gzip())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            axa_problem::PROBLEM_JSON
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            bytes,
            r#"{"type":"about:blank","title":"Request Timeout","status":408,"code":"request_timeout"}"#
        );
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
    async fn a_panic_gets_the_same_headers_as_any_other_answer() {
        let origin = "https://app.example.com";
        let mut http = config();
        http.cors_origins = vec![origin.to_string()];
        let router = Router::new().route("/panic", get(panics));
        let app = apply(router, &http).unwrap();
        let request = http::Request::get("/panic")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let headers = response.headers();
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
        assert!(headers.contains_key(header::STRICT_TRANSPORT_SECURITY));
        assert_eq!(
            headers[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'"
        );
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
    }

    #[tokio::test]
    async fn a_panic_answers_like_any_other_internal_failure() {
        let response = app().oneshot(get_request("/panic")).await.unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            axa_problem::PROBLEM_JSON
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            body,
            r#"{"type":"about:blank","title":"Internal Server Error","status":500,"code":"internal_server_error"}"#
        );
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

    /// A service with a route for each way to refuse.
    fn refusing_app() -> Router {
        #[derive(serde::Deserialize)]
        struct Named {
            #[allow(dead_code)]
            name: String,
        }
        async fn takes_json(_: axum::Json<Named>) {}
        async fn takes_number(_: axum::extract::Path<u32>) {}
        async fn says_why() -> (StatusCode, [(HeaderName, &'static str); 1], &'static str) {
            (
                StatusCode::CONFLICT,
                [(header::CONTENT_LENGTH, "11")],
                "secret text",
            )
        }
        async fn only_a_status() -> StatusCode {
            StatusCode::SERVICE_UNAVAILABLE
        }
        async fn with_a_header() -> (StatusCode, [(HeaderName, &'static str); 1]) {
            (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "30")])
        }
        async fn own_error() -> Response {
            (
                StatusCode::IM_A_TEAPOT,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"mine":true}"#,
            )
                .into_response()
        }
        async fn a_success() -> &'static str {
            "fine"
        }
        let router = Router::new()
            .route("/json", axum::routing::post(takes_json))
            .route("/number/{n}", get(takes_number))
            .route("/why", get(says_why))
            .route("/status", get(only_a_status))
            .route("/header", get(with_a_header))
            .route("/own", get(own_error))
            .route("/fine", get(a_success));
        apply(router, &config()).unwrap()
    }

    async fn problem_of(response: Response) -> serde_json::Value {
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            axa_problem::PROBLEM_JSON
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn an_unmatched_path_answers_like_not_found() {
        let response = refusing_app()
            .oneshot(get_request("/nowhere"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.headers().contains_key(X_REQUEST_ID));
        let expected = problem_of(Error::NotFound.into_response()).await;
        assert_eq!(problem_of(response).await, expected);
    }

    #[tokio::test]
    async fn a_wrong_method_is_a_problem_and_keeps_its_allow_header() {
        let request = http::Request::post("/fine").body(Body::empty()).unwrap();
        let response = refusing_app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert!(
            response.headers()[header::ALLOW]
                .to_str()
                .unwrap()
                .contains("GET")
        );
        assert_eq!(
            problem_of(response).await,
            serde_json::json!({
                "type": "about:blank",
                "title": "Method Not Allowed",
                "status": 405,
                "code": "method_not_allowed",
            })
        );
    }

    #[tokio::test]
    async fn an_oversized_body_is_a_problem() {
        let request = http::Request::post("/echo")
            .body(Body::from(vec![b'a'; 17]))
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let problem = problem_of(response).await;
        assert_eq!(problem["status"], 413);
        assert!(problem.get("detail").is_none());
    }

    #[tokio::test]
    async fn a_rejected_extractor_is_a_problem_that_quotes_nothing() {
        for (request, status) in [
            (
                http::Request::post("/json")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":1234567}"#))
                    .unwrap(),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                http::Request::post("/json")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{1234567"))
                    .unwrap(),
                StatusCode::BAD_REQUEST,
            ),
            (
                http::Request::post("/json").body(Body::from("{}")).unwrap(),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                http::Request::get("/number/1234567x")
                    .body(Body::empty())
                    .unwrap(),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let response = refusing_app().oneshot(request).await.unwrap();
            assert_eq!(response.status(), status);
            let problem = problem_of(response).await;
            assert_eq!(problem["status"], status.as_u16());
            assert!(!problem.to_string().contains("1234567"), "{problem}");
        }
    }

    #[tokio::test]
    async fn a_handlers_plain_text_error_is_not_sent() {
        let response = refusing_app().oneshot(get_request("/why")).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        // The handler's length belongs to the text that was dropped.
        let length = response.headers().get(header::CONTENT_LENGTH).cloned();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        if let Some(length) = length {
            assert_eq!(length.to_str().unwrap(), bytes.len().to_string());
        }
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({
                "type": "about:blank",
                "title": "Conflict",
                "status": 409,
                "code": "conflict",
            })
        );
    }

    #[tokio::test]
    async fn a_bare_status_is_a_problem_and_keeps_its_headers() {
        let response = refusing_app()
            .oneshot(get_request("/status"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(problem_of(response).await["code"], "service_unavailable");

        let response = refusing_app()
            .oneshot(get_request("/header"))
            .await
            .unwrap();
        assert_eq!(response.headers()[header::RETRY_AFTER], "30");
        assert_eq!(problem_of(response).await["code"], "too_many_requests");
    }

    #[tokio::test]
    async fn an_error_with_its_own_media_type_and_a_success_are_left_alone() {
        let response = refusing_app().oneshot(get_request("/own")).await.unwrap();
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], br#"{"mine":true}"#);

        let response = refusing_app().oneshot(get_request("/fine")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn a_refusal_is_compressed_like_any_other_answer() {
        let mut http = config();
        http.compression = true;
        let app = apply(Router::new().route("/fine", get(|| async {})), &http).unwrap();
        let request = http::Request::get("/nowhere")
            .header(header::ACCEPT_ENCODING, "gzip")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            axa_problem::PROBLEM_JSON
        );
        assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
    }

    #[test]
    fn only_a_missing_type_or_plain_text_is_bare() {
        assert!(is_bare(None));
        for value in ["text/plain", "TEXT/PLAIN; charset=utf-8"] {
            assert!(is_bare(Some(&HeaderValue::from_static(value))), "{value}");
        }
        for value in ["text/html", "application/json", "text/plainer"] {
            assert!(!is_bare(Some(&HeaderValue::from_static(value))), "{value}");
        }
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
