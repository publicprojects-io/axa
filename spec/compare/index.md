# Comparisons

Each page studies one open-source Rust web project from its source, at a named commit, and
says what `axa` could add, what it should not copy, and where it already does better. They
were read on 9 and 10 November 2026. [`plan.md`](../../plan.md) draws them together, and
[`tasks.md`](../../tasks.md) lists the work.

## Services built on Axum

| Project | What it is | Page |
|---|---|---|
| crates.io | The Rust package registry, on Diesel and Postgres | [crates-io](crates-io/index.md) |
| Kanidm | An identity management server | [kanidm](kanidm/index.md) |
| Svix | A webhook delivery service | [svix](svix/index.md) |
| Windmill | A workflow engine, on sqlx and a Postgres queue | [windmill](windmill/index.md) |
| Sentry Relay | Sentry's high-throughput ingestion proxy | [sentry-relay](sentry-relay/index.md) |
| Atuin | A small shell-history sync server, on Postgres or SQLite | [atuin](atuin/index.md) |
| tuwunel | A Matrix homeserver, successor to Conduit and conduwuit | [tuwunel](tuwunel/index.md) |
| Loco | A Rails-like framework on Axum, which `axa` replaces | [loco](loco/index.md) |

## Other Rust web frameworks

| Project | What it is | Page |
|---|---|---|
| actix-web | A framework on its own server, with actix-extras | [actix-web](actix-web/index.md) |
| Poem | A framework with typed OpenAPI, gRPC, and MCP | [poem](poem/index.md) |
| Rocket | A framework with request guards, forms, and fairings | [rocket](rocket/index.md) |
| Topcoat | A young full-stack framework for server-rendered apps | [topcoat](topcoat/index.md) |

A claim on a page comes from the code or documents it cites. Where a page could not confirm
something, it says so. Five of the gaps they named were checked against `axa`'s own code,
and four of them, no `WWW-Authenticate` on a `401`, readiness that stays `200` during
shutdown, a body limit that a handler reading `Body` gets past, and no request metrics,
were closed in axa 0.11, with the rest of stage 1 of the plan. Each page marks what landed
as "Done in 0.11". Request decompression is still open, as T-20.
