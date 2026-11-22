//! An outbound HTTP client that will not be turned on the network it runs in.
//!
//! A service that fetches a URL someone else chose, such as a webhook receiver, an
//! avatar, or a link preview, can be made to fetch `http://169.254.169.254/` and hand
//! back the cloud's credentials, or to reach a database or admin page that trusts the
//! private network. This is server-side request forgery. [`Client`] refuses any address
//! that is not public: private, loopback, link-local and so the metadata service,
//! carrier-grade NAT, documentation, multicast, and reserved ranges, IPv4 inside IPv6
//! included.
//!
//! ```no_run
//! # async fn fetch() -> Result<(), Box<dyn std::error::Error>> {
//! let client = axa_http_client::Client::builder().https_only(true).build()?;
//! let response = client.get("https://example.com/hook")?.send().await?;
//! let body = axa_http_client::bytes_limited(response, 1_000_000).await?;
//! # let _ = body;
//! # Ok(())
//! # }
//! ```
//!
//! It checks where it connects, not only what the URL says:
//!
//! - A host name is looked up by the client's own resolver, which drops every address
//!   that is not allowed and fails when none is left. The connection goes only to the
//!   addresses it kept, so a name that answers differently a moment later, as in DNS
//!   rebinding, changes nothing.
//! - An address written in the URL, in any form the URL standard reads, such as
//!   `http://2130706433/` or `http://[::ffff:127.0.0.1]/`, is checked before sending.
//! - Every redirect is checked the same way, and one from `https` to `http` is refused.
//! - No proxy is used, as one from the environment would make the connection for it.
//!
//! [`Builder::allow_hosts`] limits where it may go at all, and [`Builder::allow_nets`]
//! lets it reach private ranges a service does mean to reach.
//!
//! TLS is rustls on ring, which [`Builder::build`] installs as the process's provider
//! unless the service installed one first. Certificates are checked against the
//! system's trust store, so a container image needs its CA certificates.

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use bytes::{Bytes, BytesMut};
pub use reqwest;
use reqwest::{
    Method, RequestBuilder, Url,
    dns::{Addrs, Name, Resolve, Resolving},
    redirect,
};

/// The most redirects followed, unless the builder says otherwise.
pub const DEFAULT_MAX_REDIRECTS: usize = 5;

/// How long to wait for a connection, unless the builder says otherwise.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a whole request may take, unless the builder says otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a request was refused before anything was sent to where it pointed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refused {
    /// The URL does not parse, or has no host.
    Url,
    /// The scheme is not `https`, or `http` where that is allowed.
    Scheme,
    /// The host is not one [`Builder::allow_hosts`] lists.
    Host,
    /// The address is not public, and no [`Builder::allow_nets`] range holds it.
    Address(IpAddr),
    /// No address the host name has is allowed, or it has none.
    NoAddress,
    /// A redirect from `https` to `http`.
    Downgrade,
    /// More redirects than the builder allows.
    TooManyRedirects,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url => f.write_str("the URL is not one with a host"),
            Self::Scheme => f.write_str("the URL's scheme is not allowed"),
            Self::Host => f.write_str("the URL's host is not on the allow-list"),
            Self::Address(address) => write!(f, "{address} is not a public address"),
            Self::NoAddress => f.write_str("the host has no address that may be reached"),
            Self::Downgrade => f.write_str("a redirect from https to http"),
            Self::TooManyRedirects => f.write_str("too many redirects"),
        }
    }
}

impl std::error::Error for Refused {}

/// The refusal inside `error`, from a request this crate's [`Client`] refused while it
/// ran: a host name with no allowed address, or a redirect.
#[must_use]
pub fn refusal<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a Refused> {
    let mut source = Some(error);
    while let Some(error) = source {
        if let Some(refused) = error.downcast_ref::<Refused>() {
            return Some(refused);
        }
        source = error.source();
    }
    None
}

/// An address range, as `10.0.0.0/8` or `fd00::/8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Net {
    address: IpAddr,
    prefix: u8,
}

impl Net {
    /// The range of `address` and the `prefix` bits after it, or `None` when the prefix
    /// is longer than the address.
    #[must_use]
    pub fn new(address: IpAddr, prefix: u8) -> Option<Self> {
        let bits = if address.is_ipv4() { 32 } else { 128 };
        (prefix <= bits).then_some(Self { address, prefix })
    }

    /// Whether `address` is in the range. An IPv4 address inside IPv6 counts as IPv4.
    #[must_use]
    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.address, canonical(address)) {
            (IpAddr::V4(net), IpAddr::V4(address)) => {
                let mask = 32_u32
                    .checked_sub(u32::from(self.prefix))
                    .and_then(|shift| u32::MAX.checked_shl(shift))
                    .unwrap_or(0); // ARI-8
                u32::from(net) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(address)) => {
                let mask = 128_u32
                    .checked_sub(u32::from(self.prefix))
                    .and_then(|shift| u128::MAX.checked_shl(shift))
                    .unwrap_or(0); // ARI-8
                u128::from(net) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }
}

/// Why a range did not parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetError;

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an address range is an address, or an address, `/`, and a prefix length")
    }
}

impl std::error::Error for NetError {}

impl std::str::FromStr for Net {
    type Err = NetError;

    fn from_str(text: &str) -> Result<Self, NetError> {
        let (address, prefix) = match text.trim().split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text.trim(), None),
        };
        let address: IpAddr = address.parse().map_err(|_| NetError)?;
        let prefix = match prefix {
            Some(prefix) => prefix.parse().map_err(|_| NetError)?,
            None if address.is_ipv4() => 32,
            None => 128,
        };
        Self::new(address, prefix).ok_or(NetError)
    }
}

/// `address`, as IPv4 when it is IPv4 written as IPv6.
fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 @ IpAddr::V4(_) => v4,
    }
}

/// Whether `address` is one anybody on the internet could reach: none of the ranges
/// IANA keeps for private, loopback, link-local, shared, documentation, benchmarking,
/// multicast, or future use.
#[must_use]
pub fn is_public(address: IpAddr) -> bool {
    match canonical(address) {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0                                    // "this network", 0.0.0.0/8
        || a == 10                              // private
        || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT, 100.64.0.0/10
        || a == 127                             // loopback
        || (a == 169 && b == 254)               // link-local, and the metadata service
        || (a == 172 && (16..32).contains(&b))  // private
        || (a == 192 && b == 0 && c == 0)       // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)       // documentation
        || (a == 192 && b == 88 && c == 99)     // 6to4 relay anycast
        || (a == 192 && b == 168)               // private
        || (a == 198 && (b == 18 || b == 19))   // benchmarking
        || (a == 198 && b == 51 && c == 100)    // documentation
        || (a == 203 && b == 0 && c == 113)     // documentation
        || a >= 224) // multicast, reserved, and broadcast
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    // Only 2000::/3 is global unicast. Everything else is loopback, unspecified,
    // IPv4-compatible, link-local, unique local, multicast, or not yet assigned.
    if segments[0] & 0xe000 != 0x2000 {
        // NAT64, 64:ff9b::/96, reaches the IPv4 address in its last 32 bits.
        if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
            let [.., high, low] = segments;
            return is_public_v4(Ipv4Addr::from((u32::from(high) << 16) | u32::from(low)));
        }
        return false;
    }
    match segments[0] {
        // IETF protocol assignments and Teredo, 2001::/23, which can hide any IPv4
        // address, and documentation, 2001:db8::/32.
        0x2001 => segments[1] >= 0x0200 && segments[1] != 0x0db8,
        // 6to4, 2002::/16, reaches the IPv4 address in its next 32 bits.
        0x2002 => is_public_v4(Ipv4Addr::from(
            (u32::from(segments[1]) << 16) | u32::from(segments[2]),
        )),
        // Documentation, 3fff::/20.
        0x3fff => segments[1] >= 0x1000,
        _ => true,
    }
}

/// What may be reached.
#[derive(Debug)]
struct Policy {
    hosts: Option<Vec<String>>,
    nets: Vec<Net>,
    https_only: bool,
    max_redirects: usize,
}

impl Policy {
    fn permits(&self, address: IpAddr) -> bool {
        is_public(address) || self.nets.iter().any(|net| net.contains(address))
    }

    fn host_listed(&self, host: &str) -> bool {
        let Some(hosts) = &self.hosts else {
            return true;
        };
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        hosts.iter().any(|listed| match listed.strip_prefix("*.") {
            Some(suffix) => host
                .strip_suffix(suffix)
                .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1),
            None => *listed == host,
        })
    }

    /// Check what a URL says, before it is sent to or redirected to.
    fn check(&self, url: &Url) -> Result<(), Refused> {
        match url.scheme() {
            "https" => {}
            "http" if !self.https_only => {}
            _ => return Err(Refused::Scheme),
        }
        let host = url
            .host_str()
            .filter(|host| !host.is_empty())
            .ok_or(Refused::Url)?;
        if !self.host_listed(host.trim_start_matches('[').trim_end_matches(']')) {
            return Err(Refused::Host);
        }
        // The URL parser writes an address in its one standard form, whatever form it
        // was given in, so this reads every address a URL can hold.
        let address = match host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
        {
            Some(v6) => v6.parse::<Ipv6Addr>().ok().map(IpAddr::V6),
            None => host.parse::<Ipv4Addr>().ok().map(IpAddr::V4),
        };
        match address {
            Some(address) if !self.permits(address) => Err(Refused::Address(address)),
            _ => Ok(()),
        }
    }
}

impl Policy {
    /// Why a redirect to `url`, after asking for each of `previous` in turn, is
    /// refused, or `None` when it may be followed.
    fn redirect_refusal(&self, url: &Url, previous: &[Url]) -> Option<Refused> {
        // `previous` holds every URL asked for so far, the first among them.
        if previous.len() > self.max_redirects {
            return Some(Refused::TooManyRedirects);
        }
        let from_https = previous.last().is_some_and(|last| last.scheme() == "https");
        if from_https && url.scheme() == "http" {
            return Some(Refused::Downgrade);
        }
        self.check(url).err()
    }
}

/// Looks host names up, and keeps only the addresses the policy allows.
struct Resolver(Arc<Policy>);

impl Resolve for Resolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.0.clone();
        Box::pin(async move {
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let kept: Vec<SocketAddr> = found
                .filter(|address| policy.permits(address.ip()))
                .collect();
            if kept.is_empty() {
                return Err(Refused::NoAddress.into());
            }
            let addresses: Addrs = Box::new(kept.into_iter());
            Ok(addresses)
        })
    }
}

/// Builds a [`Client`].
#[derive(Debug)]
pub struct Builder {
    hosts: Option<Vec<String>>,
    nets: Vec<Net>,
    https_only: bool,
    max_redirects: usize,
    connect_timeout: Duration,
    timeout: Duration,
    user_agent: Option<String>,
}

impl Builder {
    /// Reach only these hosts: a name, lower case, or `*.` and a name for any name
    /// under it but not the name itself. An address in the URL must be listed as it
    /// is written there, without brackets. Every address is still checked.
    #[must_use]
    pub fn allow_hosts<S: Into<String>>(mut self, hosts: impl IntoIterator<Item = S>) -> Self {
        let hosts = hosts
            .into_iter()
            .map(|host| host.into().trim_end_matches('.').to_ascii_lowercase());
        self.hosts = Some(hosts.collect());
        self
    }

    /// Let these ranges be reached although they are not public, as for a service on
    /// the private network the caller means to reach. The metadata service is in
    /// `169.254.0.0/16`: never list a range that holds it unless that is meant.
    #[must_use]
    pub fn allow_nets(mut self, nets: impl IntoIterator<Item = Net>) -> Self {
        self.nets.extend(nets);
        self
    }

    /// Refuse `http`, as a production service should. Off by default.
    #[must_use]
    pub fn https_only(mut self, https_only: bool) -> Self {
        self.https_only = https_only;
        self
    }

    /// Follow at most this many redirects, 5 by default. 0 follows none, and hands
    /// back the redirect as the response.
    #[must_use]
    pub fn max_redirects(mut self, max_redirects: usize) -> Self {
        self.max_redirects = max_redirects;
        self
    }

    /// How long to wait for a connection, 5 seconds by default.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// How long a whole request may take, 30 seconds by default.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The `user-agent` to send.
    #[must_use]
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    /// The client.
    ///
    /// # Errors
    ///
    /// Fails when `reqwest` cannot build its client, as when TLS cannot start.
    pub fn build(self) -> Result<Client, reqwest::Error> {
        let policy = Arc::new(Policy {
            hosts: self.hosts,
            nets: self.nets,
            https_only: self.https_only,
            max_redirects: self.max_redirects,
        });
        let redirects = policy.clone();
        let redirect = redirect::Policy::custom(move |attempt| {
            if redirects.max_redirects == 0 {
                return attempt.stop();
            }
            match redirects.redirect_refusal(attempt.url(), attempt.previous()) {
                Some(refused) => attempt.error(refused),
                None => attempt.follow(),
            }
        });
        // reqwest takes the process's rustls provider and has none of its own. Ring,
        // unless the service installed another first, which `install_default` keeps.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut inner = reqwest::Client::builder()
            .no_proxy()
            .dns_resolver(Arc::new(Resolver(policy.clone())))
            .redirect(redirect)
            .https_only(policy.https_only)
            .connect_timeout(self.connect_timeout)
            .timeout(self.timeout);
        if let Some(user_agent) = self.user_agent {
            inner = inner.user_agent(user_agent);
        }
        Ok(Client {
            inner: inner.build()?,
            policy,
        })
    }
}

/// An HTTP client that reaches only public addresses, and the ones its builder allows.
/// Cloning it is cheap, and clones share connections.
#[derive(Debug, Clone)]
pub struct Client {
    inner: reqwest::Client,
    policy: Arc<Policy>,
}

impl Client {
    /// A builder with the defaults: any public host, `http` and `https`, 5 redirects,
    /// a 5-second connect timeout, and 30 seconds in all.
    #[must_use]
    pub fn builder() -> Builder {
        Builder {
            hosts: None,
            nets: Vec::new(),
            https_only: false,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            timeout: DEFAULT_TIMEOUT,
            user_agent: None,
        }
    }

    /// A request with `method` to `url`, once the URL is checked. A host name is
    /// checked again when it is looked up, and a refusal then is an error from `send`
    /// that [`refusal`] finds.
    ///
    /// # Errors
    ///
    /// Refuses a URL that does not parse or has no host, a scheme that is not allowed,
    /// a host the allow-list does not hold, and an address that is not allowed.
    pub fn request(&self, method: Method, url: &str) -> Result<RequestBuilder, Refused> {
        let url = Url::parse(url).map_err(|_| Refused::Url)?;
        self.policy.check(&url)?;
        Ok(self.inner.request(method, url))
    }

    /// A `GET` of `url`. See [`Client::request`].
    ///
    /// # Errors
    ///
    /// As [`Client::request`].
    pub fn get(&self, url: &str) -> Result<RequestBuilder, Refused> {
        self.request(Method::GET, url)
    }

    /// A `POST` to `url`. See [`Client::request`].
    ///
    /// # Errors
    ///
    /// As [`Client::request`].
    pub fn post(&self, url: &str) -> Result<RequestBuilder, Refused> {
        self.request(Method::POST, url)
    }
}

/// Why a body was not read whole.
#[derive(Debug)]
#[non_exhaustive]
pub enum BodyError {
    /// It is longer than the limit.
    TooLarge,
    /// It could not be read.
    Read(reqwest::Error),
}

impl fmt::Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("the response body is longer than its limit"),
            Self::Read(error) => write!(f, "the response body could not be read: {error}"),
        }
    }
}

impl std::error::Error for BodyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TooLarge => None,
            Self::Read(error) => Some(error),
        }
    }
}

/// The body of `response`, refused once it passes `limit` bytes, which it counts as
/// they arrive, whatever `content-length` says.
///
/// # Errors
///
/// Fails when the body is longer than `limit`, or cannot be read.
pub async fn bytes_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Bytes, BodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(limit).unwrap_or(u64::MAX))
    {
        return Err(BodyError::TooLarge);
    }
    let mut body = BytesMut::new();
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Read)? {
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|total| total > limit)
        // ARI-5: a total past the type is past the limit
        {
            return Err(BodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn only_public_addresses_are_public() {
        for address in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::1",
            "100::1",
            "2001::1",
            "2001:0:4136:e378::1",
            "2001:db8::1",
            "2002:7f00:1::1",
            "2002:a9fe:a9fe::1",
            "3fff::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "fec0::1",
            "ff02::1",
        ] {
            assert!(!is_public(ip(address)), "{address}");
        }
        for address in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "172.15.255.255",
            "172.32.0.1",
            "192.0.1.1",
            "192.169.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.255",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2001:200::1",
            "2001:4860:4860::8888",
            "2002:808:808::1",
            "2606:4700::1111",
            "3fff:1000::1",
        ] {
            assert!(is_public(ip(address)), "{address}");
        }
    }

    #[test]
    fn a_range_holds_its_addresses_and_ipv4_inside_ipv6() {
        let net: Net = "10.1.0.0/16".parse().unwrap();
        assert!(net.contains(ip("10.1.255.1")));
        assert!(net.contains(ip("::ffff:10.1.0.9")));
        assert!(!net.contains(ip("10.2.0.1")));
        let host: Net = "fd00::5".parse().unwrap();
        assert!(host.contains(ip("fd00::5")) && !host.contains(ip("fd00::6")));
        let all: Net = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains(ip("127.0.0.1")) && !all.contains(ip("::1")));
        for bad in ["10.0.0.0/33", "fd00::/129", "nope", "10.0.0.0/x"] {
            assert_eq!(bad.parse::<Net>(), Err(NetError), "{bad}");
        }
    }

    fn policy(hosts: Option<&[&str]>, nets: &[&str], https_only: bool) -> Policy {
        Policy {
            hosts: hosts.map(|hosts| hosts.iter().map(|host| (*host).to_string()).collect()),
            nets: nets.iter().map(|net| net.parse().unwrap()).collect(),
            https_only,
            max_redirects: DEFAULT_MAX_REDIRECTS,
        }
    }

    fn check(policy: &Policy, url: &str) -> Result<(), Refused> {
        policy.check(&Url::parse(url).unwrap())
    }

    #[test]
    fn an_address_in_the_url_is_checked_in_every_form_it_can_take() {
        let policy = policy(None, &[], false);
        for url in [
            "http://127.0.0.1/",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://0177.0.0.1/",
            "http://127.1/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:7f00:1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[fd00:ec2::254]/",
            "http://0.0.0.0:8080/",
        ] {
            assert!(
                matches!(check(&policy, url), Err(Refused::Address(_))),
                "{url}"
            );
        }
        assert_eq!(check(&policy, "https://8.8.8.8/"), Ok(()));
        assert_eq!(check(&policy, "https://example.com/"), Ok(()));
    }

    #[test]
    fn the_scheme_and_the_allow_list_are_checked() {
        let open = policy(None, &[], true);
        assert_eq!(check(&open, "http://example.com/"), Err(Refused::Scheme));
        for url in [
            "ftp://example.com/",
            "file:///etc/passwd",
            "gopher://example.com/",
        ] {
            assert_eq!(
                check(&policy(None, &[], false), url).unwrap_err(),
                Refused::Scheme,
                "{url}"
            );
        }
        let listed = policy(
            Some(&["api.example.com", "*.hooks.example.com", "8.8.8.8"]),
            &[],
            false,
        );
        for url in [
            "https://api.example.com/",
            "https://API.Example.com./x",
            "https://a.hooks.example.com/",
            "https://a.b.hooks.example.com/",
            "https://8.8.8.8/",
        ] {
            assert_eq!(check(&listed, url), Ok(()), "{url}");
        }
        for url in [
            "https://example.com/",
            "https://hooks.example.com/",
            "https://evilhooks.example.com/",
            "https://api.example.com.evil.com/",
            "https://8.8.4.4/",
        ] {
            assert_eq!(check(&listed, url), Err(Refused::Host), "{url}");
        }
    }

    #[test]
    fn an_allowed_range_may_be_reached_and_no_other() {
        let policy = policy(None, &["10.0.0.0/8"], false);
        assert_eq!(check(&policy, "http://10.1.2.3/"), Ok(()));
        assert_eq!(
            check(&policy, "http://192.168.0.1/"),
            Err(Refused::Address(ip("192.168.0.1")))
        );
        assert!(policy.permits(ip("::ffff:10.0.0.1")));
    }

    #[test]
    fn a_redirect_is_checked_and_never_downgrades_or_runs_on() {
        let policy = policy(None, &[], false);
        let url = |text: &str| Url::parse(text).unwrap();
        let first = [url("https://example.com/")];
        assert_eq!(
            policy.redirect_refusal(&url("https://example.org/"), &first),
            None
        );
        assert_eq!(
            policy.redirect_refusal(&url("http://example.org/"), &first),
            Some(Refused::Downgrade)
        );
        assert_eq!(
            policy.redirect_refusal(&url("http://127.0.0.1/"), &[url("http://example.com/")]),
            Some(Refused::Address(ip("127.0.0.1")))
        );
        let chain = vec![url("http://example.com/"); DEFAULT_MAX_REDIRECTS + 1];
        assert_eq!(
            policy.redirect_refusal(&url("http://example.com/"), &chain),
            Some(Refused::TooManyRedirects)
        );
        assert_eq!(
            policy.redirect_refusal(&url("http://example.com/"), &chain[1..]),
            None
        );
    }

    #[test]
    fn a_wildcard_needs_a_name_before_its_suffix() {
        let listed = policy(Some(&["*.hooks.example.com"]), &[], false);
        assert!(listed.host_listed("a.hooks.example.com"));
        assert!(!listed.host_listed(".hooks.example.com"));
        assert!(!listed.host_listed("hooks.example.com"));
    }

    #[test]
    fn a_range_without_a_prefix_is_one_address_of_either_kind() {
        let one: Net = "10.0.0.1".parse().unwrap();
        assert!(one.contains("10.0.0.1".parse().unwrap()));
        assert!(!one.contains("10.0.0.2".parse().unwrap()));
        let one: Net = "2001:db8::1".parse().unwrap();
        assert!(one.contains("2001:db8::1".parse().unwrap()));
        assert!(!one.contains("2001:db8::2".parse().unwrap()));
    }

    #[test]
    fn an_ipv6_range_holds_its_prefix_and_nothing_past_it() {
        let net: Net = "2001:db8::/32".parse().unwrap();
        assert!(net.contains("2001:db8:ffff::1".parse().unwrap()));
        assert!(!net.contains("2001:db9::".parse().unwrap()));
        let net: Net = "fd00::/8".parse().unwrap();
        assert!(net.contains("fdff::1".parse().unwrap()));
        assert!(!net.contains("fe00::1".parse().unwrap()));
    }

    #[test]
    fn every_error_says_what_was_wrong() {
        let address: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(
            Refused::Address(address).to_string(),
            "10.0.0.1 is not a public address"
        );
        let texts: std::collections::HashSet<String> = [
            Refused::Url,
            Refused::Scheme,
            Refused::Host,
            Refused::NoAddress,
            Refused::Downgrade,
            Refused::TooManyRedirects,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(texts.len(), 6);
        assert!(texts.iter().all(|text| !text.is_empty()));
        assert!(NetError.to_string().contains("prefix"));
        assert_eq!(
            BodyError::TooLarge.to_string(),
            "the response body is longer than its limit"
        );
        assert!(std::error::Error::source(&BodyError::TooLarge).is_none());
        let _ = rustls::crypto::ring::default_provider().install_default();
        let unreadable = reqwest::Client::new().get("not a url").build().unwrap_err();
        let error = BodyError::Read(unreadable);
        assert!(
            error
                .to_string()
                .starts_with("the response body could not be read: ")
        );
        assert!(std::error::Error::source(&error).is_some());
    }
}

/// `is_public` over generated addresses, against the IANA special-purpose ranges written
/// out as ranges, in IPv4, IPv4-mapped IPv6, NAT64, and 6to4 alike.
#[cfg(test)]
mod properties {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use proptest::prelude::*;

    use super::{Net, is_public};

    const NOT_PUBLIC_V4: [&str; 14] = [
        "0.0.0.0/8",
        "10.0.0.0/8",
        "100.64.0.0/10",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "172.16.0.0/12",
        "192.0.0.0/24",
        "192.0.2.0/24",
        "192.88.99.0/24",
        "192.168.0.0/16",
        "198.18.0.0/15",
        "198.51.100.0/24",
        "203.0.113.0/24",
        "224.0.0.0/3",
    ];

    fn expected(address: Ipv4Addr) -> bool {
        !NOT_PUBLIC_V4
            .iter()
            .any(|net| net.parse::<Net>().unwrap().contains(IpAddr::V4(address)))
    }

    /// Addresses near the edges of the ranges, where a wrong comparison shows, and any.
    fn v4() -> impl Strategy<Value = Ipv4Addr> {
        let near_a_range = (
            prop::sample::select(vec![
                0_u8, 8, 9, 10, 11, 100, 126, 127, 128, 169, 172, 192, 198, 203, 223, 224,
            ]),
            any::<u8>(),
            prop::sample::select(vec![0_u8, 1, 2, 3, 99, 100, 112, 113, 114, 255]),
            any::<u8>(),
        )
            .prop_map(|(a, b, c, d)| Ipv4Addr::new(a, b, c, d));
        prop_oneof![near_a_range, any::<u32>().prop_map(Ipv4Addr::from)]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2_000))]

        #[test]
        fn an_ipv4_address_is_public_unless_a_range_holds_it(address in v4()) {
            prop_assert_eq!(is_public(IpAddr::V4(address)), expected(address));
        }

        #[test]
        fn ipv4_inside_ipv6_is_judged_as_the_ipv4_address(address in v4()) {
            let want = expected(address);
            let [high, low] = [
                u16::try_from(u32::from(address) >> 16).unwrap(),
                u16::try_from(u32::from(address) & 0xffff).unwrap(),
            ];
            let mapped = IpAddr::V6(address.to_ipv6_mapped());
            let nat64 = IpAddr::V6(Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, high, low));
            let six_to_four = IpAddr::V6(Ipv6Addr::new(0x2002, high, low, 0, 0, 0, 0, 1));
            prop_assert_eq!(is_public(mapped), want);
            prop_assert_eq!(is_public(nat64), want);
            prop_assert_eq!(is_public(six_to_four), want);
        }

        #[test]
        fn only_global_unicast_ipv6_outside_its_special_ranges_is_public(segments in any::<[u16; 8]>()) {
            let address = Ipv6Addr::from(segments);
            let [first, second, ..] = segments;
            let special = first & 0xe000 != 0x2000
                || (first == 0x2001 && (second < 0x0200 || second == 0x0db8))
                || first == 0x2002
                || (first == 0x3fff && second < 0x1000);
            prop_assume!(first != 0x2002 && address.to_ipv4_mapped().is_none());
            prop_assert_eq!(is_public(IpAddr::V6(address)), !special);
        }
    }
}
