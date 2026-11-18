//! `client::resolve` with any `X-Forwarded-For` headers: an untrusted peer is always the
//! answer, and the walk never reaches past an address that is not trusted.

#![no_main]

use std::net::{IpAddr, Ipv4Addr};

use arbitrary::Arbitrary;
use axa::client::{ProxyNet, resolve};
use axum::http::{HeaderMap, HeaderValue};
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input {
    peer: [u8; 4],
    trust_peer: bool,
    headers: Vec<Vec<u8>>,
}

fuzz_target!(|input: Input| {
    let peer = IpAddr::V4(Ipv4Addr::from(input.peer));
    let trusted: Vec<ProxyNet> = if input.trust_peer {
        vec![format!("{peer}/32").parse().unwrap()]
    } else {
        Vec::new()
    };
    let mut headers = HeaderMap::new();
    for value in input.headers.iter().take(8) {
        if let Ok(value) = HeaderValue::from_bytes(value) {
            headers.append("x-forwarded-for", value);
        }
    }
    let client = resolve(peer, &headers, &trusted);
    if !input.trust_peer {
        assert_eq!(client, peer, "an untrusted peer is the client");
    }
});
