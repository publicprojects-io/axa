//! Mail a service sent, read from the memory mailer it built.
//!
//! In tests, give the service `memory:` as its mail URL, so it builds its
//! [`Mailer`] with [`Mailer::from_url`] as in production and keeps it in
//! `ctx.shared_store`. Then [`outbox`] finds what it sent:
//!
//! ```ignore
//! request::<App, _, _>(|server, ctx| async move {
//!     server.post("/sign-in/link").json(&json!({"email": "ada@example.org"})).await;
//!     let sent = axa_test::mail::outbox(&ctx.shared_store).take();
//!     assert_eq!(sent[0].to, ["ada@example.org"]);
//! })
//! .await;
//! ```

pub use axa::mail::{Mailer, Outbox, Sent};
use axa::store::Store;

/// The outbox of the memory [`Mailer`] in a service's store, `ctx.shared_store`.
///
/// # Panics
///
/// If there is no `Mailer` in the store, or it is not a memory mailer, so a test
/// that would send real mail fails instead.
#[must_use]
pub fn outbox(store: &Store) -> Outbox {
    let mailer = store
        .get_ref::<Mailer>()
        .expect("the service keeps its Mailer in ctx.shared_store");
    mailer
        .outbox()
        .expect("the service's Mailer is a memory mailer: give it memory: in tests")
}

/// The messages sent so far to `address`, leaving them in the outbox.
#[must_use]
pub fn sent_to(outbox: &Outbox, address: &str) -> Vec<Sent> {
    outbox
        .sent()
        .into_iter()
        .filter(|sent| sent.to.iter().any(|to| to.eq_ignore_ascii_case(address)))
        .collect()
}
