//! The client against a server on this machine, which, being loopback, it must not
//! reach unless told it may.

use std::net::SocketAddr;

use axa_http_client::{BodyError, Client, Net, Refused, bytes_limited, refusal};
use axum::{
    Router,
    body::Body,
    http::{StatusCode, header::LOCATION},
    response::IntoResponse,
    routing::get,
};

/// A server on 127.0.0.1, and its port.
async fn server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let to = |target: String| move || async move { (StatusCode::FOUND, [(LOCATION, target)]) };
    let app = Router::new()
        .route("/", get(|| async { "hello" }))
        .route(
            "/to-127-0-0-2",
            get(to(format!("http://127.0.0.2:{port}/"))),
        )
        .route(
            "/to-localhost",
            get(to(format!("http://localhost:{port}/"))),
        )
        .route("/to-file", get(to("file:///etc/passwd".to_string())))
        .route("/loop", get(to("/loop".to_string())))
        .route("/big", get(|| async { "x".repeat(10_000) }))
        .route(
            "/stream",
            get(|| async {
                let chunks = (0..10).map(|_| Ok::<_, std::io::Error>(vec![b'x'; 1_000]));
                Body::from_stream(futures_stream(chunks)).into_response()
            }),
        );
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    port
}

/// A stream of `items`, with no content-length.
fn futures_stream<I: Iterator<Item = Result<Vec<u8>, std::io::Error>> + Send + Unpin + 'static>(
    items: I,
) -> impl futures_core::Stream<Item = Result<Vec<u8>, std::io::Error>> + Send {
    struct Iter<I>(I);
    impl<I: Iterator + Unpin> futures_core::Stream for Iter<I> {
        type Item = I::Item;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<I::Item>> {
            std::task::Poll::Ready(self.0.next())
        }
    }
    Iter(items)
}

fn loopback() -> Client {
    let net: Net = "127.0.0.1/32".parse().unwrap();
    Client::builder().allow_nets([net]).build().unwrap()
}

/// The refusal in the error from sending `url` with `client`.
async fn refused_on_send(client: &Client, url: &str) -> Refused {
    let error = client.get(url).unwrap().send().await.unwrap_err();
    refusal(&error)
        .cloned()
        .unwrap_or_else(|| panic!("{error:?}"))
}

#[tokio::test]
async fn loopback_is_refused_by_address_and_by_name() {
    let port = server().await;
    let client = Client::builder().build().unwrap();
    assert_eq!(
        client
            .get(&format!("http://127.0.0.1:{port}/"))
            .unwrap_err(),
        Refused::Address("127.0.0.1".parse().unwrap())
    );
    assert_eq!(
        refused_on_send(&client, &format!("http://localhost:{port}/")).await,
        Refused::NoAddress
    );
}

#[tokio::test]
async fn an_allowed_range_is_reached() {
    let port = server().await;
    let response = loopback()
        .get(&format!("http://127.0.0.1:{port}/"))
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "hello");
    // `localhost` is also ::1, which is not allowed, so only 127.0.0.1 is tried.
    let response = loopback()
        .get(&format!("http://localhost:{port}/"))
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn every_redirect_is_checked() {
    let port = server().await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        refused_on_send(&loopback(), &format!("{base}/to-127-0-0-2")).await,
        Refused::Address("127.0.0.2".parse().unwrap())
    );
    // `reqwest` follows no scheme but `http` and `https`, and hands the redirect back.
    let response = loopback()
        .get(&format!("{base}/to-file"))
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let listed = Client::builder()
        .allow_nets(["127.0.0.1/32".parse().unwrap()])
        .allow_hosts(["127.0.0.1"])
        .build()
        .unwrap();
    assert_eq!(
        refused_on_send(&listed, &format!("{base}/to-localhost")).await,
        Refused::Host
    );
    let few = Client::builder()
        .allow_nets(["127.0.0.1/32".parse().unwrap()])
        .max_redirects(2)
        .build()
        .unwrap();
    assert_eq!(
        refused_on_send(&few, &format!("{base}/loop")).await,
        Refused::TooManyRedirects
    );
    let none = Client::builder()
        .allow_nets(["127.0.0.1/32".parse().unwrap()])
        .max_redirects(0)
        .build()
        .unwrap();
    let response = none
        .get(&format!("{base}/to-127-0-0-2"))
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn a_body_is_refused_past_its_limit_with_or_without_a_length() {
    let port = server().await;
    let client = loopback();
    for path in ["big", "stream"] {
        let url = format!("http://127.0.0.1:{port}/{path}");
        let response = client.get(&url).unwrap().send().await.unwrap();
        assert!(
            matches!(
                bytes_limited(response, 1_000).await,
                Err(BodyError::TooLarge)
            ),
            "{path}"
        );
        let response = client.get(&url).unwrap().send().await.unwrap();
        assert_eq!(
            bytes_limited(response, 10_000).await.unwrap().len(),
            10_000,
            "{path}"
        );
    }
}
