# Where the idempotency layer goes

`axa-idempotency-key` answers a retried write with its first answer. It is one layer, and
it only behaves if it sits in the right place. This guide says where that is and why.

## The order

Layers added later are outside layers added earlier. So add this one first of the
layers that matter here, then whatever refuses a caller, and call `apply` last:

```rust
use axa::idempotency::{Idempotency, idempotent};
use axa::paseto::{self, Allowed};
use axum::middleware::from_fn_with_state;
# use std::sync::Arc;
# async fn example(
#     pool: sqlx::PgPool,
#     ctx: axa::AppContext,
#     routes: axa::routes::AppRoutes,
#     guard: Arc<paseto::Guard>,
# ) -> axa::Result<axum::Router> {

let keys = Idempotency::new(pool, |request| {
    request.extensions().get::<Allowed>().map(|allowed| allowed.claims.sub.clone())
});
keys.check().await?; // A missing table stops the boot, not the first write.

let router = routes
    .into_router(ctx.clone())                              // the routes
    .layer(from_fn_with_state(keys, idempotent))           // inside the guards
    .layer(from_fn_with_state(guard, paseto::middleware)); // the guards
let app = axa::middleware::apply(router, &ctx.config.http)?; // last, and outermost
# Ok(app)
# }

```

From the outside in, a request then meets:

```text
request id, tracing, security headers, CORS, timeout, compression, refusal shaping,
body limit, panic catching     <- axa::middleware::apply
authentication and policy      <- your guards
idempotent                     <- this layer
your handler
```

Do not use `AppRoutes::into_app` here. It calls `apply` for you, so anything you add
afterwards lands outside the stack.

## Why it goes there

**Inside compression.** The layer stores the body it reads. Outside compression that
body is gzip, and a replay would send gzip to a caller that never asked for it, with no
`content-encoding` to say so. One service shipped that defect. Inside, the layer stores
plain bytes, and the stack compresses each replay for the caller that asked.

If it ends up outside anyway, nothing wrong is stored. An answer with a
`content-encoding` is never kept: the caller gets it, a retry gets a `409`, and the log
says `idempotency_answer_not_kept` with the reason `encoded`. That line means this layer
is in the wrong place, or that a handler compresses for itself.

**Inside the guards.** The layer answers without calling the handler. Outside the
guards, that answer would reach a caller they would have refused. Inside, a replay
reaches only a request that passed them.

It also means that the layer can read what the guards learned. The closure given to
`Idempotency::new` runs after them, so it reads who is calling from an extension.

**Inside the timeout and the panic catcher.** Each of these ends a run without an
answer for this layer to see. So does a crash, or a caller hanging up. The run may have
done its work, so the key stays claimed for its whole `ttl`, 24 hours by default, and a
retry of it gets a `409` `idempotency_in_progress` until then. That is the safe answer:
the write never runs twice under one key. Its `Retry-After` is as long as the first
request has run so far, from 1 second up to the end of its lease, so a client that obeys
asks about a lost run some 17 times in a day, not once a second. A client that has checked the write did not
happen can send it again under a new key.

A run reports in while it works, every third of its lease, so a long handler is never
taken for lost. The lease is the `ttl` unless you choose a shorter one, and choosing one
lets the write run a second time:

```rust
# fn example(keys: axa::idempotency::Idempotency) {
// Only for a handler that is safe to run twice, such as one with a unique constraint.
let keys = keys.rerun_abandoned_after(std::time::Duration::from_mins(1));
# }
```

Once a run has not reported in for that long, a retry of the same request takes the key
over and runs again. A different request under that key is still refused. A lease under
60 ms, or longer than the `ttl`, stops the boot with a panic.

## Naming the caller

A key belongs to a caller, and the closure says who that is. When it returns `None`, a
request with a key is refused with a `401` and does not run. Three things follow.

With `Allowed` from `axa-paseto-auth` or `axa-keycloak-auth`, there is none on a public path or
with enforcement off. If those routes take keys, name the caller some other way.

If finding the caller needs an `await`, write a small layer of your own that does it
and leaves the answer in an extension, and read that in the closure:

```rust
# use axum::{extract::Request, middleware::Next, response::Response};
# struct Claims { sub: String }
# async fn request_claims(_: &axum::http::HeaderMap) -> Result<Claims, ()> { Err(()) }
#[derive(Clone)]
struct Caller(String);

async fn name_the_caller(mut request: Request, next: Next) -> Response {
    if let Ok(claims) = request_claims(request.headers()).await {
        request.extensions_mut().insert(Caller(claims.sub));
    }
    next.run(request).await
}

```

With tenants, return a text that names both, such as `format!("{tenant}:{subject}")`.
Do not return one fixed text for everyone who is signed out unless they may read each
other's answers, because they then share every key.

## Leaving a route out

By default the layer covers everything but `GET`, `HEAD`, `OPTIONS`, and `TRACE`. Narrow
it for routes that keep idempotency of their own, or that answer with something that
should not sit in a table, such as a token:

```rust
# fn example(keys: axa::idempotency::Idempotency) {
let keys = keys.only(|method, path| {
    method == axum::http::Method::POST
        && path.starts_with("/api/")
        && path != "/api/plans/import"
});
# }
```

A route that accepts more than 1 MiB needs this too, or `max_request_bytes`, because the
layer holds the whole request to compare it. A larger keyed request is a `413`.

## The table

Put the SQL in a migration. `axa` does not own your schema:

```rust
# async fn example(pool: sqlx::PgPool) -> Result<(), sqlx::Error> {
for statement in axa::idempotency::SETUP_STATEMENTS {
    sqlx::query(statement).execute(&pool).await?;
}
# Ok(())
# }
```

A service whose role may change the schema can call `axa::idempotency::setup(&pool)` at
boot instead. `Idempotency::new` takes a `PgPool`. On SeaORM that is
`ctx.db.get_postgres_connection_pool().clone()`.

## Requiring a key

A key is optional by default. To refuse a write that has none, name the routes:

```rust
# fn example(keys: axa::idempotency::Idempotency) {
let keys = keys.required(|_, path| path == "/payments");
# }
```

A request with no key then gets a `400` with the `code` `idempotency_key_required`.
`required` only applies to what the layer covers, so it cannot ask a read for a key.

## What a service publishes

A client cannot find out how keys work here by trying them. The
[Idempotency-Key draft](https://datatracker.ietf.org/doc/draft-ietf-httpapi-idempotency-key-header/)
says each service must publish this, so put it in your API documentation:

- Which routes take a key, and which require one.
- How long a key lives, 24 hours by default, and that it belongs to one caller.
- That a write whose run was lost holds its key for that long, so a retry gets a `409`
  and should use a new key once it has checked the write did not happen.
- That a key's answer is replayed only for a `2xx` or `3xx`, and that the same key with
  a different request is a `422`.

## Sweeping

This is required. Nothing runs by itself: an expired row is never replayed, but it stays
on disk, with the copy of the answer it holds, until you delete it. A claim whose run
was lost is deleted too, once its lease ends. Without a schedule
the answers are kept for ever. Call `sweep`:

```rust
# fn example(keys: axa::idempotency::Idempotency) {
tokio::spawn(async move {
    let mut every_hour = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        every_hour.tick().await;
        if let Err(error) = keys.sweep().await {
            tracing::warn!(%error, "idempotency sweep failed");
        }
    }
});
# }
```

## Personal data

A kept answer is a copy of what the handler sent, and is not encrypted, so an answer
holding a person's record is a second copy of them, in the table and in every backup. A
service must:

1. Schedule `sweep`, as above.
2. Add `axa_idempotency` to its record of personal data and its retention list.
3. Say in its privacy notice that erasure can take up to the key's lifetime.

The third is needed because `keys.forget_caller(subject)` erases by who made the
request, not by whose data an answer holds. When staff create a patient record, the
answer belongs to that staff member, so erasing the patient does not find it. It goes
when the key expires and the sweep runs.

A route that cannot accept this is left out with `only`, or kept shorter with `ttl`.

## After the answer

A `202` says the work was accepted, and a replay says the same for as long as the key
lives, even once the job has failed. When a job fails and its caller should be able to
retry under the same key, store the key with the job and call
`keys.forget(caller, key)`.
