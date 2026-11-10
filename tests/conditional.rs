//! Conditional requests behind the standard middleware stack, as a service mounts them.

use std::{
    convert::Infallible,
    future::poll_fn,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use axa::{
    Result,
    conditional::{self, ETag, MAX_BODY},
    config::Http,
    middleware, server,
};
use axum::{
    Json, Router,
    body::{Body, Bytes, HttpBody, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{Next, from_fn},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use hyper::body::Frame;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

/// Long enough to be compressed.
const LONG: &str = "a body long enough that the compression layer will take an interest in it";

fn http() -> Http {
    Http {
        body_limit: 1024,
        compression: false,
        timeout_seconds: 0,
        timeout_overrides: Vec::new(),
        cors_origins: Vec::new(),
        hsts: false,
        hsts_max_age: 0,
        csp: None,
        permissions_policy: None,
        trusted_proxies: Vec::new(),
    }
}

fn compressed() -> Http {
    Http {
        compression: true,
        ..http()
    }
}

fn stack(routes: Router) -> Router {
    middleware::apply(routes, &http()).unwrap()
}

async fn call(app: &Router, method: Method, uri: &str, headers: &[(&str, &str)]) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn fetch(app: &Router, uri: &str, headers: &[(&str, &str)]) -> Response {
    call(app, Method::GET, uri, headers).await
}

async fn body_of(response: Response) -> Bytes {
    to_bytes(response.into_body(), usize::MAX).await.unwrap()
}

fn etag_of(response: &Response) -> String {
    response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string()
}

/// One handler that tags its own answer.
fn tagged(tag: &'static str) -> Router {
    Router::new().route(
        "/",
        get(move || async move { ([(header::ETAG, tag)], LONG) }),
    )
}

/// One route with bodies tagged by hashing.
fn hashed(handler: axum::routing::MethodRouter) -> Router {
    Router::new()
        .route("/", handler)
        .layer(from_fn(conditional::etag))
}

/// A body that sends `first`, if any, and never ends.
struct Endless {
    first: Option<Bytes>,
}

impl HttpBody for Endless {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, Infallible>>> {
        match self.first.take() {
            Some(bytes) => Poll::Ready(Some(Ok(Frame::data(bytes)))),
            None => Poll::Pending,
        }
    }
}

/// A body that says it has a size and then fails.
struct Failing;

impl HttpBody for Failing {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, std::io::Error>>> {
        Poll::Ready(Some(Err(std::io::Error::other("secret text"))))
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        hyper::body::SizeHint::with_exact(5)
    }
}

#[tokio::test]
async fn a_caller_holding_the_tag_gets_a_304_with_no_body() {
    let app = stack(tagged("\"v1\""));
    let response = fetch(&app, "/", &[("if-none-match", "\"v1\"")]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(etag_of(&response), "\"v1\"");
    assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
    assert!(body_of(response).await.is_empty());
}

#[tokio::test]
async fn a_caller_with_an_old_tag_or_none_gets_the_whole_answer() {
    let app = stack(tagged("\"v2\""));
    for headers in [&[("if-none-match", "\"v1\"")][..], &[]] {
        let response = fetch(&app, "/", headers).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(etag_of(&response), "\"v2\"");
        assert_eq!(body_of(response).await, LONG);
    }
}

#[tokio::test]
async fn lists_and_weak_tags_compare_weakly() {
    let app = stack(tagged("W/\"v2\""));
    for held in [
        "W/\"v2\"",
        "\"v2\"",
        "\"v1\", W/\"v2\"",
        "\"x,y\", \"v2\"",
        "*",
    ] {
        let response = fetch(&app, "/", &[("if-none-match", held)]).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{held}");
    }
    for held in ["v2", "\"v2", "\"v1\", junk", "\"x,v2\""] {
        let response = fetch(&app, "/", &[("if-none-match", held)]).await;
        assert_eq!(response.status(), StatusCode::OK, "{held}");
    }
}

#[tokio::test]
async fn two_header_lines_read_as_one_list() {
    let app = stack(tagged("\"v2\""));
    let request = Request::get("/")
        .header("if-none-match", "\"v1\"")
        .header("if-none-match", "\"v2\"")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn a_star_matches_an_answer_with_no_tag() {
    let app = stack(Router::new().route("/", get(|| async { "plain" })));
    let response = fetch(&app, "/", &[("if-none-match", "*")]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert!(!response.headers().contains_key(header::ETAG));
    let response = fetch(&app, "/", &[("if-none-match", "\"v1\"")]).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_304_keeps_what_the_200_said_and_nothing_about_a_body() {
    async fn handler() -> Response {
        let mut response = LONG.into_response();
        let headers = response.headers_mut();
        for (name, value) in [
            ("etag", "\"v1\""),
            ("cache-control", "private, max-age=60"),
            ("vary", "accept-language"),
            ("expires", "Sat, 07 Nov 2026 10:00:00 GMT"),
            ("content-location", "/elsewhere"),
            ("last-modified", "Sat, 07 Nov 2026 09:00:00 GMT"),
            ("set-cookie", "a=1"),
            ("x-total-count", "12"),
            ("content-language", "en"),
            ("content-disposition", "attachment"),
            ("content-length", "74"),
        ] {
            headers.insert(name, HeaderValue::from_static(value));
        }
        response
    }

    let app = stack(Router::new().route("/", get(handler)));
    let response = fetch(&app, "/", &[("if-none-match", "\"v1\"")]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    let headers = response.headers();
    for name in [
        "etag",
        "cache-control",
        "vary",
        "expires",
        "content-location",
        "last-modified",
        "set-cookie",
        "x-total-count",
    ] {
        assert!(headers.contains_key(name), "{name} went missing");
    }
    for name in [
        "content-type",
        "content-length",
        "content-language",
        "content-disposition",
    ] {
        assert!(!headers.contains_key(name), "{name} stayed");
    }
}

#[tokio::test]
async fn a_304_gets_everything_the_stack_adds() {
    let config = Http {
        cors_origins: vec!["https://app.example".to_string()],
        hsts: true,
        hsts_max_age: 60,
        csp: Some("default-src 'none'".to_string()),
        permissions_policy: None,
        ..compressed()
    };
    let app = middleware::apply(tagged("\"v1\""), &config).unwrap();
    let response = fetch(
        &app,
        "/",
        &[
            ("if-none-match", "\"v1\""),
            ("origin", "https://app.example"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    let headers = response.headers();
    for name in [
        "x-request-id",
        "x-content-type-options",
        "x-frame-options",
        "referrer-policy",
        "strict-transport-security",
        "content-security-policy",
    ] {
        assert!(headers.contains_key(name), "{name} went missing");
    }
    assert_eq!(
        headers["access-control-allow-origin"],
        "https://app.example"
    );
    let vary: Vec<_> = headers
        .get_all(header::VARY)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert!(vary.contains(&"accept-encoding"), "{vary:?}");
    assert!(
        vary.iter().any(|value| value.contains("origin")),
        "{vary:?}"
    );
}

#[tokio::test]
async fn a_page_on_a_listed_origin_can_read_the_tag() {
    let config = Http {
        cors_origins: vec!["https://app.example".to_string()],
        ..http()
    };
    let app = middleware::apply(tagged("\"v1\""), &config).unwrap();
    let response = fetch(&app, "/", &[("origin", "https://app.example")]).await;
    assert_eq!(response.headers()["access-control-expose-headers"], "etag");
}

#[tokio::test]
async fn a_304_has_the_vary_compression_would_have_added() {
    let app = middleware::apply(tagged("W/\"v1\""), &compressed()).unwrap();
    let gzip = ("accept-encoding", "gzip");
    let full = fetch(&app, "/", &[gzip]).await;
    assert_eq!(full.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(full.headers()[header::VARY], "accept-encoding");

    let response = fetch(&app, "/", &[gzip, ("if-none-match", "W/\"v1\"")]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(response.headers()[header::VARY], "accept-encoding");
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    assert!(body_of(response).await.is_empty());
}

#[tokio::test]
async fn a_304_is_sent_with_no_body_on_a_real_connection() {
    let routes = hashed(get(|| async { LONG }));
    let app = middleware::apply(routes, &compressed()).unwrap();
    let tag = etag_of(&fetch(&app, "/", &[]).await);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(server::serve_until(app, listener, std::future::pending()));
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    // A second request on the same connection shows where the `304` ended.
    let request = format!(
        "GET / HTTP/1.1\r\nhost: x\r\naccept-encoding: gzip\r\nif-none-match: {tag}\r\n\r\n\
         GET /again HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();

    assert!(answer.starts_with("HTTP/1.1 304 "), "{answer}");
    let (first, second) = answer.split_once("\r\n\r\n").unwrap();
    assert!(second.starts_with("HTTP/1.1 404 "), "{answer}");
    let first = first.to_ascii_lowercase();
    assert!(first.contains(&format!("etag: {tag}").to_ascii_lowercase()));
    assert!(first.contains("vary: accept-encoding"), "{answer}");
    for name in ["content-encoding", "content-length", "transfer-encoding"] {
        assert!(!first.contains(name), "{answer}");
    }
}

#[tokio::test]
async fn a_head_is_compared_like_a_get() {
    let app = stack(tagged("\"v1\""));
    let response = call(&app, Method::HEAD, "/", &[("if-none-match", "\"v1\"")]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(etag_of(&response), "\"v1\"");
}

#[tokio::test]
async fn a_write_is_never_answered_with_a_304() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counted = runs.clone();
    let write = move || {
        let counted = counted.clone();
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            ([(header::ETAG, "\"v1\"")], "written")
        }
    };
    let app = stack(Router::new().route("/", post(write.clone()).put(write)));
    for method in [Method::POST, Method::PUT] {
        let response = call(&app, method, "/", &[("if-none-match", "\"v1\"")]).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn only_a_success_is_compared() {
    for status in [
        StatusCode::SWITCHING_PROTOCOLS,
        StatusCode::MOVED_PERMANENTLY,
        StatusCode::NOT_FOUND,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let app = stack(Router::new().route(
            "/",
            get(move || async move { (status, [(header::ETAG, "\"v1\"")]) }),
        ));
        for held in ["\"v1\"", "*"] {
            let response = fetch(&app, "/", &[("if-none-match", held)]).await;
            assert_eq!(response.status(), status, "{held}");
        }
    }
    let app = stack(Router::new().route(
        "/",
        get(|| async { (StatusCode::PARTIAL_CONTENT, [(header::ETAG, "\"v1\"")]) }),
    ));
    let response = fetch(&app, "/", &[("if-none-match", "\"v1\"")]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}

async fn require_a_key(request: Request, next: Next) -> Response {
    if request.headers().contains_key("x-key") {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

#[tokio::test]
async fn a_caller_who_is_refused_gets_the_refusal_whatever_they_hold() {
    let routes = hashed(get(|| async { "secret" })).layer(from_fn(require_a_key));
    let app = stack(routes);
    let tag = etag_of(&fetch(&app, "/", &[("x-key", "1")]).await);

    let without = fetch(&app, "/", &[]).await;
    for held in [tag.as_str(), "*"] {
        let response = fetch(&app, "/", &[("if-none-match", held)]).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{held}");
        assert!(!response.headers().contains_key(header::ETAG));
        assert_eq!(response.headers().len(), without.headers().len());
    }
    let allowed = fetch(&app, "/", &[("x-key", "1"), ("if-none-match", &tag)]).await;
    assert_eq!(allowed.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn a_tag_covers_what_this_caller_may_see() {
    async fn masked(headers: HeaderMap) -> Json<Value> {
        let may_see = headers.contains_key("x-manager");
        Json(json!({ "name": "A", "salary": may_see.then_some(100) }))
    }

    let app = stack(hashed(get(masked)));
    let manager = etag_of(&fetch(&app, "/", &[("x-manager", "1")]).await);
    let response = fetch(&app, "/", &[("if-none-match", &manager)]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(etag_of(&response), manager);
}

#[tokio::test]
async fn an_answer_gets_a_weak_tag_from_its_body_and_then_a_304() {
    let app = stack(hashed(get(|| async { LONG })));
    let first = fetch(&app, "/", &[]).await;
    let tag = etag_of(&first);
    assert!(tag.starts_with("W/\""), "{tag}");
    assert_eq!(body_of(first).await, LONG);

    let again = fetch(&app, "/", &[]).await;
    assert_eq!(etag_of(&again), tag, "the same body has the same tag");

    let response = fetch(&app, "/", &[("if-none-match", &tag)]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(etag_of(&response), tag);
}

#[tokio::test]
async fn a_changed_body_has_a_new_tag() {
    let count = Arc::new(AtomicUsize::new(0));
    let app = stack(hashed(get(move || {
        let count = count.clone();
        async move { format!("{}", count.fetch_add(1, Ordering::SeqCst)) }
    })));
    let first = etag_of(&fetch(&app, "/", &[]).await);
    let response = fetch(&app, "/", &[("if-none-match", &first)]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(etag_of(&response), first);
}

#[tokio::test]
async fn a_hash_covers_only_a_gets_200() {
    let app = stack(
        Router::new()
            .route("/write", post(|| async { "written" }))
            .route("/put", put(|| async { "written" }))
            .route(
                "/created",
                get(|| async { (StatusCode::CREATED, "created") }),
            )
            .route(
                "/missing",
                get(|| async { (StatusCode::NOT_FOUND, "missing") }),
            )
            .layer(from_fn(conditional::etag)),
    );
    for (method, uri) in [
        (Method::POST, "/write"),
        (Method::PUT, "/put"),
        (Method::GET, "/created"),
        (Method::GET, "/missing"),
    ] {
        let response = call(&app, method, uri, &[]).await;
        assert!(!response.headers().contains_key(header::ETAG), "{uri}");
    }
}

#[tokio::test]
async fn a_handlers_own_tag_is_left_alone() {
    let app = stack(tagged("\"v7\"").layer(from_fn(conditional::etag)));
    let response = fetch(&app, "/", &[]).await;
    assert_eq!(etag_of(&response), "\"v7\"");
    assert_eq!(body_of(response).await, LONG);
}

#[tokio::test]
async fn a_handlers_strong_tag_is_left_alone_when_its_answer_is_compressed() {
    let app = middleware::apply(tagged("\"v7\""), &compressed()).unwrap();
    let response = fetch(&app, "/", &[("accept-encoding", "gzip")]).await;
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(etag_of(&response), "\"v7\"");
}

#[tokio::test]
async fn no_store_gets_no_generated_tag_and_a_handlers_tag_still_works() {
    let no_store = || [(header::CACHE_CONTROL, "private, no-store")];
    let app = stack(hashed(get(move || async move { (no_store(), LONG) })));
    let response = fetch(&app, "/", &[]).await;
    assert!(!response.headers().contains_key(header::ETAG));

    let app = stack(Router::new().route(
        "/",
        get(move || async move { (no_store(), [(header::ETAG, "\"v1\"")], LONG) }),
    ));
    let response = fetch(&app, "/", &[("if-none-match", "\"v1\"")]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
}

#[tokio::test]
async fn an_empty_answer_gets_no_tag() {
    let app = stack(hashed(get(|| async { StatusCode::OK })));
    let response = fetch(&app, "/", &[]).await;
    assert!(!response.headers().contains_key(header::ETAG));
}

#[tokio::test]
async fn a_head_gets_no_generated_tag_and_keeps_its_length() {
    let app = stack(hashed(get(|| async { LONG })));
    let response = call(&app, Method::HEAD, "/", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::ETAG));
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        LONG.len().to_string()
    );
}

#[tokio::test]
async fn the_tag_is_the_same_whichever_encoding_is_asked_for() {
    let routes = hashed(get(|| async { LONG }));
    let app = middleware::apply(routes, &compressed()).unwrap();
    let plain = fetch(&app, "/", &[]).await;
    assert!(!plain.headers().contains_key(header::CONTENT_ENCODING));
    let tag = etag_of(&plain);
    assert!(tag.starts_with("W/"), "encodings may share only a weak tag");

    for encoding in ["gzip", "br", "zstd"] {
        let accept = ("accept-encoding", encoding);
        let response = fetch(&app, "/", &[accept]).await;
        assert_eq!(response.headers()[header::CONTENT_ENCODING], encoding);
        assert_eq!(etag_of(&response), tag, "{encoding}");
        assert_ne!(body_of(response).await, LONG, "{encoding}");

        let held = ("if-none-match", tag.as_str());
        let response = fetch(&app, "/", &[accept, held]).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{encoding}");
    }
}

#[tokio::test]
async fn an_already_encoded_answer_is_not_hashed() {
    let app = stack(hashed(get(|| async {
        ([(header::CONTENT_ENCODING, "gzip")], LONG)
    })));
    let response = fetch(&app, "/", &[]).await;
    assert!(!response.headers().contains_key(header::ETAG));
}

async fn first_frame(response: Response) -> Bytes {
    let mut body = response.into_body();
    let frame = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
    frame.unwrap().unwrap().into_data().unwrap()
}

#[tokio::test]
async fn a_stream_is_never_buffered() {
    let endless = || async {
        Body::new(Endless {
            first: Some(Bytes::from_static(b"one")),
        })
    };
    let app = stack(hashed(get(endless)));
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        let response = fetch(&app, "/", &[]).await;
        assert!(!response.headers().contains_key(header::ETAG));
        first_frame(response).await
    });
    assert_eq!(wait.await.expect("the layer waited for the stream"), "one");
}

#[tokio::test]
async fn server_sent_events_reach_the_caller_as_they_are_sent() {
    let events = || async {
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::new(Endless {
                first: Some(Bytes::from_static(b"data: 1\n\n")),
            }),
        )
    };
    let routes = hashed(get(events));
    let app = middleware::apply(routes, &compressed()).unwrap();
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        let accept = ("accept-encoding", "gzip");
        let response = fetch(&app, "/", &[accept]).await;
        assert!(!response.headers().contains_key(header::ETAG));
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        first_frame(response).await
    });
    assert_eq!(wait.await.expect("an event was held back"), "data: 1\n\n");
}

#[tokio::test]
async fn a_body_at_the_limit_is_tagged_and_one_past_it_is_not() {
    let sized = |size: usize| async move {
        let app = stack(hashed(get(move || async move { vec![b'x'; size] })));
        fetch(&app, "/", &[]).await
    };
    let response = sized(MAX_BODY).await;
    assert!(response.headers().contains_key(header::ETAG));
    assert_eq!(body_of(response).await.len(), MAX_BODY);

    let response = sized(MAX_BODY + 1).await;
    assert!(!response.headers().contains_key(header::ETAG));
    assert_eq!(body_of(response).await.len(), MAX_BODY + 1);
}

#[tokio::test]
async fn a_body_that_fails_is_a_generic_500() {
    let app = stack(hashed(get(|| async { Body::new(Failing) })));
    let response = fetch(&app, "/", &[]).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_of(response).await;
    assert!(!String::from_utf8_lossy(&body).contains("secret"));
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "internal_server_error");
}

#[tokio::test]
async fn a_timestamp_in_the_body_never_matches_and_does_no_harm() {
    let tick = Arc::new(AtomicUsize::new(0));
    let app = stack(hashed(get(move || {
        let tick = tick.clone();
        async move { Json(json!({ "open": 3, "as_of": tick.fetch_add(1, Ordering::SeqCst) })) }
    })));
    let mut held = etag_of(&fetch(&app, "/", &[]).await);
    for _ in 0..3 {
        let response = fetch(&app, "/", &[("if-none-match", &held)]).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_ne!(etag_of(&response), held);
        held = etag_of(&response);
        assert_eq!(body_of(response).await.first(), Some(&b'{'));
    }
}

#[tokio::test]
async fn a_handler_can_leave_a_timestamp_out_of_its_tag() {
    let tick = Arc::new(AtomicUsize::new(0));
    let app = stack(Router::new().route(
        "/",
        get(move || {
            let tick = tick.clone();
            async move {
                let payload = json!({ "open": 3 });
                let tag = ETag::weak_of(&payload).unwrap();
                let mut body = payload;
                body["as_of"] = tick.fetch_add(1, Ordering::SeqCst).into();
                (tag, Json(body))
            }
        }),
    ));
    let tag = etag_of(&fetch(&app, "/", &[]).await);
    let response = fetch(&app, "/", &[("if-none-match", &tag)]).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn weak_of_gives_the_tag_the_layer_gives_the_same_json() {
    let value = json!({ "a": [1, 2], "b": "c" });
    let served = value.clone();
    let app = stack(hashed(get(move || async move { Json(served.clone()) })));
    let response = fetch(&app, "/", &[]).await;
    assert_eq!(
        etag_of(&response),
        ETag::weak_of(&value).unwrap().to_string()
    );
    assert_ne!(
        ETag::strong_of(&value).unwrap().to_string(),
        ETag::weak_of(&value).unwrap().to_string()
    );
    assert!(
        !ETag::strong_of(&value)
            .unwrap()
            .to_string()
            .starts_with("W/")
    );
}

/// Keeps the first three rows, and takes the tag away when that leaves rows out.
async fn page(request: Request, next: Next) -> Response {
    let (mut parts, body) = next.run(request).await.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    let rows: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
    if rows.len() > 3 {
        parts.headers.remove(header::ETAG);
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    let page = serde_json::to_vec(&rows[..rows.len().min(3)]).unwrap();
    Response::from_parts(parts, Body::from(page))
}

#[tokio::test]
async fn a_layer_that_takes_the_tag_away_means_no_304() {
    let list = |rows: usize| {
        Router::new()
            .route(
                "/",
                get(move || async move {
                    let tag = ETag::weak("list").unwrap();
                    (tag, Json(vec![1; rows]))
                }),
            )
            .layer(from_fn(page))
    };
    let held = ("if-none-match", "W/\"list\"");
    let whole = fetch(&stack(list(2)), "/", &[held]).await;
    assert_eq!(whole.status(), StatusCode::NOT_MODIFIED);
    let part = fetch(&stack(list(5)), "/", &[held]).await;
    assert_eq!(part.status(), StatusCode::OK);
    assert_eq!(body_of(part).await, "[1,1,1]");
}

#[tokio::test]
async fn a_hash_outside_paging_covers_the_page() {
    let rows = Arc::new(AtomicUsize::new(4));
    let served = rows.clone();
    let app = stack(
        Router::new()
            .route(
                "/",
                get(move || {
                    let rows = served.load(Ordering::SeqCst);
                    async move { Json(vec![1; rows]) }
                }),
            )
            .layer(from_fn(page))
            .layer(from_fn(conditional::etag)),
    );
    let tag = etag_of(&fetch(&app, "/", &[]).await);
    rows.store(5, Ordering::SeqCst);
    let response = fetch(&app, "/", &[("if-none-match", &tag)]).await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_MODIFIED,
        "the page is the same"
    );
}

#[tokio::test]
async fn one_route_can_opt_in_alone() {
    let app = stack(
        Router::new()
            .route(
                "/tagged",
                get(|| async { LONG }).layer(from_fn(conditional::etag)),
            )
            .route("/plain", get(|| async { LONG })),
    );
    let tagged = fetch(&app, "/tagged", &[]).await;
    assert!(tagged.headers().contains_key(header::ETAG));
    let plain = fetch(&app, "/plain", &[]).await;
    assert!(!plain.headers().contains_key(header::ETAG));
}

/// A record at version 7 that `If-Match` guards.
fn guarded(
    check: fn(&HeaderMap, &ETag) -> Result<()>,
    current: fn() -> ETag,
) -> (Router, Arc<AtomicUsize>) {
    let writes = Arc::new(AtomicUsize::new(0));
    let counted = writes.clone();
    let write = move |headers: HeaderMap| {
        let counted = counted.clone();
        async move {
            check(&headers, &current())?;
            counted.fetch_add(1, Ordering::SeqCst);
            Ok::<_, axa::Error>(StatusCode::NO_CONTENT)
        }
    };
    (stack(Router::new().route("/", put(write))), writes)
}

fn version() -> ETag {
    ETag::strong("v7").unwrap()
}

async fn assert_problem(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let body = body_of(response).await;
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["status"], status.as_u16());
    assert_eq!(problem["code"], code);
    assert!(problem["detail"].is_string());
}

#[tokio::test]
async fn a_write_with_the_current_tag_goes_through() {
    let (app, writes) = guarded(conditional::if_match, version);
    for sent in ["\"v7\"", "\"v6\", \"v7\"", "*"] {
        let response = call(&app, Method::PUT, "/", &[("if-match", sent)]).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{sent}");
    }
    assert_eq!(writes.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_stale_write_is_a_412_problem_and_writes_nothing() {
    let (app, writes) = guarded(conditional::if_match, version);
    for sent in ["\"v6\"", "W/\"v7\"", "v7", "\"v7\" junk", "*, \"v7\""] {
        let response = call(&app, Method::PUT, "/", &[("if-match", sent)]).await;
        assert_problem(
            response,
            StatusCode::PRECONDITION_FAILED,
            "precondition_failed",
        )
        .await;
    }
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_weak_current_tag_never_passes_if_match() {
    let (app, writes) = guarded(conditional::if_match, || ETag::weak("v7").unwrap());
    for sent in ["W/\"v7\"", "\"v7\""] {
        let response = call(&app, Method::PUT, "/", &[("if-match", sent)]).await;
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED, "{sent}");
    }
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    let response = call(&app, Method::PUT, "/", &[("if-match", "*")]).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn if_match_is_optional_unless_required() {
    let (app, writes) = guarded(conditional::if_match, version);
    let response = call(&app, Method::PUT, "/", &[]).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(writes.load(Ordering::SeqCst), 1);

    let (app, writes) = guarded(conditional::require_if_match, version);
    let response = call(&app, Method::PUT, "/", &[]).await;
    assert_problem(
        response,
        StatusCode::PRECONDITION_REQUIRED,
        "precondition_required",
    )
    .await;
    assert_eq!(writes.load(Ordering::SeqCst), 0);

    let response = call(&app, Method::PUT, "/", &[("if-match", "\"v6\"")]).await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let response = call(&app, Method::PUT, "/", &[("if-match", "\"v7\"")]).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[test]
fn a_service_can_give_its_own_answer_to_a_stale_write() {
    let current = version();
    let mut headers = HeaderMap::new();
    assert_eq!(conditional::if_match_holds(&headers, &current), None);
    headers.insert(header::IF_MATCH, HeaderValue::from_static("\"v7\""));
    assert_eq!(conditional::if_match_holds(&headers, &current), Some(true));
    headers.insert(header::IF_MATCH, HeaderValue::from_static("\"v6\""));
    assert_eq!(conditional::if_match_holds(&headers, &current), Some(false));
    headers.insert(header::IF_MATCH, HeaderValue::from_static("not a tag"));
    assert_eq!(conditional::if_match_holds(&headers, &current), Some(false));
}
