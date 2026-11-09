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
