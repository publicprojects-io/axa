# axa

**axa** is a small shared library for services built directly on [Axum](https://docs.rs/axum). It provides the plumbing every service needs: route tables, middleware, health checks, configuration, logging, graceful shutdown, background jobs, and more.

> **A library, not a framework.** Your `main.rs` stays ordinary Rust that calls ordinary functions. Nothing calls into your code from behind a trait.

---

## Why axa Exists

For services handling identity, clinical, or government records, the framework trade-off runs the wrong way: an auditor must read the framework **and** your code, and each dependency joins your supply chain.

axa flips this model:

| Framework Approach | axa Approach |
|---|---|
| `main.rs` hides behind traits | `main.rs` is ordinary Rust |
| Shared code lives in framework | Shared code lives in one library |
| 510+ dependencies (example) | ~348 dependencies (same service with sqlx) |
| Permissive defaults | **Fail-closed defaults** |

### Fail-Closed Examples

A production service **refuses to start** if:
- No `DATABASE_URL` is provided (when database is configured)
- `CORS_ALLOWED_ORIGINS=*` (wildcard CORS)
- `DATABASE_URL` uses cleartext on a remote host
- Any configuration value is invalid

---

## Quick Start

See the [Quick Start Guide](docs/QUICKSTART.md) for a complete 5-minute tutorial.

### Minimal Service

```rust
use axa::{Environment, config::{Config, Defaults}, logger, prelude::*, server};

async fn hello() -> Result<Response> {
    format::json(serde_json::json!({"hello": "world"}))
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    logger::init("hello_service", &config.logger);

    let ctx = AppContext::new(environment, (), config);
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
        .into_app(ctx)?;

    server::serve(router, &config.server).await
}
```

Run it:
```bash
cargo run
```

Your service listens on port **1241** (a=1, x=24, a=1) and serves:
- `/api/hello` - Your custom route
- `/_ping` - Process is up
- `/_health` - Process is up
- `/_readiness` - Database and dependencies ready

---

## Installation

```toml
[dependencies]
axa = "0.10"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

---

## Features

axa is modular. Enable only what you need via Cargo features:

| Feature | Crate | Purpose |
|---|---|---|
| `sqlx` | `axa-sqlx` | PostgreSQL with sqlx (recommended) |
| `sea-orm` | `axa-seaorm` | PostgreSQL with SeaORM |
| `diesel` | `axa-diesel` | PostgreSQL with Diesel (no `libpq`) |
| `queue` | `axa-postgres-job-queue` | Job queue and scheduling |
| `outbox` | `axa-transactional-outbox` | Transactional outbox |
| `outbox-encryption` | | Outbox with sealed payloads |
| `paseto` | `axa-paseto-auth` | PASETO bearer tokens |
| `keycloak` | `axa-keycloak-auth` | Keycloak OAuth2/OIDC |
| `session` | `axa-web-session` | Browser sessions |
| `redis` | `axa-redis-cache` | Redis cache |
| `cursor-pagination` | `axa-cursor-pagination` | Signed cursor pagination |
| `ratelimit` | `axa-request-rate-limit` | Rate limiting |
| `idempotency` | `axa-idempotency-key` | Idempotency keys |
| `versioning` | `axa-accepts-version-negotiation` | API versioning |
| `openapi` | `axa-openapi-docs` | OpenAPI docs and Swagger UI |
| `tracing` | `axa-tracing` | Advanced tracing and diagnostics |
| `opentelemetry` | `axa-opentelemetry` | OTLP traces and metrics |
| `prometheus` | `axa-prometheus-metrics` | Prometheus metrics |
| `tera` | `axa-tera-templates` | Tera HTML templates |
| `problem` | `axa-problem-details-for-http-apis` | RFC 9457 problem details (always included) |

**Note**: Each feature adds dependencies. Enable only what you use.

---

## Documentation

| Resource | Purpose |
|---|---|
| **[Quick Start](docs/QUICKSTART.md)** | Hands-on 5-minute tutorial |
| **[Feature Selection](docs/FEATURES.md)** | Choose the right features |
| **[Configuration](docs/CONFIGURATION.md)** | All configuration options |
| **[Examples](docs/EXAMPLES.md)** | Real-world patterns |
| **[Architecture](docs/ARCHITECTURE.md)** | How axa works |
| **[Specification](spec/index.md)** | Single source of truth |
| **[Changelog](CHANGELOG.md)** | Release history |

---

## Project Links

- **Source**: [github.com/publicprojects-io/axa](https://github.com/publicprojects-io/axa)
- **Docs**: [docs.rs/axa](https://docs.rs/axa)
- **Package**: [crates.io/crates/axa](https://crates.io/crates/axa)

---

## Requirements

- Rust **1.97** or newer
- Edition **2024**
- PostgreSQL (for `queue`, `outbox`, `sqlx`, `sea-orm`, `idempotency` features)

---

## What axa Provides

### Core (Always Available)

- **Route tables**: `Routes` and `AppRoutes` build ordinary `axum::Router`
- **Middleware stack**: Request ID, tracing, security headers, CORS, timeout, compression, body limit, panic catch
- **Error handling**: RFC 9457 problem details with generic error text to clients
- **Configuration**: Environment-based with code defaults
- **Health checks**: `/_ping`, `/_health`, `/_readiness`
- **Logging**: Structured logging with request tracking
- **Graceful shutdown**: SIGTERM/SIGINT handling with in-flight request completion
- **CLI**: Standard `start`, `db`, `task`, `check` commands
- **Tasks**: Named jobs for operators to run
- **Background jobs**: Worker abstraction with multiple modes

### With Features

Each optional feature adds specific capabilities. See [Feature Selection](docs/FEATURES.md) for details.

---

## Principles

These principles guide axa's design:

| Principle | Meaning |
|---|---|
| **Library, not framework** | Your code controls the flow |
| **Two consumers first** | Code added only when two services need it, or one has a safety case |
| **Fail closed** | Production refuses unsafe defaults |
| **Generic errors** | Internal failure text goes to logs, not clients |
| **Written afresh** | Not copied from Loco (Apache-2.0 only) |
| **New dependencies raised first** | Cost is paid by all consumers |
| **A record is not state** | Audit rows are append-only |
| **Final means final** | Outcomes are never undone by helpers |
| **An outcome commits with its audit** | API and audit commit together |
| **The safe way is the only way** | Safer behavior replaces weaker, weaker is removed |

---

## Never Added

These will **never** be added to axa, as they violate the auditability principle:

- ❌ Lifecycle hooks
- ❌ Route macros
- ❌ Code generation
- ❌ Dependency-injection container
- ❌ Plugin system
- ❌ Configuration language
- ❌ Configuration files
- ❌ Database abstraction
- ❌ Generic CRUD controller

---

## Migration

- **[Upgrading to axa 0.3](help/upgrading-to-0.3/index.md)** - Breaking changes and upgrade path
- **[From Loco into axa](help/from-loco-into-axa/index.md)** - Complete migration guide
- **[From Loco SeaORM into axa-seaorm](help/from-loco-seaorm-into-axa-seaorm/index.md)** - SeaORM-specific migration

---

## Contributing

See [AGENTS.md](AGENTS.md) for:
- Build, test, lint commands
- Conventions and rules
- Release process

---

## Relationship to Loco

To upgrade from an earlier version, read [Upgrading to axa 0.3](help/upgrading-to-0.3/index.md).

To move a project across, follow [Converting a project from Loco to axa](help/from-loco-into-axa/index.md), and for the database, [Moving SeaORM code from Loco to axa](help/from-loco-seaorm-into-axa-seaorm/index.md).

axa was extracted from what the Main X Index services relied on in [Loco](https://loco.rs) (Apache-2.0), and some differences are deliberate:

| Loco | axa |
|---|---|
| A `Hooks` trait drives your app | Your `main.rs` calls plain functions |
| YAML config rendered through a template engine | The environment, with defaults in code |
| Permissive CORS when enabled | Listed origins only in production |
| `x-powered-by` header | None |
| No request timeout or security headers by default | Both on in production; headers everywhere |
| `If-None-Match` compared as one exact string | Weak comparison, lists, and `*`, as RFC 9110 has it |

The code was written afresh rather than copied, so this crate can keep its own licence terms.

---

## License

MIT OR Apache-2.0 OR BSD-3-Clause OR GPL-2.0-only OR GPL-3.0-only

See [LICENSE.md](LICENSE.md) for details.
