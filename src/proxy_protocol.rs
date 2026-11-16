//! The PROXY protocol, versions 1 and 2, which a layer-4 load balancer sends at the start
//! of each connection to say whom the connection is from.
//!
//! [`crate::server`] reads it only from the ranges in `PROXY_PROTOCOL_FROM`, since anyone
//! else could write one. A connection from those ranges must start with a header, and one
//! that does not is closed. A connection from anywhere else is served as it is.

use std::{io, net::SocketAddr};

use ppp::{v1, v2};
use tokio::io::{AsyncRead, AsyncReadExt};

/// The longest version 1 header, as the protocol sets it.
const V1_MAX: usize = 107;

/// The fixed part of a version 2 header: the prefix, version and command, family, and
/// length.
const V2_FIXED: usize = 16;

/// The most bytes of addresses and extensions read after a version 2 header's fixed part.
/// Load balancers send a few dozen, so a longer one is refused rather than read.
pub(crate) const V2_MAX_REST: usize = 4096;

/// Read a PROXY header from the start of `stream`, and answer the client's address. A
/// header that names no client, as for a load balancer's own health check, answers
/// `peer`, the address that connected.
///
/// It reads exactly the header, so the bytes after it are left for HTTP.
///
/// # Errors
///
/// The connection ended, or did not start with a header this can read.
pub(crate) async fn read_header<S: AsyncRead + Unpin>(
    stream: &mut S,
    peer: SocketAddr,
) -> io::Result<SocketAddr> {
    // Both versions' headers are at least this long, and it is all of version 2's prefix.
    let mut start = [0_u8; 12];
    stream.read_exact(&mut start).await?;
    if start == v2::PROTOCOL_PREFIX {
        let mut header = Vec::from(start);
        header.resize(V2_FIXED, 0);
        stream.read_exact(&mut header[start.len()..]).await?;
        let rest = usize::from(u16::from_be_bytes([header[14], header[15]]));
        if rest > V2_MAX_REST {
            return Err(invalid(format!(
                "a PROXY v2 header of more than {V2_MAX_REST} bytes"
            )));
        }
        header.resize(V2_FIXED + rest, 0);
        stream.read_exact(&mut header[V2_FIXED..]).await?;
        let header = v2::Header::try_from(header.as_slice())
            .map_err(|error| invalid(format!("a bad PROXY v2 header: {error}")))?;
        return Ok(match (header.command, header.addresses) {
            (v2::Command::Proxy, v2::Addresses::IPv4(addresses)) => {
                SocketAddr::from((addresses.source_address, addresses.source_port))
            }
            (v2::Command::Proxy, v2::Addresses::IPv6(addresses)) => {
                SocketAddr::from((addresses.source_address, addresses.source_port))
            }
            // `LOCAL` is the balancer speaking for itself, and an unspecified or Unix
            // address names no client.
            _ => peer,
        });
    }
    if !start.starts_with(b"PROXY ") {
        return Err(invalid("no PROXY header".to_string()));
    }
    // A byte at a time, so as not to read past the line into the request.
    let mut header = Vec::from(start);
    while !header.ends_with(b"\r\n") {
        if header.len() >= V1_MAX {
            return Err(invalid(format!(
                "a PROXY v1 header of more than {V1_MAX} bytes"
            )));
        }
        header.push(stream.read_u8().await?);
    }
    let header = v1::Header::try_from(header.as_slice())
        .map_err(|error| invalid(format!("a bad PROXY v1 header: {error}")))?;
    Ok(match header.addresses {
        v1::Addresses::Tcp4(addresses) => {
            SocketAddr::from((addresses.source_address, addresses.source_port))
        }
        v1::Addresses::Tcp6(addresses) => {
            SocketAddr::from((addresses.source_address, addresses.source_port))
        }
        v1::Addresses::Unknown => peer,
    })
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str = "10.0.0.9:40000";

    async fn read(bytes: &[u8]) -> (io::Result<SocketAddr>, Vec<u8>) {
        let mut stream = bytes;
        let read = read_header(&mut stream, PEER.parse().unwrap()).await;
        (read, stream.to_vec())
    }

    fn v2(command: u8, family: u8, rest: &[u8]) -> Vec<u8> {
        let mut header = Vec::from(v2::PROTOCOL_PREFIX);
        header.push(0x20 | command);
        header.push(family);
        header.extend(u16::try_from(rest.len()).unwrap().to_be_bytes());
        header.extend(rest);
        header
    }

    #[tokio::test]
    async fn a_v1_header_names_the_client_and_leaves_the_request() {
        let (client, left) =
            read(b"PROXY TCP4 192.0.2.7 10.0.0.1 51000 1241\r\nGET / HTTP/1.1\r\n").await;
        assert_eq!(client.unwrap(), "192.0.2.7:51000".parse().unwrap());
        assert_eq!(left, b"GET / HTTP/1.1\r\n");
        let (client, _) = read(b"PROXY TCP6 2001:db8::7 2001:db8::1 51000 1241\r\n").await;
        assert_eq!(client.unwrap(), "[2001:db8::7]:51000".parse().unwrap());
    }

    #[tokio::test]
    async fn a_v2_header_names_the_client_and_leaves_the_request() {
        let mut bytes = v2(
            1,
            0x11,
            &[
                192, 0, 2, 7, 10, 0, 0, 1, 0xc7, 0x38, 0x04, 0xd9, 0x04, 0, 1, 42,
            ],
        );
        bytes.extend(b"GET / HTTP/1.1\r\n");
        let (client, left) = read(&bytes).await;
        assert_eq!(client.unwrap(), "192.0.2.7:51000".parse().unwrap());
        assert_eq!(left, b"GET / HTTP/1.1\r\n");
    }

    #[tokio::test]
    async fn a_header_that_names_no_client_keeps_the_peer() {
        let peer: SocketAddr = PEER.parse().unwrap();
        assert_eq!(read(b"PROXY UNKNOWN\r\n").await.0.unwrap(), peer);
        // The balancer's own health check.
        assert_eq!(read(&v2(0, 0, &[])).await.0.unwrap(), peer);
    }

    #[tokio::test]
    async fn a_missing_long_or_bad_header_is_refused() {
        assert!(read(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.0.is_err());
        assert!(read(b"PROXY TCP4 192.0.2.7").await.0.is_err());
        let long = format!("PROXY UNKNOWN {}\r\n", "x".repeat(V1_MAX));
        assert!(read(long.as_bytes()).await.0.is_err());
        assert!(
            read(b"PROXY TCP4 not-an-address 10.0.0.1 1 2\r\n")
                .await
                .0
                .is_err()
        );
        let mut long = Vec::from(v2::PROTOCOL_PREFIX);
        long.extend([0x21, 0x11]);
        long.extend(u16::try_from(V2_MAX_REST + 1).unwrap().to_be_bytes());
        long.extend(vec![0; V2_MAX_REST + 1]);
        assert!(read(&long).await.0.is_err());
    }
}
