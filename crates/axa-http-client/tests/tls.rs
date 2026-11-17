//! TLS on ring, in a process of its own, so no other test installs a provider first.

use rustls::crypto::{CryptoProvider, ring};

#[test]
fn a_client_builds_on_ring_with_no_provider_installed_and_again_after() {
    assert!(CryptoProvider::get_default().is_none());
    axa_http_client::Client::builder()
        .build()
        .expect("first client");
    let installed = CryptoProvider::get_default().expect("a provider is installed");
    let suites = |provider: &CryptoProvider| {
        provider
            .cipher_suites
            .iter()
            .map(rustls::SupportedCipherSuite::suite)
            .collect::<Vec<_>>()
    };
    assert_eq!(suites(installed), suites(&ring::default_provider()));
    axa_http_client::Client::builder()
        .build()
        .expect("second client");
}
