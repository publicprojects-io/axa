# Feature Selection Guide

This guide helps you choose which axa features and crates to use for your project.

---

## Understanding axa's Structure

axa follows a **modular design**: the main `axa` crate provides core functionality, while optional features add specific capabilities through separate crates.

```
┌─────────────────────────────────────────┐
│              axa (core)                 │
│  - Routes, Middleware, Config, Errors   │
│  - Health checks, Logging, Server       │
│  - Background jobs, Tasks               │
└─────────────────┬───────────────────────┘
                  │
    ┌─────────────┼─────────────┐
    ▼             ▼             ▼
┌─────────┐   ┌─────────┐   ┌─────────┐
│  sqlx   │   │  queue  │   │ session │
│ (DB)    │   │ (Jobs)  │   │ (Auth)  │
└─────────┘   └─────────┘   └─────────┘
    ...         ...         ...
```

---

## Decision Tree: Which Features Do I Need?

### Step 1: Database Access

**Do you need a database?**

- **No database needed** → Use core `axa` only
- **PostgreSQL with sqlx (RECOMMENDED)** → Add `axa = { version = "0.10", features = ["sqlx"] }`
- **PostgreSQL with SeaORM** → Add `axa = { version = "0.10", features = ["sea-orm"] }`
- **PostgreSQL with Diesel (no native libpq)** → Add `axa = { version = "0.10", features = ["diesel"] }`

**Recommendation**: Use `sqlx` unless you have existing SeaORM code. Diesel is for environments where you cannot link `libpq`.

---

### Step 2: Background Jobs

**Do you need to run background jobs?**

- **No** → Skip
- **Yes, with PostgreSQL-backed queue** → Add `axa = { version = "0.10", features = ["queue"] }`
- **Yes, with in-process execution** → Use `BackgroundAsync` worker mode (built into core)

The `queue` feature provides:
- Job persistence in PostgreSQL
- Worker pool management
- Retry with exponential backoff
- Scheduled jobs with `Queue::every()`

---

### Step 3: Transactional Outbox

**Do you need reliable event publishing?**

- **No** → Skip
- **Yes, basic outbox** → Add `axa = { version = "0.10", features = ["outbox"] }`
- **Yes, with encrypted payloads** → Add `axa = { version = "0.10", features = ["outbox-encryption"] }`

The outbox feature provides:
- Transactional event storage
- At-least-once delivery guarantees
- Relay process that publishes to your broker
- Complete audit trail of deliveries and failures

**Note**: You must provide your own broker client.

---

### Step 4: Authentication

**What authentication method do you need?**

- **None / Custom** → Skip
- **PASETO tokens (RECOMMENDED for new projects)** → Add `axa = { version = "0.10", features = ["paseto"] }`
- **Keycloak OAuth2 / OIDC** → Add `axa = { version = "0.10", features = ["keycloak"] }`
- **Both PASETO and Keycloak** → Add both features

**Comparison**:

| Feature | Pros | Cons | Best For |
|---------|------|------|----------|
| `paseto` | Simple, no external dependencies, supports ABAC | Manual token issuance | Microservices, internal APIs |
| `keycloak` | Full OAuth2/OIDC, external identity provider | Requires Keycloak instance | User-facing applications, SSO |

---

### Step 5: Web Sessions

**Do you need browser-based sessions?**

- **No (API only)** → Skip
- **Yes, with cookies** → Add `axa = { version = "0.10", features = ["session"] }`

The `session` feature provides:
- Secure, encrypted sessions
- Form token protection (CSRF)
- Session renewal
- Anonymous visitor support (no storage until sign-in)

---

### Step 6: API Versioning

**Do you need API versioning?**

- **No** → Skip
- **Yes, header-based** → Add `axa = { version = "0.10", features = ["versioning"] }`

The `versioning` feature provides:
- `Accepts-Version` header negotiation
- Graceful version deprecation
- Automatic 406 responses for unsupported versions

---

### Step 7: Caching

**Do you need caching?**

- **No** → Skip
- **Yes, Redis cache** → Add `axa = { version = "0.10", features = ["redis"] }`

The `redis` feature provides:
- Mandatory TTL on all entries
- Request-scoped cache loading
- Graceful degradation during outages
- Memory-backed alternative for testing

---

### Step 8: Rate Limiting

**Do you need request rate limiting?**

- **No** → Skip
- **Yes, in-process** → Add `axa = { version = "0.10", features = ["ratelimit"] }`

The `ratelimit` feature provides:
- Per-client limits
- Per-key limits
- Failed login lockout
- IPv6 /64 subnet grouping
- Bounded memory usage

---

### Step 9: Pagination

**Do you need to paginate collections?**

- **No** → Skip
- **Yes, cursor-based** → Add `axa = { version = "0.10", features = ["cursor-pagination"] }`

The `cursor-pagination` feature provides:
- Opaque, signed cursors
- No total counts (security feature)
- `Link: rel="next"` headers
- LIKE wildcard escaping

**Note**: axa does NOT provide offset pagination due to security concerns (total counts can leak information).

---

### Step 10: Templates

**Do you need HTML templates?**

- **No** → Skip
- **Yes, Tera templates** → Add `axa = { version = "0.10", features = ["tera"] }`

The `tera` feature provides:
- Safe HTML escaping by default
- Custom error handling
- Template inheritance

---

### Step 11: Observability

**Do you need observability?**

- **No** → Skip
- **Yes, Prometheus metrics** → Add `axa = { version = "0.10", features = ["prometheus"] }`
- **Yes, OpenTelemetry tracing** → Add `axa = { version = "0.10", features = ["opentelemetry"] }`
- **Yes, advanced tracing** → Add `axa = { version = "0.10", features = ["tracing"] }`

| Feature | Purpose |
|---------|---------|
| `prometheus` | Metrics exposition (`/metrics`) |
| `opentelemetry` | OTLP export (traces + metrics) |
| `tracing` | Request tracking, diagnostics, redaction |

---

### Step 12: API Documentation

**Do you need API documentation?**

- **No** → Skip
- **Yes, OpenAPI docs** → Add `axa = { version = "0.10", features = ["openapi"] }`

The `openapi` feature provides:
- OpenAPI document serving
- Swagger UI (pinned version)
- Drift detection between routes and spec

---

### Step 13: Idempotency

**Do you need idempotent writes?**

- **No** → Skip
- **Yes, idempotency keys** → Add `axa = { version = "0.10", features = ["idempotency"] }`

The `idempotency` feature provides:
- `Idempotency-Key` header handling
- 24-hour TTL by default
- At-most-once semantics
- Sweeper for expired keys

---

## Common Feature Combinations

### REST API with PostgreSQL

```toml
[dependencies]
axa = { version = "0.10", features = ["sqlx", "cursor-pagination", "ratelimit"] }
```

Perfect for: JSON APIs, microservices, internal tools

---

### Full-Stack Web Application

```toml
[dependencies]
axa = { 
    version = "0.10", 
    features = [
        "sqlx",
        "session", 
        "tera",
        "cursor-pagination",
        "ratelimit",
        "openapi"
    ] 
}
```

Perfect for: User-facing applications, admin panels

---

### Event-Driven Service

```toml
[dependencies]
axa = { 
    version = "0.10", 
    features = ["sqlx", "queue", "outbox"] 
}
```

Perfect for: CQRS, event sourcing, reliable messaging

---

### High-Security API

```toml
[dependencies]
axa = { 
    version = "0.10", 
    features = [
        "sqlx",
        "paseto",
        "idempotency",
        "outbox-encryption",
        "opentelemetry"
    ] 
}
```

Perfect for: Financial services, healthcare, government

---

## Feature Comparison Matrix

| Feature | DB | Jobs | Auth | Sessions | Cache | Rate Limit | Versioning | Docs | Templates | Tracing | Metrics | Outbox |
|---------|----|------|------|----------|-------|------------|-----------|------|----------|---------|---------|--------|
| `sqlx` | ✅ | | | | | | | | | | | |
| `sea-orm` | ✅ | | | | | | | | | | | |
| `diesel` | ✅ | | | | | | | | | | | |
| `queue` | ✅ | ✅ | | | | | | | | | | |
| `outbox` | ✅ | ✅ | | | | | | | | | | ✅ |
| `outbox-encryption` | ✅ | ✅ | | | | | | | | | | ✅ |
| `paseto` | | | ✅ | | | | | | | | | |
| `keycloak` | | | ✅ | | | | | | | | | |
| `session` | | | | ✅ | | | | | | | | |
| `redis` | | | | | ✅ | | | | | | | |
| `ratelimit` | | | | | | ✅ | | | | | | |
| `versioning` | | | | | | | ✅ | | | | | |
| `openapi` | | | | | | | | ✅ | | | | |
| `tera` | | | | | | | | | ✅ | | | |
| `tracing` | | | | | | | | | | ✅ | | |
| `prometheus` | | | | | | | | | | | ✅ | |
| `opentelemetry` | | | | | | | | | | ✅ | ✅ | |
| `idempotency` | ✅ | | | | | | | | | | | |
| `cursor-pagination` | | | | | | | | | | | | |

---

## Dependency Impact

Each feature adds dependencies. Here's the impact of each:

| Feature | Additional Dependencies | Notes |
|---------|------------------------|-------|
| `sqlx` | sqlx | Heavy, but well-maintained |
| `sea-orm` | sea-orm | Large dependency tree |
| `diesel` | diesel | No native libpq |
| `queue` | axa-postgres-job-queue | Requires PostgreSQL |
| `outbox` | axa-transactional-outbox | Requires PostgreSQL |
| `paseto` | axa-paseto-auth | Minimal dependencies |
| `keycloak` | axa-keycloak-auth | Minimal dependencies |
| `session` | axa-web-session, axum-session | |
| `redis` | axa-redis-cache | |
| `ratelimit` | axa-request-rate-limit | |
| `versioning` | axa-accepts-version-negotiation | |
| `openapi` | axa-openapi-docs | |
| `tera` | axa-tera-templates | |
| `tracing` | axa-tracing | |
| `prometheus` | axa-prometheus-metrics | |
| `opentelemetry` | axa-opentelemetry | |
| `idempotency` | axa-idempotency-key | Requires PostgreSQL |
| `cursor-pagination` | axa-cursor-pagination | |

**Recommendation**: Only enable features you actually need. Each adds to compile time, binary size, and audit surface.

---

## Migration Notes

### Upgrading from axa 0.9

See [UPGRADING.md](../help/upgrading-to-0.3/index.md) for details.

**Key changes**:
- The `testing` feature is now the `axa-test` crate
- Thirteen crates were renamed (e.g., `axa-ratelimit` → `axa-request-rate-limit`)

### From Loco

See [FROM_LOCO.md](../help/from-loco-into-axa/index.md) for a complete migration guide.

---

## Next Steps

1. **Start small**: Begin with core `axa` only
2. **Add features as needed**: Don't enable everything at once
3. **Test each feature**: Some features require PostgreSQL or other services
4. **Monitor dependencies**: Use `cargo tree` to understand what you're pulling in
5. **Review security**: Each feature has its own security considerations

---

## See Also

- [Quick Start](QUICKSTART.md) - Get started in 5 minutes
- [Configuration](CONFIGURATION.md) - All environment variables
- [Examples](EXAMPLES.md) - Real-world patterns
- [Full API Docs](../README.md) - Complete reference
