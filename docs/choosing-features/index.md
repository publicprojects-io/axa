# Choosing Features Guide

This guide helps you select the right axa features for your use case.
Use it as a decision tree when starting a new project or adding functionality.

---

## Quick Reference Table

| Need | Feature | Crate | When to Use |
|------|---------|-------|-------------|
| Database (Postgres) | `postgres-sqlx` | `axa-postgres-sqlx` | Raw SQL, full control |
| Database (Postgres) | `postgres-seaorm` | `axa-postgres-seaorm` | ORM, migrations, Loco projects |
| Database (Postgres) | `postgres-diesel` | `axa-postgres-diesel` | ORM, compile-time safety, no C libs |
| Database (Postgres) | `postgres-toasty` | `axa-postgres-toasty` | Async ORM, migrations forward-only |
| Database (MySQL) | `mysql-sqlx` | `axa-mysql-sqlx` | Raw SQL, full control |
| Database (MySQL) | `mysql-seaorm` | `axa-mysql-seaorm` | ORM, migrations |
| Database (MySQL) | `mysql-diesel` | `axa-mysql-diesel` | ORM, compile-time safety, no C libs |
| Database (MySQL) | `mysql-toasty` | `axa-mysql-toasty` | Async ORM, migrations forward-only |
| Database (SQLite) | `sqlite-sqlx` | `axa-sqlite-sqlx` | Raw SQL, embedded database |
| Database (SQLite) | `sqlite-seaorm` | `axa-sqlite-seaorm` | ORM, migrations |
| Database (SQLite) | `sqlite-diesel` | `axa-sqlite-diesel` | ORM, compile-time safety |
| Job Queue (Postgres) | `queue` | `axa-postgres-job-queue` | Background jobs with Postgres |
| Job Queue (MySQL) | N/A | `axa-mysql-job-queue` | Background jobs with MySQL |
| Job Queue (SQLite) | N/A | `axa-sqlite-job-queue` | Background jobs with SQLite |
| Authentication | PASETO | `axa-paseto-auth` | Stateless tokens, recommended |
| Authentication | Keycloak | `axa-keycloak-auth` | Keycloak integration |
| Observability | Tracing | `axa-tracing` | Logging with request tracking |
| Observability | OpenTelemetry | `axa-opentelemetry` | Distributed tracing export |
| Observability | Prometheus | `axa-prometheus-metrics` | Metrics collection |
| API Docs | OpenAPI | `axa-openapi-docs` | OpenAPI specification |
| API Docs | Swagger UI | `axa-swagger-ui` | Interactive API documentation |
| Caching | Redis | `axa-redis-cache` | Distributed cache |
| Caching | Memory | `axa-memory-cache` | In-process cache |
| Web | Session | `axa-web-session` | Cookie-based sessions |
| Web | Templates (Tera) | `axa-tera-templates` | HTML templates |
| Web | Templates (Maud) | `axa-maud-templates` | Compile-time HTML templates |
| Web | Pagination | `axa-cursor-pagination` | Cursor-based pagination |
| Outbox | Transactional | `axa-transactional-outbox` | Reliable event publishing |
| Outbox | Encryption | `axa-outbox-encryption` | Encrypt outbox payloads |
| Security | Idempotency | `axa-idempotency-key` | Prevent duplicate processing |
| Security | Rate Limit | `axa-request-rate-limit` | Request throttling |
| Versioning | Accepts-Version | `axa-accepts-version-negotiation` | API version negotiation |
| Test Helpers | N/A | `axa-test` | Request-level test utilities |

---

## Decision Trees

### Choosing a Database Approach

```
Do you need an ORM?
├── No → Use SQLx (raw SQL)
│   ├── Postgres → axa-postgres-sqlx
│   ├── MySQL → axa-mysql-sqlx
│   └── SQLite → axa-sqlite-sqlx
│
└── Yes
    ├── Do you need migrations?
    │   ├── No → Use Diesel (compile-time, no runtime deps)
    │   │   ├── Postgres → axa-postgres-diesel
    │   │   ├── MySQL → axa-mysql-diesel
    │   │   └── SQLite → axa-sqlite-diesel
    │   │
    │   └── Yes
    │       ├── Do you need down migrations?
    │       │   ├── Yes → Use SeaORM
    │       │   │   ├── Postgres → axa-postgres-seaorm
    │       │   │   ├── MySQL → axa-mysql-seaorm
    │       │   │   └── SQLite → axa-sqlite-seaorm
    │       │   └── No → Use Toasty (forward-only)
    │       │       ├── Postgres → axa-postgres-toasty
    │       │       └── MySQL → axa-mysql-toasty
    │       └── (SQLite Toasty not available)
    │
    └── Coming from Loco? → Use SeaORM (axa-postgres-seaorm)
```

### Choosing Authentication

```
Do you need third-party auth?
├── Yes, Keycloak → axa-keycloak-auth
│
└── No
    ├── Do you need PASETO tokens?
    │   ├── Yes → axa-paseto-auth
    │   └── No → Implement your own or use a third-party crate
    │
    └── Need sessions? → axa-web-session
```

### Choosing Caching

```
Do you need distributed caching?
├── Yes → axa-redis-cache
│
└── No (in-process only) → axa-memory-cache
```

### Choosing Job Queue

```
Which database are you using?
├── Postgres → axa-postgres-job-queue
├── MySQL → axa-mysql-job-queue
└── SQLite → axa-sqlite-job-queue

Note: All queues use the same axa-queue interface via features
```

### Choosing Templates

```
Which template engine?
├── Tera (string-based) → axa-tera-templates
└── Maud (compile-time) → axa-maud-templates
```

---

## Feature Categories Explained

### Core Features (Always Included)

- **Environment Detection**: `axa-environment` - Distinguish dev/test/prod
- **Problem Details**: `axa-problem` - RFC 9457 error handling
- **Routing**: Built-in route tables
- **Configuration**: Environment-based settings
- **Middleware**: Standard HTTP middleware stack
- **Health**: `/_health/live`, `/_health/ready`, `/_health/startup`
- **Logging**: Structured logging setup
- **Error Handling**: Generic errors, RFC 9457 problems

### Database Features

All database features provide:
- Connection pooling
- Health checks
- Configuration from environment
- Graceful shutdown

**Choose based on:**
- ORM vs raw SQL preference
- Migration needs
- Database type (Postgres, MySQL, SQLite)

### Authentication Features

- **axa-paseto-auth**: PASETO v4 tokens, local authentication
- **axa-keycloak-auth**: Keycloak/OIDC integration
- **axa-web-session**: Cookie-based sessions

**Note:** You can use multiple auth features together

### Observability Features

- **axa-tracing**: Request tracing, structured logging
- **axa-opentelemetry**: OpenTelemetry export
- **axa-prometheus-metrics**: Prometheus metrics

**Recommended:** Use all three for comprehensive observability

### API Documentation Features

- **axa-openapi-docs**: OpenAPI 3.0 specification generation
- **axa-swagger-ui**: Interactive API documentation

**Note:** These work best together

### Caching Features

- **axa-redis-cache**: Redis-based caching
- **axa-memory-cache**: In-memory caching

**Note:** Can be used independently or together

### Web Features

- **axa-web-session**: Session management
- **axa-tera-templates**: Tera template rendering
- **axa-maud-templates**: Maud template rendering
- **axa-cursor-pagination**: Cursor-based pagination

### Outbox Pattern Features

- **axa-transactional-outbox**: Transactional outbox pattern
- **axa-outbox-encryption**: Encryption for outbox payloads

**Note:** Outbox requires a database feature

### Security Features

- **axa-idempotency-key**: Idempotency key handling
- **axa-request-rate-limit**: Request rate limiting

### Versioning Features

- **axa-accepts-version-negotiation**: API version negotiation via headers

---

## Common Combinations

### REST API with Postgres

```toml
[dependencies]
axa = { version = "0.12", features = ["postgres-sqlx"] }
axa-postgres-sqlx = "0.12"
```

### REST API with Postgres + Auth + Observability

```toml
[dependencies]
axa = { version = "0.12", features = ["postgres-seaorm", "paseto", "tracing", "opentelemetry", "prometheus"] }
axa-postgres-seaorm = "0.12"
axa-paseto-auth = "0.12"
axa-opentelemetry = "0.12"
```

### Background Jobs with Postgres

```toml
[dependencies]
axa = { version = "0.12", features = ["queue"] }
axa-postgres-sqlx = "0.12"
axa-postgres-job-queue = "0.12"
```

### Full-featured Service

```toml
[dependencies]
axa = { 
  version = "0.12", 
  features = [
    "postgres-seaorm",
    "queue",
    "paseto",
    "tracing",
    "opentelemetry",
    "prometheus",
    "openapi",
    "session",
    "cursor-pagination",
    "idempotency",
    "versioning",
  ] 
}
axa-postgres-seaorm = "0.12"
axa-postgres-job-queue = "0.12"
axa-paseto-auth = "0.12"
axa-opentelemetry = "0.12"
```

---

## Migration Guide

### From Loco

If migrating from Loco, use these equivalent features:

| Loco | axa |
|------|-----|
| `loco_rs` | `axa` (core) + database feature |
| `loco_rs::model` | `axa::model` |
| `loco_rs::controller` | Your own handlers |
| SeaORM integration | `axa-postgres-seaorm` or `axa-mysql-seaorm` |
| Job queue | `axa-postgres-job-queue` |
| Idempotency | `axa-idempotency-key` |

See [help/from-loco-into-axa/](help/from-loco-into-axa/index.md) for detailed migration.

### From actix-web

| actix-web | axa |
|-----------|-----|
| `web::Data` | `AppContext` in Axum state |
| Middleware | `axa::middleware` |
| Error handling | `axa::Error` with RFC 9457 |

See [help/axa-for-actix-users/](help/axa-for-actix-users/index.md) for detailed migration.

---

## See Also

- [features/](features/index.md) - Complete feature list
- [quickstart/](quickstart/index.md) - Get started in 5 minutes
- [configuration/](configuration/index.md) - All configuration options
- [examples/](examples/index.md) - Real-world usage patterns
