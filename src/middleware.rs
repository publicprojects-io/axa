//! The standard middleware stack.
//!
//! From the outside in: cross-site guard, request id, tracing, client address,
//! security headers, CORS, timeout, `304`s, compression, refusal shaping,
//! body limit, panic catching. A panic is caught last in, so that its answer
//! gets everything any other answer gets.

use std::{
    any::Any,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes, HttpBody},
    extract::{DefaultBodyLimit, MatchedPath, Request, State},
    http::{
        Extensions, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header, uri::Authority,
    },
    middleware::{Next, from_fn, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::{CompressionLayer, DefaultPredicate, Predicate},
    cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer},
    sensitive_headers::SetSensitiveHeadersLayer,
    set_header::SetResponseHeaderLayer,
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use uuid::Uuid;

use crate::{AppContext, Error, Result, config::Http, error::ErrorDetail};

/// The request id header.
pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// `sec-fetch-site` header name.
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

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
            .allow_headers(AllowHeaders::mirror_request())
            // A browser hides this from a page unless told, and a page needs it to send back.
            .expose_headers([header::ETAG]),
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

/// `method` with a body limit of its own, replacing the service-wide limit there, for
/// the extractors and for a handler that reads the body itself.
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
/// let route: MethodRouter = body_limit(post(upload), 70_000_000);
/// ```
pub fn body_limit<S>(method: MethodRouter<S>, bytes: usize) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    method.layer(DefaultBodyLimit::max(bytes)).layer(from_fn(
        move |request: Request, next: Next| raise_body_limit(bytes, request, next),
    ))
}

/// The limit on the body of one request, set by the stack and raised by a route's own
/// [`body_limit`] before anything reads the body.
#[derive(Clone)]
struct BodyLimit(Arc<AtomicUsize>);

/// Bind every request's body to the service-wide limit, or to a route's own, so that a
/// handler reading the body itself, past the extractors' `DefaultBodyLimit`, is bound
/// too. The limit is read when the body is first read, by which time a route has set its
/// own.
async fn limit_body(State(bytes): State<usize>, request: Request, next: Next) -> Response {
    let limit = BodyLimit(Arc::new(AtomicUsize::new(bytes)));
    let (mut parts, body) = request.into_parts();
    parts.extensions.insert(limit.clone());
    let body = Body::new(LimitedLater::Waiting(Some(body), limit));
    next.run(Request::from_parts(parts, body)).await
}

/// A route's own limit, in place of the service-wide one.
async fn raise_body_limit(bytes: usize, request: Request, next: Next) -> Response {
    if let Some(limit) = request.extensions().get::<BodyLimit>() {
        limit.0.store(bytes, Ordering::SeqCst);
    }
    next.run(request).await
}

/// A body bound by `http_body_util::Limited`, whose error `axum` answers as `413`, made
/// when it is first read.
enum LimitedLater {
    Waiting(Option<Body>, BodyLimit),
    Reading(http_body_util::Limited<Body>),
}

impl HttpBody for LimitedLater {
    type Data = Bytes;
    type Error = axum::BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        if let Self::Waiting(body, limit) = &mut *self {
            let bytes = limit.0.load(Ordering::SeqCst);
            let body = body.take().unwrap_or_default();
            *self = Self::Reading(http_body_util::Limited::new(body, bytes));
        }
        match &mut *self {
            Self::Reading(limited) => Pin::new(limited).poll_frame(context),
            Self::Waiting(..) => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Waiting(Some(body), _) => body.is_end_stream(),
            Self::Waiting(None, _) => true,
            Self::Reading(limited) => limited.is_end_stream(),
        }
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        match self {
            Self::Waiting(Some(body), _) => body.size_hint(),
            Self::Waiting(None, _) => hyper::body::SizeHint::with_exact(0),
            Self::Reading(limited) => limited.size_hint(),
        }
    }
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
        .layer(from_fn_with_state(config.body_limit, limit_body))
        .layer(from_fn(shape_refusals));
    if config.compression {
        // Never a response holding a secret, which compression can leak (BREACH).
        let predicate = DefaultPredicate::new()
            .and(|_, _, headers: &HeaderMap, _: &Extensions| !holds_a_secret(headers));
        router = router.layer(CompressionLayer::new().compress_when(predicate));
    }
    // Outside compression, so that it sees what a handler set and compression sees it.
    router = router.layer(from_fn(no_store_secrets));
    // Outside compression, so that a `304` has the `vary` the `200` would have had, and
    // nothing is compressed only to be dropped.
    router = router.layer(from_fn(crate::conditional::not_modified));
    if config.timeout_seconds > 0 || !config.timeout_overrides.is_empty() {
        router = router.layer(from_fn_with_state(Arc::new(config.clone()), timeout));
    }
    // Inside CORS, the trace, and the standard headers, so that a refusal has them all.
    router = router.layer(from_fn_with_state(
        Arc::new(CrossSite::new(config)),
        cross_site_guard,
    ));
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
    if let Some(policy) = &config.permissions_policy {
        router = router.layer(default_header(
            PERMISSIONS_POLICY,
            HeaderValue::from_str(policy).map_err(|_| {
                Error::string("HTTP_PERMISSIONS_POLICY is not a valid header value")
            })?,
        ));
    }
    if let Some(policy) = &config.csp {
        router = router.layer(default_header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(policy).map_err(|_| {
                Error::string("HTTP_CONTENT_SECURITY_POLICY is not a valid header value")
            })?,
        ));
    }
    Ok(router
        .layer(set_default_header(
            header::X_CONTENT_TYPE_OPTIONS,
            "nosniff",
        ))
        .layer(set_default_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(set_default_header(header::REFERRER_POLICY, "no-referrer"))
        .layer(set_default_header(
            CROSS_ORIGIN_OPENER_POLICY,
            "same-origin",
        ))
        .layer(set_default_header(
            CROSS_ORIGIN_RESOURCE_POLICY,
            "same-origin",
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request| request_span(request))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(from_fn_with_state(
            Arc::new(config.trusted_proxies.clone()),
            crate::client::record,
        ))
        .layer(from_fn(request_id))
        // Outermost, so that no layer, the trace above all, records a credential.
        .layer(SetSensitiveHeadersLayer::new([
            header::AUTHORIZATION,
            header::PROXY_AUTHORIZATION,
            header::COOKIE,
            header::SET_COOKIE,
        ])))
}

/// `permissions-policy`, which `http` does not name.
const PERMISSIONS_POLICY: HeaderName = HeaderName::from_static("permissions-policy");
/// `cross-origin-opener-policy`, which `http` does not name.
const CROSS_ORIGIN_OPENER_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-opener-policy");
/// `cross-origin-resource-policy`, which `http` does not name.
const CROSS_ORIGIN_RESOURCE_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-resource-policy");

/// Whether a response holds a secret: it sets a cookie, or it says not to store it.
fn holds_a_secret(headers: &HeaderMap) -> bool {
    headers.contains_key(header::SET_COOKIE)
        || headers
            .get_all(header::CACHE_CONTROL)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| {
                value
                    .split(',')
                    .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"))
            })
}

/// Mark a response that sets a cookie `no-store`, unless its handler said how to cache
/// it, so that no cache keeps a session or a token.
async fn no_store_secrets(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if headers.contains_key(header::SET_COOKIE) && !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// What the cross-site write guard lets through: the origins CORS lists, and the
/// paths a service named in [`Http::cross_site_paths`].
struct CrossSite {
    /// Any origin, for a CORS list of `*`, which production refuses.
    any_origin: bool,
    origins: Vec<String>,
    paths: Vec<String>,
}

impl CrossSite {
    fn new(config: &Http) -> Self {
        Self {
            any_origin: config.cors_origins.iter().any(|origin| origin == "*"),
            origins: config.cors_origins.clone(),
            paths: config
                .cross_site_paths
                .iter()
                .map(|path| trimmed_path(path).to_string())
                .collect(),
        }
    }

    /// Whether `request` may write from another site, whatever its headers say.
    fn allows(&self, request: &Request) -> bool {
        let path = trimmed_path(request.uri().path());
        if self.paths.iter().any(|allowed| allowed == path) {
            return true;
        }
        request
            .headers()
            .get(header::ORIGIN)
            .and_then(|origin| origin.to_str().ok())
            .is_some_and(|origin| {
                self.any_origin
                    // Exactly, as CORS compares them.
                    || self.origins.iter().any(|allowed| allowed == origin)
            })
    }
}

/// `path` without a trailing slash, keeping `/` itself.
fn trimmed_path(path: &str) -> &str {
    match path.trim_end_matches('/') {
        "" => "/",
        trimmed => trimmed,
    }
}

/// Refuse a write from another site: an unsafe method or a WebSocket upgrade with
/// `sec-fetch-site: cross-site` is `403` with code `cross_site_write_refused`. With no
/// `sec-fetch-site`, an `origin` that is not the request's own host is `403` with code
/// `origin_mismatch`. With neither header, the request is from no browser and passes.
/// An origin CORS lists, and a path in [`Http::cross_site_paths`], pass too.
async fn cross_site_guard(
    State(allowed): State<Arc<CrossSite>>,
    request: Request,
    next: Next,
) -> Response {
    if !is_unsafe_method(request.method()) && !is_websocket_upgrade(request.headers()) {
        return next.run(request).await;
    }
    if let Some(reason) = cross_site_refusal(&request)
        && !allowed.allows(&request)
    {
        let detail = ErrorDetail::with_reason(reason);
        return Error::CustomError(StatusCode::FORBIDDEN, detail).into_response();
    }
    next.run(request).await
}

/// Why `request` is a write from another site, or `None` when it is not one.
fn cross_site_refusal(request: &Request) -> Option<&'static str> {
    let headers = request.headers();
    if let Some(site) = headers.get(&SEC_FETCH_SITE) {
        // `same-origin`, `same-site`, and `none`, which a user's own navigation sends,
        // pass. A value that is not text is no browser's, and is refused.
        return match site.to_str() {
            Ok(site) if !site.eq_ignore_ascii_case("cross-site") => None,
            _ => Some("cross_site_write_refused"),
        };
    }
    let origin = headers.get(header::ORIGIN)?;
    // HTTP/2 sends the host as `:authority`, which reaches a handler in the URI.
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .or_else(|| request.uri().authority().map(Authority::as_str));
    let matches = match (origin.to_str(), host) {
        (Ok(origin), Some(host)) => origin_host(origin).eq_ignore_ascii_case(host),
        _ => false,
    };
    (!matches).then_some("origin_mismatch")
}

/// Whether `method` can change something: anything but `GET`, `HEAD`, and `OPTIONS`.
fn is_unsafe_method(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Whether the request asks to become a WebSocket.
fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// The `host[:port]` of an origin, `scheme://host[:port]`. An opaque origin, `null`,
/// has none, and matches no host.
fn origin_host(origin: &str) -> &str {
    origin.split_once("://").map_or(origin, |(_, host)| host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http,
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
    };
    use tower::ServiceExt;

    fn config() -> Http {
        Http {
            body_limit: 16,
            compression: false,
            timeout_seconds: 0,
            timeout_overrides: Vec::new(),
            cross_site_paths: Vec::new(),
            cors_origins: Vec::new(),
            hsts: true,
            hsts_max_age: 31_536_000,
            csp: Some("default-src 'none'".to_string()),
            permissions_policy: Some(crate::config::DEFAULT_PERMISSIONS_POLICY.to_string()),
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

    #[tokio::test]
    async fn the_cross_origin_and_permissions_headers_are_set_unless_a_handler_sets_its_own() {
        let response = app_with_defaults().await;
        assert_eq!(
            response.headers()[CROSS_ORIGIN_OPENER_POLICY],
            "same-origin"
        );
        assert_eq!(
            response.headers()[CROSS_ORIGIN_RESOURCE_POLICY],
            "same-origin"
        );
        assert_eq!(
            response.headers()[PERMISSIONS_POLICY],
            crate::config::DEFAULT_PERMISSIONS_POLICY
        );
        let router = Router::new().route(
            "/",
            get(|| async { ([(CROSS_ORIGIN_RESOURCE_POLICY, "cross-origin")], "ok") }),
        );
        let response = apply(router, &config())
            .unwrap()
            .oneshot(get_request("/"))
            .await
            .unwrap();
        assert_eq!(
            response.headers()[CROSS_ORIGIN_RESOURCE_POLICY],
            "cross-origin"
        );
        let none = Http {
            permissions_policy: None,
            ..config()
        };
        let router = Router::new().route("/", get(|| async { "ok" }));
        let response = apply(router, &none)
            .unwrap()
            .oneshot(get_request("/"))
            .await
            .unwrap();
        assert!(response.headers().get(PERMISSIONS_POLICY).is_none());
    }

    /// A router whose `/cookie` sets a cookie, and whose `/cached` sets one with its own
    /// caching, both with a body long enough to compress.
    fn cookies() -> Router {
        let long = "a cookie and a body long enough to compress ".repeat(20);
        let cached = long.clone();
        Router::new()
            .route(
                "/cookie",
                get(move || async move { ([(header::SET_COOKIE, "id=secret")], long) }),
            )
            .route(
                "/cached",
                get(move || async move {
                    (
                        [
                            (header::SET_COOKIE, "id=secret"),
                            (header::CACHE_CONTROL, "private, max-age=60"),
                        ],
                        cached,
                    )
                }),
            )
    }

    fn gzip_request(path: &str) -> http::Request<Body> {
        http::Request::builder()
            .uri(path)
            .header(header::ACCEPT_ENCODING, "gzip")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn a_response_setting_a_cookie_is_not_stored_or_compressed() {
        let app = apply(cookies(), &compressing()).unwrap();
        let response = app.clone().oneshot(gzip_request("/cookie")).await.unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
        // A handler's own caching stands, and it is still not compressed.
        let response = app.oneshot(gzip_request("/cached")).await.unwrap();
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, max-age=60"
        );
        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
        // Anything else is still compressed.
        let response = apply(Router::new().route("/", get(an_error)), &compressing())
            .unwrap()
            .oneshot(accepting_gzip())
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    }

    #[test]
    fn no_store_is_read_as_a_directive_among_others() {
        let headers = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
            headers
        };
        assert!(holds_a_secret(&headers("private, No-Store")));
        assert!(!holds_a_secret(&headers("no-cache, max-age=0")));
        assert!(!holds_a_secret(&HeaderMap::new()));
    }

    #[tokio::test]
    async fn credential_headers_are_marked_sensitive() {
        let router = Router::new().route(
            "/",
            get(|headers: HeaderMap| async move {
                let sensitive = |name| headers.get(name).is_some_and(HeaderValue::is_sensitive);
                (sensitive(header::AUTHORIZATION) && sensitive(header::COOKIE)).to_string()
            }),
        );
        let request = http::Request::builder()
            .uri("/")
            .header(header::AUTHORIZATION, "Bearer secret")
            .header(header::COOKIE, "id=secret")
            .body(Body::empty())
            .unwrap();
        let response = apply(router, &config())
            .unwrap()
            .oneshot(request)
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 64)
            .await
            .unwrap();
        assert_eq!(&body[..], b"true");
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
                body_limit(axum::routing::post(|body: String| async move { body }), 64),
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

    /// A handler that reads the body itself, past every extractor, and says how much
    /// it read, or `413` when the limit stopped it.
    async fn read_raw(body: Body) -> (StatusCode, String) {
        match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => (StatusCode::OK, bytes.len().to_string()),
            Err(error) => {
                // `axum` wraps a body's error in its own, once for each body around it.
                let mut inner = error.into_inner();
                loop {
                    match inner.downcast::<axum::Error>() {
                        Ok(wrapped) => inner = wrapped.into_inner(),
                        Err(unwrapped) => {
                            inner = unwrapped;
                            break;
                        }
                    }
                }
                assert!(inner.is::<http_body_util::LengthLimitError>(), "{inner}");
                (StatusCode::PAYLOAD_TOO_LARGE, String::new())
            }
        }
    }

    /// A body of `length` bytes, with no `content-length` header.
    fn streamed(path: &str, length: usize) -> http::Request<Body> {
        http::Request::builder()
            .method("POST")
            .uri(path)
            .body(Body::from(vec![b'a'; length]))
            .unwrap()
    }

    #[tokio::test]
    async fn a_handler_reading_the_body_itself_is_bound_by_the_limit() {
        let limit = config().body_limit;
        let router = Router::new()
            .route("/raw", axum::routing::post(read_raw))
            .route(
                "/large",
                body_limit(axum::routing::post(read_raw), limit * 2),
            );
        let app = apply(router, &config()).unwrap();
        let status = |path: &'static str, length| {
            let app = app.clone();
            async move { app.oneshot(streamed(path, length)).await.unwrap().status() }
        };
        assert_eq!(status("/raw", limit).await, StatusCode::OK);
        assert_eq!(
            status("/raw", limit + 1).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        // A route's own limit binds a handler reading the body itself too.
        assert_eq!(status("/large", limit + 1).await, StatusCode::OK);
        assert_eq!(
            status("/large", limit * 2 + 1).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
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

    // Tests for cross-site guard (T-13)

    fn cross_site_request(path: &str, method: &str) -> http::Request<Body> {
        http::Request::builder()
            .uri(path)
            .method(method)
            .header(SEC_FETCH_SITE, "cross-site")
            .body(Body::empty())
            .unwrap()
    }

    fn origin_request(path: &str, method: &str, origin: &str, host: &str) -> http::Request<Body> {
        http::Request::builder()
            .uri(path)
            .method(method)
            .header(header::ORIGIN, origin)
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap()
    }

    fn websocket_request(path: &str, sec_fetch_site: Option<&str>) -> http::Request<Body> {
        let mut builder = http::Request::builder()
            .uri(path)
            .method("GET")
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade");
        if let Some(sfs) = sec_fetch_site {
            builder = builder.header(SEC_FETCH_SITE, sfs);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn a_cross_site_unsafe_method_is_refused() {
        let router = Router::new()
            .route("/test", get(|| async { "ok" }))
            .route("/test", post(|| async { "created" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(cross_site_request("/test", "POST"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("cross_site_write_refused"));
    }

    #[tokio::test]
    async fn a_cross_site_websocket_upgrade_is_refused() {
        let router = Router::new().route("/ws", get(|| async { "ok" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(websocket_request("/ws", Some("cross-site")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("cross_site_write_refused"));
    }

    #[tokio::test]
    async fn an_unsafe_method_with_a_mismatched_origin_is_refused() {
        let router = Router::new().route("/test", post(|| async { "created" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(origin_request(
                "/test",
                "POST",
                "https://evil.com",
                "good.com:1241",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("origin_mismatch"));
    }

    #[tokio::test]
    async fn a_websocket_upgrade_with_a_mismatched_origin_is_refused() {
        // Create a websocket request with origin that doesn't match host
        let request = http::Request::builder()
            .uri("/ws")
            .method("GET")
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header(header::ORIGIN, "https://evil.com")
            .header(header::HOST, "good.com:1241")
            .body(Body::empty())
            .unwrap();
        let router = Router::new().route("/ws", get(|| async { "ok" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("origin_mismatch"));
    }

    #[tokio::test]
    async fn an_unsafe_method_with_no_browser_headers_passes() {
        let router = Router::new().route("/test", post(|| async { "created" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(
                http::Request::builder()
                    .uri("/test")
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_same_origin_unsafe_method_passes() {
        let router = Router::new().route("/test", post(|| async { "created" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(origin_request(
                "/test",
                "POST",
                "https://good.com:1241",
                "good.com:1241",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_same_site_unsafe_method_passes() {
        let router = Router::new().route("/test", post(|| async { "created" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(
                http::Request::builder()
                    .uri("/test")
                    .method("POST")
                    .header(SEC_FETCH_SITE, "same-site")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_websocket_upgrade_with_no_browser_headers_passes() {
        // WebSocket without sec-fetch-site or origin should pass (non-browser client)
        let request = http::Request::builder()
            .uri("/ws")
            .method("GET")
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .body(Body::empty())
            .unwrap();
        let router = Router::new().route("/ws", get(|| async { "ok" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    fn with_cross_site(cors_origins: &[&str], paths: &[&str]) -> Http {
        Http {
            cors_origins: cors_origins.iter().map(ToString::to_string).collect(),
            cross_site_paths: paths.iter().map(ToString::to_string).collect(),
            ..config()
        }
    }

    fn cross_site_from(path: &str, origin: &str) -> http::Request<Body> {
        http::Request::builder()
            .uri(path)
            .method("POST")
            .header(SEC_FETCH_SITE, "cross-site")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn a_cors_listed_origin_may_write_from_another_site() {
        let router = Router::new().route("/test", post(|| async { "created" }));
        let app = apply(router, &with_cross_site(&["https://app.example"], &[])).unwrap();
        let listed = app
            .clone()
            .oneshot(cross_site_from("/test", "https://app.example"))
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        assert_eq!(
            listed.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://app.example"
        );
        let unlisted = app
            .oneshot(cross_site_from("/test", "https://evil.example"))
            .await
            .unwrap();
        assert_eq!(unlisted.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_wildcard_cors_list_lets_every_origin_write() {
        let router = Router::new().route("/test", post(|| async { "created" }));
        let response = apply(router, &with_cross_site(&["*"], &[]))
            .unwrap()
            .oneshot(cross_site_from("/test", "https://any.example"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_listed_cross_site_path_is_let_through_and_no_other() {
        let router = Router::new()
            .route("/auth/callback", post(|| async { "signed in" }))
            .route("/auth/callback/more", post(|| async { "more" }));
        let app = apply(router, &with_cross_site(&[], &["/auth/callback/"])).unwrap();
        for path in ["/auth/callback", "/auth/callback/"] {
            let response = app
                .clone()
                .oneshot(cross_site_from(path, "https://idp.example"))
                .await
                .unwrap();
            // Axum routes no trailing slash, so the second is a `404`, but not a refusal.
            assert_ne!(response.status(), StatusCode::FORBIDDEN, "{path}");
        }
        let response = app
            .oneshot(cross_site_from(
                "/auth/callback/more",
                "https://idp.example",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_cross_site_refusal_gets_the_standard_headers() {
        let router = Router::new().route("/test", post(|| async { "created" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(cross_site_request("/test", "POST"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(response.headers().contains_key(X_REQUEST_ID));
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
    }

    #[tokio::test]
    async fn an_origin_is_compared_with_the_authority_when_there_is_no_host() {
        let router = Router::new().route("/test", post(|| async { "created" }));
        let app = apply(router, &config()).unwrap();
        for (origin, status) in [
            ("https://good.example", StatusCode::OK),
            ("https://evil.example", StatusCode::FORBIDDEN),
            ("null", StatusCode::FORBIDDEN),
        ] {
            let request = http::Request::builder()
                .uri("https://good.example/test")
                .method("POST")
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), status, "{origin}");
        }
        let no_host = http::Request::builder()
            .uri("/test")
            .method("POST")
            .header(header::ORIGIN, "https://good.example")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(no_host).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_safe_method_with_cross_site_passes() {
        let router = Router::new().route("/test", get(|| async { "ok" }));
        let response = apply(router, &config())
            .unwrap()
            .oneshot(cross_site_request("/test", "GET"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
