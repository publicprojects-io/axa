//! The TLS crypto provider every connection a service makes shares.
//!
//! rustls needs one process-wide crypto provider. A crate built with no provider of its
//! own, such as reqwest's `rustls-no-provider`, panics with "No rustls crypto provider
//! is configured" on its first connection unless one is installed. Axa installs ring,
//! the provider every Axa crate uses, before it opens a connection: in
//! [`crate::db::connect`] and [`crate::AppContext::new`]. `axa-test` installs it before a
//! test boots. A service's own code that runs earlier, such as a test that builds a
//! client without booting, calls [`install_provider`] itself.

/// Install ring as the process's rustls crypto provider, unless one is already
/// installed. Safe to call any number of times, from any thread.
pub fn install_provider() {
    // An error means another provider is installed already, which is what we want.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Whether a rustls crypto provider is installed for the process, ring or another.
#[must_use]
pub fn is_installed() -> bool {
    rustls::crypto::CryptoProvider::get_default().is_some()
}

#[cfg(test)]
mod tests {
    use super::install_provider;

    #[test]
    fn the_provider_is_ring_and_installing_it_twice_is_harmless() {
        install_provider();
        install_provider();
        assert!(super::is_installed());
        let installed = rustls::crypto::CryptoProvider::get_default().expect("installed");
        let ring = rustls::crypto::ring::default_provider();
        assert_eq!(
            installed.cipher_suites.len(),
            ring.cipher_suites.len(),
            "ring's cipher suites"
        );
    }
}
