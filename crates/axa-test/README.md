# axa-test

Request-level test helpers for [`axa`](https://crates.io/crates/axa) services, built
on [`axum-test`](https://docs.rs/axum-test).

A service implements `axa::boot::TestApp` to say how it boots for tests, then sends
real requests to its full router, middleware and all, with no port opened.

```toml
[dev-dependencies]
axa-test = "0.4"
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
axa-test = { version = "0.4", features = ["paseto"] }
```

Never use these keys outside tests: anyone can mint a token they verify.


## A database for each test

With the `postgres-sqlx` feature, `axa_test::postgres::TestDatabase` gives each test a
Postgres database of its own, migrated, on the server `AXA_TEST_DB_URL` names, as a
role that may create databases:

```toml
[dev-dependencies]
axa-test = { version = "0.4", features = ["postgres-sqlx"] }
```

```rust,ignore
use axa_test::postgres::TestDatabase;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_note_is_saved() {
    let db = TestDatabase::new(&MIGRATOR).await.unwrap();
    // ... use db.pool, or db.options for the service's own pool ...
    db.close().await;
}
```

The migrations run once, into a template named for them, `axa_test_template_` and a
hash, and each test's database is cloned from it with `CREATE DATABASE ... TEMPLATE`.
Tests no longer wait for each other or see each other's rows, so they can run in
parallel.

`close().await` drops the database and waits. A guard dropped without it, as when a
test panics, has the database dropped on a thread of its own, waiting at most two
seconds for it. Any it leaves when the test binary exits are dropped by the next run,
once no session has used them for two minutes. A changed migration gets a new
template, and `drop_templates` drops the old ones.
