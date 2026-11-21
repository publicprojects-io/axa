//! What the helpers do when a service cannot boot.

use axa::Error;
use axa_test::prelude::*;

/// A service whose boot for tests fails.
struct Broken;

impl TestApp for Broken {
    fn boot_test() -> impl Future<Output = axa::Result<BootResult>> {
        std::future::ready(Err(Error::Message("no database for tests".to_string())))
    }
}

#[tokio::test]
async fn a_failed_boot_is_handed_back() {
    let booted = boot_test::<Broken>().await;
    assert!(matches!(booted, Err(Error::Message(text)) if text == "no database for tests"));
}

#[tokio::test]
#[should_panic(expected = "the service boots for tests")]
async fn a_request_test_on_a_service_that_cannot_boot_fails() {
    request::<Broken, _, _>(|_server, _ctx| async {
        panic!("the test ran on a service that never booted");
    })
    .await;
}

/// A service that reads its settings through the lookup it is given. Its boot stops
/// there, saying the host it read, so the test needs no database of any kind.
struct FromSettings;

impl TestApp for FromSettings {
    fn boot_test() -> impl Future<Output = axa::Result<BootResult>> {
        Self::boot_test_with(lookup(&[]))
    }

    fn boot_test_with(settings: Lookup) -> impl Future<Output = axa::Result<BootResult>> {
        let defaults = axa::config::Defaults {
            database: "",
            ..axa::config::Defaults::default()
        };
        let read = axa::config::Config::from_lookup(&axa::Environment::Test, &defaults, |name| {
            settings(name)
        });
        std::future::ready(read.and_then(|config| Err(Error::Message(config.server.host))))
    }
}

#[tokio::test]
async fn a_test_boots_with_the_settings_it_names_and_no_others() {
    let host = |booted: axa::Result<BootResult>| match booted {
        Err(Error::Message(host)) => host,
        other => panic!("{other:?}"),
    };
    let given = boot_test_with::<FromSettings>(&[("HOST", "https://crm.example.org")]).await;
    assert_eq!(host(given), "https://crm.example.org");
    assert_eq!(
        host(boot_test::<FromSettings>().await),
        "http://localhost",
        "the default"
    );
}

#[tokio::test]
#[should_panic(expected = "implement TestApp::boot_test_with")]
async fn a_service_that_cannot_boot_from_settings_says_so_rather_than_ignore_them() {
    request_with::<Broken, _, _>(
        &[("HOST", "https://crm.example.org")],
        |_server, _ctx| async {
            panic!("the test ran with settings the service never read");
        },
    )
    .await;
}

/// Set once per process, as a service's settings often are.
static ONCE: std::sync::OnceLock<&str> = std::sync::OnceLock::new();

#[test]
fn a_test_in_its_own_process_sets_state_read_once() {
    axa_test::own_process!();
    ONCE.set("first").unwrap();
    assert_eq!(ONCE.get(), Some(&"first"));
}

#[test]
fn another_in_its_own_process_sets_it_again() {
    axa_test::own_process!();
    ONCE.set("second")
        .expect("a fresh process, where nothing set it");
    assert_eq!(ONCE.get(), Some(&"second"));
}

#[tokio::test]
async fn an_async_test_runs_in_its_own_process_too() {
    axa_test::own_process!();
    ONCE.set("third")
        .expect("a fresh process, where nothing set it");
    tokio::task::yield_now().await;
    assert_eq!(ONCE.get(), Some(&"third"));
}

#[test]
#[should_panic(expected = "did not pass alone in its own process")]
fn a_name_that_matches_no_test_fails() {
    axa_test::own_process::run_alone("boot::no_such_test");
}

#[tokio::test]
async fn booting_a_test_with_settings_installs_the_tls_provider_first() {
    axa_test::own_process!();
    assert!(!axa::tls::is_installed(), "a fresh process has none");
    assert!(boot_test_with::<FromSettings>(&[]).await.is_err());
    assert!(axa::tls::is_installed());
}

#[tokio::test]
async fn the_provider_is_installed_even_for_a_boot_that_fails_before_any_context() {
    axa_test::own_process!();
    assert!(!axa::tls::is_installed(), "a fresh process has none");
    assert!(boot_test::<Broken>().await.is_err());
    assert!(
        axa::tls::is_installed(),
        "axa-test installed it, not the service"
    );
}
