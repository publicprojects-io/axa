//! The address of the client behind a request.
//!
//! `X-Forwarded-For` is only what the caller and each proxy wrote. Its first entry is
//! whatever the caller chose, so reading it records a forged address as evidence.
//! Each proxy appends what it saw, so the entries can be trusted from the right, for
//! as long as the machine that wrote each one is a proxy this service trusts.
//!
//! List those proxies in `TRUSTED_PROXIES`. With none listed, the header is ignored.
//!
//! [`crate::middleware::apply`] resolves the address for each request served by
//! [`crate::server`], and a handler reads it with [`ClientAddr`]. It is also a plain
//! [`IpAddr`] request extension, for a layer that does not depend on this crate.
//!
//! It stays out of the request span on purpose. An address is personal data, so it
//! is recorded where a service chooses to, such as an audit row.

use std::{
    convert::Infallible,
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderMap, HeaderName, request::Parts},
    middleware::Next,
    response::Response,
};

/// The header proxies record addresses in.
const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");

/// The most entries read, so that a long header costs a bounded amount of work.
const MAX_HOPS: usize = 32;

/// A proxy's address, or a range of them, as in `10.0.0.1` or `10.0.0.0/8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyNet {
    network: IpAddr,
    prefix: u8,
}

impl ProxyNet {
    /// Whether `address` is in this range.
    #[must_use]
    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.network, address.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                masked(u128::from(network.to_bits()), 32, self.prefix)
                    == masked(u128::from(address.to_bits()), 32, self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                masked(network.to_bits(), 128, self.prefix)
                    == masked(address.to_bits(), 128, self.prefix)
            }
            _ => false,
        }
    }
}

/// `bits` of `width`, keeping the leading `prefix`.
const fn masked(bits: u128, width: u8, prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        bits >> (width - prefix)
    }
}

/// Why a [`ProxyNet`] could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidProxyNet;

impl fmt::Display for InvalidProxyNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not an address or CIDR range")
    }
}

impl std::error::Error for InvalidProxyNet {}

impl FromStr for ProxyNet {
    type Err = InvalidProxyNet;

    fn from_str(text: &str) -> Result<Self, InvalidProxyNet> {
        let (address, prefix) = text
            .split_once('/')
            .map_or((text, None), |(a, p)| (a, Some(p)));
        let network = address
            .parse::<IpAddr>()
            .map_err(|_| InvalidProxyNet)?
            .to_canonical();
        let width = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.parse::<u8>().map_err(|_| InvalidProxyNet)?,
            None => width,
        };
        if prefix > width {
            return Err(InvalidProxyNet);
        }
        Ok(Self { network, prefix })
    }
}

impl fmt::Display for ProxyNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

/// The client's address, given the machine that connected.
///
/// An untrusted `peer` is the answer, whatever its headers say. A trusted one vouches
/// for the rightmost entry, and so on leftwards while each entry is trusted. An entry
/// that cannot be read ends the walk at the last address known to be good.
#[must_use]
pub fn resolve(peer: IpAddr, headers: &HeaderMap, trusted: &[ProxyNet]) -> IpAddr {
    let is_trusted = |address: IpAddr| trusted.iter().any(|net| net.contains(address));
    let mut client = peer.to_canonical();
    if !is_trusted(client) {
        return client;
    }
    // Several headers read as one list, in order, so the last header's last entry is
    // the nearest. Reading newest first stops a long header at the near end.
    let values = headers.get_all(&X_FORWARDED_FOR);
    let entries = values
        .iter()
        .rev()
        .map(|value| value.to_str().unwrap_or(""))
        .flat_map(|value| value.rsplit(','));
    for entry in entries.take(MAX_HOPS) {
        let Ok(address) = entry.trim().parse::<IpAddr>() else {
            break;
        };
        client = address.to_canonical();
        if !is_trusted(client) {
            break;
        }
    }
    client
}

/// The client's address, from the connection and any trusted proxies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddr(pub Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for ClientAddr {
    type Rejection = Infallible;

    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Infallible> {
        Ok(Self(parts.extensions.get::<IpAddr>().copied()))
    }
}

/// Puts the client's address on the request. Without `ConnectInfo`, as in a test that
/// calls the router directly, there is no address to put.
pub(crate) async fn record(
    State(trusted): State<std::sync::Arc<Vec<ProxyNet>>>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    if let Some(peer) = peer {
        let client = resolve(peer, request.headers(), &trusted);
        request.extensions_mut().insert(client);
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    const PEER: &str = "10.0.0.1";

    fn trusted() -> Vec<ProxyNet> {
        ["10.0.0.0/8", "fd00::/8"]
            .iter()
            .map(|net| net.parse().unwrap())
            .collect()
    }

    fn resolved(peer: &str, forwarded: &[&str]) -> String {
        let mut headers = HeaderMap::new();
        for value in forwarded {
            headers.append(&X_FORWARDED_FOR, HeaderValue::from_str(value).unwrap());
        }
        resolve(peer.parse().unwrap(), &headers, &trusted()).to_string()
    }

    #[test]
    fn with_no_proxies_the_header_is_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert(&X_FORWARDED_FOR, HeaderValue::from_static("1.2.3.4"));
        let peer = PEER.parse().unwrap();
        assert_eq!(resolve(peer, &headers, &[]).to_string(), PEER);
    }

    #[test]
    fn a_caller_that_is_no_proxy_cannot_name_an_address() {
        assert_eq!(resolved("203.0.113.9", &["1.2.3.4"]), "203.0.113.9");
    }

    #[test]
    fn the_rightmost_untrusted_entry_is_the_client() {
        assert_eq!(resolved(PEER, &["198.51.100.7"]), "198.51.100.7");
        assert_eq!(resolved(PEER, &["198.51.100.7, 10.0.0.2"]), "198.51.100.7");
    }

    #[test]
    fn an_entry_the_caller_wrote_is_not_believed() {
        // The caller sent 1.2.3.4, and the proxy appended the caller's real address.
        assert_eq!(resolved(PEER, &["1.2.3.4, 198.51.100.7"]), "198.51.100.7");
    }

    #[test]
    fn repeated_headers_read_as_one_list() {
        assert_eq!(resolved(PEER, &["1.2.3.4", "198.51.100.7"]), "198.51.100.7");
    }

    #[test]
    fn an_unreadable_entry_ends_the_walk() {
        assert_eq!(resolved(PEER, &["1.2.3.4, junk, 10.0.0.2"]), "10.0.0.2");
        assert_eq!(resolved(PEER, &["junk"]), PEER);
        assert_eq!(resolved(PEER, &[""]), PEER);
    }

    #[test]
    fn with_only_proxies_the_leftmost_is_the_client() {
        assert_eq!(resolved(PEER, &["10.0.0.3, 10.0.0.2"]), "10.0.0.3");
    }

    #[test]
    fn a_long_header_stops_at_the_limit() {
        let long = vec!["10.0.0.2"; 100].join(",");
        assert_eq!(resolved(PEER, &[&format!("1.2.3.4,{long}")]), "10.0.0.2");
    }

    #[test]
    fn an_ipv4_address_inside_ipv6_is_the_same_address() {
        assert_eq!(
            resolved("::ffff:10.0.0.1", &["198.51.100.7"]),
            "198.51.100.7"
        );
        assert_eq!(resolved("::ffff:203.0.113.9", &[]), "203.0.113.9");
    }

    #[test]
    fn ranges_read_and_match() {
        let net: ProxyNet = "192.168.1.0/24".parse().unwrap();
        assert!(net.contains("192.168.1.200".parse().unwrap()));
        assert!(!net.contains("192.168.2.1".parse().unwrap()));
        assert!(!net.contains("fd00::1".parse().unwrap()));
        let all: ProxyNet = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains("8.8.8.8".parse().unwrap()));
        let one: ProxyNet = "2001:db8::1".parse().unwrap();
        assert!(one.contains("2001:db8::1".parse().unwrap()));
        assert!(!one.contains("2001:db8::2".parse().unwrap()));
        assert_eq!(one.to_string(), "2001:db8::1/128");
    }

    #[test]
    fn a_bad_range_is_refused() {
        for text in ["", "junk", "10.0.0.0/33", "fd00::/129", "10.0.0.0/x", "/8"] {
            assert_eq!(text.parse::<ProxyNet>(), Err(InvalidProxyNet), "{text}");
        }
    }
}
