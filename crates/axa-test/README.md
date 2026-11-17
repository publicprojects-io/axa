# axa-test

Request-level test helpers for [`axa`](https://crates.io/crates/axa) services, built
on [`axum-test`](https://docs.rs/axum-test).

A service implements `axa::boot::TestApp` to say how it boots for tests, then sends
real requests to its full router, middleware and all, with no port opened.

```toml
[dev-dependencies]
axa-test = "0.9"
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

## Logs

`axa_test::logs::capture()` keeps what the test's thread logs, as JSON lines, until it is
dropped, a request through `TestServer` included:

```rust,ignore
let logs = axa_test::logs::capture();
server.post("/sign-in").json(&form).await;
assert!(logs.contains("sign-in refused"));
logs.assert_absent(&form.password);
```

## Mail

With the `mail` feature, give the service `memory:` as its mail URL in tests, so it
builds its `Mailer` as in production and keeps it in `ctx.shared_store`. Then
`axa_test::mail::outbox(&ctx.shared_store)` reads what it sent, and fails the test if
the mailer would send real mail. `mail::sent_to(&outbox, address)` picks one
recipient's messages.

## OpenAPI conformance

With the `openapi` feature, `axa_test::openapi::Conformance` checks a real response
against an OpenAPI 3.1 document, such as one `utoipa` wrote: that its status is one the
operation documents, its content type one that status documents, and its JSON body
valid against the schema there, `$ref`s into `#/components` included. It fetches
nothing, and refuses a 3.0 document rather than read its schemas wrongly.

```rust,ignore
let conformance = Conformance::new(serde_json::to_value(ApiDoc::openapi()).unwrap());
conformance.assert(&server.get("/api/places/1").await);
conformance.assert(&server.get("/api/places/missing").await);
```

## Tenants

`axa_test::isolation::assert_hidden` checks that one tenant cannot reach another's
resources: every probe of the resource, sent as the other tenant, must answer `404`, and
a list must not show it.

With the `paseto` feature, `axa_test::paseto::TestKeys` signs PASETO tokens under a
published throwaway key, with whatever attributes a test names, and gives the verifier
that accepts them, for a service whose guard is `axa-paseto-auth`:

```toml
[dev-dependencies]
axa-test = { version = "0.9", features = ["paseto"] }
```

Never use these keys outside tests: anyone can mint a token they verify.


## A database for each test

With the `postgres-sqlx` feature, `axa_test::postgres::TestDatabase` gives each test a
Postgres database of its own, migrated, on the server `AXA_TEST_DB_URL` names, as a
role that may create databases:

```toml
[dev-dependencies]
axa-test = { version = "0.9", features = ["postgres-sqlx"] }
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
