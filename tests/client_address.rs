//! The client's address, through a real server and the standard middleware stack.

use std::net::IpAddr;

use axa::{client::ClientAddr, config::Http, middleware, server};
use axum::{Extension, Router, routing::get};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

fn http(trusted_proxies: &[&str]) -> Http {
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
        trusted_proxies: trusted_proxies
            .iter()
            .map(|net| net.parse().unwrap())
            .collect(),
    }
}

/// What a handler sees for a request carrying `forwarded`, and what a layer that
/// knows only `std` sees.
async fn seen(trusted_proxies: &[&str], forwarded: &str) -> String {
    let router = Router::new().route(
        "/",
        get(
            |ClientAddr(client): ClientAddr, Extension(plain): Extension<IpAddr>| async move {
                format!("[{}|{plain}]", client.unwrap())
            },
        ),
    );
    let app = middleware::apply(router, &http(trusted_proxies)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(server::serve_until(app, listener, std::future::pending()));

    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\nX-Forwarded-For: {forwarded}\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let start = response.find('[').expect("a body");
    let end = response.find(']').expect("a body");
    response[start..=end].to_string()
}

#[tokio::test]
async fn a_handler_gets_the_address_behind_a_trusted_proxy() {
    assert_eq!(
        seen(&["127.0.0.1"], "1.2.3.4, 198.51.100.7").await,
        "[198.51.100.7|198.51.100.7]"
    );
}

#[tokio::test]
async fn with_no_trusted_proxy_a_forged_header_changes_nothing() {
    assert_eq!(seen(&[], "1.2.3.4").await, "[127.0.0.1|127.0.0.1]");
}
