# axa-test

Request-level test helpers for [`axa`](https://crates.io/crates/axa) services, built
on [`axum-test`](https://docs.rs/axum-test).

A service implements `axa::boot::TestApp` to say how it boots for tests, then sends
real requests to its full router, middleware and all, with no port opened.

```toml
[dev-dependencies]
axa-test = "0.3"
```

```rust,ignore
use axa_test::prelude::*;

#[tokio::test]
async fn health_answers() {
    request::<App, _, _>(|server, _ctx| async move {
        assert_eq!(server.get("/_health/live").await.status_code(), 200);
    })
    .await;
}
```

It depends on `axa`, so it is a dev-dependency: it never ships in a service's binary.
A crate that lists its own `axum-test` should use the same major version, 21, or it
gets two copies, and errors that say `TestServer` does not match `TestServer`.

## Tenants

`axa_test::isolation::assert_hidden` checks that one tenant cannot reach another's
resources: every probe of the resource, sent as the other tenant, must answer `404`, and
a list must not show it.

With the `paseto` feature, `axa_test::paseto::TestKeys` signs PASETO tokens under a
published throwaway key, with whatever attributes a test names, and gives the verifier
that accepts them, for a service whose guard is `axa-paseto-auth`:

```toml
[dev-dependencies]
axa-test = { version = "0.3", features = ["paseto"] }
```

Never use these keys outside tests: anyone can mint a token they verify.

