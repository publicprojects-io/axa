# Architecture Overview

This document explains the architectural principles and design of axa.

---

## Core Principles

### 1. Library, Not Framework

axa is a **library**. This means:

- Your `main.rs` stays ordinary Rust
- You call functions directly, not implement traits
- Every piece can be read, replaced, or left out
- No lifecycle hooks, no code generation
- The control flow is visible from your code

**Contrast with Frameworks**:
```
Framework:     Your code -> Framework -> Behavior
                   (indirection, magic)

Library:       Your code -> axa functions -> Behavior
                   (direct, visible)
```

---

### 2. Fail Closed

In production, axa **refuses to start** with unsafe or invalid configuration:

- Missing `DATABASE_URL` in production → boot fails
- `CORS_ALLOWED_ORIGINS=*` in production → boot fails
- Invalid `REQUEST_TIMEOUT_SECONDS=abc` → boot fails
- Database URL without TLS in production → boot fails

This ensures that production deployments are always secure.

---

### 3. Audit-Friendly

Every request path is traceable:

```
Request → Router → Handler → Database
     ↓           ↓          ↓
  Logging     Middleware  Query
     ↓           ↓          ↓
  All visible in your code
```

No framework code runs behind your back. An auditor can read the entire path.

---

### 4. Generic Errors

Internal errors are **never** returned to clients:

```rust
// This error message stays in logs only
let result = sqlx::query("SELECT * FROM users")
    .fetch_all(&pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Database error");
        Error::Internal("Database error".to_string())
    })?;

// Client sees: {"type": "about:blank", "title": "Internal Server Error", ...}
// Client does NOT see: "relation \"users\" does not exist"
```

---

### 5. Spec-First Development

axa has a **single source of truth**: the specification in `spec/index.md`

1. Each requirement has an ID (e.g., `CFG-1`, `RTE-1`)
2. Each requirement names its test
3. Code conforms to the spec, not the other way around
4. A behavior change requires: spec edit + code edit + test edit

This ensures consistency and prevents regression.

---

## Component Architecture

### Core Components

```
┌─────────────────────────────────────────────────────────────────────┐
│                              axa (core)                            │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────────┐ │
│  │  routes.rs  │  │ config.rs  │  │  context.rs                 │ │
│  │  - Routes   │  │  - Config   │  │  - AppContext               │ │
│  │  - Groups   │  │  - Defaults │  │  - Shared state              │ │
│  │  - Mounted  │  │  - Env      │  │                             │ │
│  └─────────────┘  └─────────────┘  └─────────────────────────────┘ │
│                                                                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────────┐ │
│  │ middleware  │  │   error.rs   │  │  server.rs                  │ │
│  │  - Stack    │  │  - Error     │  │  - Connection limits        │ │
│  │  - Layers   │  │  - Result    │  │  - Graceful shutdown        │ │
│  │  - Apply    │  │  - Problem   │  │  - Signal handling          │ │
│  └─────────────┘  └─────────────┘  └─────────────────────────────┘ │
│                                                                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────────┐ │
│  │  health.rs  │  │  logger.rs  │  │  boot.rs (test only)        │ │
│  │  - Checks   │  │  - init     │  │  - TestApp                  │ │
│  │  - /_ping   │  │  - format   │  │                             │ │
│  │  - /_health │  │  - layers   │  │                             │ │
│  │  - /_ready  │  │             │  │                             │ │
│  └─────────────┘  └─────────────┘  └─────────────────────────────┘ │
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  prelude.rs - Re-exported types and traits                │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

### Optional Features (Subcrates)

```
┌─────────────────────────────────────────────────────────────────────┐
│                         axa (with features)                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐   │
│  │   Database      │  │   Job Queue     │  │  Authentication │   │
│  │                 │  │                 │  │                 │   │
│  │  ├─ sqlx         │  │  ├─ queue       │  │  ├─ paseto       │   │
│  │  ├─ sea-orm      │  │  └─ postgres-job│  │  └─ keycloak    │   │
│  │  └─ diesel       │  │                 │  │                 │   │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘   │
│                                                                      │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐   │
│  │   Observability  │  │   API Docs      │  │  Caching        │   │
│  │                 │  │                 │  │                 │   │
│  │  ├─ tracing      │  │  ├─ openapi     │  │  ├─ redis        │   │
│  │  ├─ opentelemetry│  │  └─ swagger-ui  │  │  └─ memory       │   │
│  │  └─ prometheus   │  │                 │  │                 │   │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘   │
│                                                                      │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐   │
│  │   Web           │  │   Outbox        │  │  Security       │   │
│  │                 │  │                 │  │                 │   │
│  │  ├─ session      │  │  ├─ outbox      │  │  ├─ idempotency │   │
│  │  ├─ tera         │  │  └─ encryption  │  │  └─ ratelimit   │   │
│  │  └─ cursor-pag   │  │                 │  │                 │   │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘   │
│                                                                      │
│  ┌─────────────────┐  ┌─────────────────┐                          │
│  │   Versioning    │  │   Test Helpers  │                          │
│  │                 │  │                 │                          │
│  │  └─ accepts-v   │  │  └─ axa-test    │                          │
│  └─────────────────┘  └─────────────────┘                          │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

Each feature is:
- Optional (enabled via Cargo features)
- A separate crate (but re-exported under `axa::`)
- Usable independently or together

---

## Request Flow

### Incoming Request

```
┌─────────────────────────────────────────────────────────────────────┐
│                           Request Lifecycle                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. Connection Accepted                                             │
│     └─ server.rs handles connection                                 │
│     └─ MAX_CONNECTIONS limits open connections                      │
│     └─ KEEP_ALIVE_TIMEOUT_SECONDS for idle connections              │
│                                                                      │
│  2. Header Read                                                     │
│     └─ HEADER_READ_TIMEOUT_SECONDS limits header read time          │
│     └─ TRUSTED_PROXIES determines client IP from X-Forwarded-For     │
│     └─ ClientAddr extractor gets the real client address            │
│                                                                      │
│  3. Request ID Assigned                                            │
│     └─ From x-request-id header (sanitized)                        │
│     └─ Or new UUID v4                                               │
│     └─ Stored in request extensions                                 │
│     └─ Added to every log line                                      │
│     └─ Returned in response x-request-id header                     │
│                                                                      │
│  4. Middleware Stack Applied (Outside → Inside)                     │
│     └─ Request ID layer                                             │
│     └─ Tracing layer (logs method, route template)                   │
│     └─ Security headers layer (HSTS, CSP, X-Frame-Options, etc.)    │
│     └─ CORS layer (if origins configured)                           │
│     └─ Timeout layer (REQUEST_TIMEOUT_SECONDS)                     │
│     └─ Conditional request layer (ETag, 304)                         │
│     └─ Compression layer (if enabled)                               │
│     └─ Body limit layer (BODY_LIMIT_BYTES)                         │
│     └─ Panic catch layer (turns panics into 500)                    │
│                                                                      │
│  5. Route Matching                                                  │
│     └─ Routes checked in order                                      │
│     └─ Path parameters extracted                                    │
│     └─ Query parameters extracted                                   │
│     └─ Request body extracted (JSON, etc.)                          │
│                                                                      │
│  6. Handler Execution                                               │
│     └─ Your handler code runs                                       │
│     └─ Returns Result<Response>                                     │
│     └─ Errors automatically converted to problems                   │
│                                                                      │
│  7. Response Processing                                             │
│     └─ Errors converted to RFC 9457 problem details                 │
│     └─ Success responses passed through                            │
│     └─ Conditional responses (304) generated if ETag matches       │
│     └─ Security headers added (if not set by handler)               │
│     └─ CORS headers added (if applicable)                          │
│     └─ Request ID added to response                                 │
│     └─ Compression applied (if enabled)                            │
│                                                                      │
│  8. Response Sent                                                   │
│     └─ SEND_STALL_TIMEOUT_SECONDS limits slow clients               │
│     └─ Connection returned to pool or closed                        │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

### Middleware Stack Order

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Middleware Stack (Top to Bottom)                 │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Your custom layers (added before apply)                     │   │
│  │  - Run INSIDE the standard stack                              │   │
│  │  - Get request ID, security headers, CORS                   │   │
│  │  - Refusals have full headers                                │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Request ID Layer                                          │   │
│  │  - Keeps caller's x-request-id (sanitized)                   │   │
│  │  - Mints UUID if none provided                              │   │
│  │  - Sets request header to match response                     │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Tracing Layer                                              │   │
│  │  - Logs request start                                       │   │
│  │  - Adds request ID, method, route template to every log      │   │
│  │  - NEVER logs path or query (may contain PII)                │   │
│  │  - Logs response completion                                 │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Security Headers Layer                                    │   │
│  │  - x-content-type-options: nosniff                          │   │
│  │  - x-frame-options: DENY                                    │   │
│  │  - referrer-policy: no-referrer                             │   │
│  │  - strict-transport-security (if HSTS enabled)             │   │
│  │  - content-security-policy (if configured)                  │   │
│  │  - Handler's own headers win over defaults                   │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  CORS Layer                                                │   │
│  │  - Only added if CORS_ALLOWED_ORIGINS is set                 │   │
│  │  - Production: no CORS unless origins listed                  │   │
│  │  - Development: allows all origins                           │   │
│  │  - Refuses * in production                                   │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Timeout Layer                                             │   │
│  │  - REQUEST_TIMEOUT_SECONDS (default: 60s in prod)           │   │
│  │  - Returns 408 Request Timeout                              │   │
│  │  - Per-route overrides possible                             │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Conditional Request Layer                                 │   │
│  │  - Handles If-None-Match (304 responses)                    │   │
│  │  - Handles If-Match (412 responses)                         │   │
│  │  - Weak and strong ETag comparison                          │   │
│  │  - Hashes response bodies for ETag generation               │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Compression Layer                                         │   │
│  │  - Gzip, Brotli, Zstd (if enabled)                          │   │
│  │  - Only compresses if client accepts                        │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Body Limit Layer                                          │   │
│  │  - BODY_LIMIT_BYTES (default: 2MB)                          │   │
│  │  - Returns 413 Payload Too Large                             │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Panic Catch Layer                                        │   │
│  │  - Turns handler panics into 500                            │   │
│  │  - Logs panic without message (security)                    │   │
│  │  - Still applies security headers                           │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Your Router / Handlers                                    │   │
│  │  - Your route definitions                                   │   │
│  │  - Your handler code                                        │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                              ↓                                   │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  Your custom layers (added after apply)                     │   │
│  │  - Run OUTSIDE the standard stack                           │   │
│  │  - Refusals have NO request ID, security headers, CORS      │   │
│  │  - Body is compressed                                       │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Background Job Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Background Job Lifecycle                       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Queue Mode (BackgroundQueue)                                       │
│  ──────────────────────────────                                      │
│                                                                      │
│  1. Job Enqueued                                                    │
│     └─ perform_later(&ctx, args)                                    │
│     └─ Serialized and stored in axa_queue table                      │
│     └─ Returns job ID immediately                                   │
│                                                                      │
│  2. Worker Claims Job                                               │
│     └─ Worker loops query for ready jobs                            │
│     └─ FOR UPDATE SKIP LOCKED ensures only one claim                 │
│     └─ Job state changes: queued → running                         │
│     └─ Heartbeat started                                            │
│                                                                      │
│  3. Job Executed                                                   │
│     └─ Worker deserializes arguments                                │
│     └─ Your perform() method called                                 │
│     └─ Heartbeat sent periodically                                  │
│                                                                      │
│  4. Job Completion                                                 │
│     └─ On success: state → completed                               │
│     └─ On failure: state → failed, error stored                     │
│     └─ On panic: caught, state → failed                            │
│     └─ On timeout: state → failed                                   │
│                                                                      │
│  5. Retry Logic                                                    │
│     └─ Default: 1 attempt (no retry)                                │
│     └─ max_attempts sets retry count                                │
│     └─ Exponential backoff: 1s, 2s, 4s, ...                        │
│     └─ Max backoff: 2^(attempts-1) seconds                          │
│                                                                      │
│  6. Stale Job Detection                                            │
│     └─ Worker reports in every heartbeat_interval                   │
│     └─ After stale_minutes without heartbeat: job requeued         │
│     └─ If no attempts left: job fails permanently                  │
│                                                                      │
│  Foreground Modes (BackgroundAsync, ForegroundBlocking)            │
│  ─────────────────────────────────────────────────────────────    │
│                                                                      │
│  BackgroundAsync:                                                   │
│  - Spawns job on current runtime                                   │
│  - Returns immediately                                              │
│  - No persistence                                                   │
│                                                                      │
│  ForegroundBlocking:                                               │
│  - Runs job before returning                                       │
│  - Reports failure immediately                                      │
│  - Used in tests                                                    │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Outbox Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                           Outbox Lifecycle                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  1. Event Written                                                   │
│     └─ outbox::write(&mut tx, &event)                              │
│     └─ Written on caller's transaction                              │
│     └─ Commits or rolls back with the business change               │
│     └─ State: inserted                                              │
│                                                                      │
│  2. Event Claimed                                                   │
│     └─ Relay queries for unclaimed events                           │
│     └─ FOR UPDATE SKIP LOCKED ensures one claim                     │
│     └─ State: claimed                                               │
│                                                                      │
│  3. Decision Made                                                   │
│     └─ decide(&event, &claim_tx) in claim's transaction             │
│     └─ Returns Decision::Deliver, Hold, or Refuse("code")          │
│     └─ Refuse is final - event never retried                       │
│     └─ Hold charges nothing - event retried later                   │
│                                                                      │
│  4. Publish Attempt                                                 │
│     └─ publish(&event, &attempt_tx) outside transaction             │
│     └─ Your code calls broker client                                │
│     └─ Returns Ok(()) on success, Failure on error                 │
│     └─ Failure::unavailable("code") - retry later                 │
│     └─ Failure::rejected("code") - event fails                     │
│                                                                      │
│  5. Record Kept                                                    │
│     └─ record::outcome(&event, &result, &record_tx)                 │
│     └─ Runs in each outcome's transaction                          │
│     └─ Write your audit row here                                    │
│     └─ State: delivered, failed, or refused                        │
│                                                                      │
│  Key Guarantees:                                                    │
│  - At-least-once delivery                                           │
│  - Events and attempts only inserted (never updated/deleted)       │
│  - Database triggers refuse all changes to outbox tables           │
│  - Consumer must deduplicate on event_id                            │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Data Flow Patterns

### Request-Response Flow

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Client    │────▶│   Router    │────▶│  Handler    │
└─────────────┘     └─────────────┘     └─────────────┘
       ▲                   ▲                   ▲
       │                   │                   │
       │                   ▼                   │
       │             ┌─────────────┐           │
       │             │ Middleware  │           │
       │             └─────────────┘           │
       │                   ▲                   │
       └───────────────────┼───────────────────┘
                           │
                    ┌──────▼──────┐
                    │   Response  │
                    └─────────────┘
```

### Database Flow

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Handler    │────▶│    Pool     │────▶│ PostgreSQL  │
└─────────────┘     └─────────────┘     └─────────────┘
       ▲                   ▲                   ▲
       │                   │                   │
    Result              Query              Result
       │                   │                   │
       └───────────────────┼───────────────────┘
                           │
                    ┌──────▼──────┐
                    │ Connection   │
                    │  (reused)    │
                    └─────────────┘
```

### Job Queue Flow

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Handler    │────▶│    Queue    │────▶│ PostgreSQL  │
│ (enqueue)   │     │ (axa_queue) │     │  (storage)   │
└─────────────┘     └─────────────┘     └─────────────┘
                                                  ▲
                                                  │
                           ┌──────────────────────┘
                           │
                    ┌──────▼──────┐
                    │   Workers   │
                    │  (claim &   │
                    │   execute)   │
                    └─────────────┘
```

---

## Directory Structure

```
axa/
├── Cargo.toml                 # Workspace manifest
├── src/
│   ├── lib.rs                 # Main library entry point
│   ├── routes.rs              # Route table definitions
│   ├── config.rs              # Configuration parsing
│   ├── context.rs             # AppContext and state
│   ├── middleware.rs          # Standard middleware stack
│   ├── server.rs              # Server and connection handling
│   ├── health.rs              # Health check routes
│   ├── logger.rs              # Logging setup
│   ├── error.rs               # Error types and handling
│   ├── format.rs              # Response helpers
│   ├── conditional.rs          # Conditional request handling
│   ├── extract.rs              # Custom extractors
│   ├── cli.rs                 # Command line parsing
│   ├── task.rs                # Task system
│   ├── worker.rs              # Background worker system
│   ├── queue.rs               # Queue integration (feature)
│   ├── db.rs                  # Database helpers
│   ├── store.rs               # In-process store
│   ├── boot.rs                # Test boot helpers
│   └── prelude.rs             # Re-exports
│
├── crates/
│   ├── axa-environment/        # Environment detection (no deps)
│   ├── axa-problem-details... # RFC 9457 problem details
│   ├── axa-sqlx/              # sqlx integration
│   ├── axa-seaorm/            # SeaORM integration
│   ├── axa-diesel/            # Diesel integration
│   ├── axa-postgres-job-queue # Job queue engine
│   ├── axa-transactional-outbox # Outbox implementation
│   ├── axa-paseto-auth/       # PASETO authentication
│   ├── axa-keycloak-auth/     # Keycloak authentication
│   ├── axa-web-session/       # Session management
│   ├── axa-tera-templates/    # Tera template rendering
│   ├── axa-redis-cache/       # Redis caching
│   ├── axa-request-rate-limit # Rate limiting
│   ├── axa-idempotency-key/   # Idempotency keys
│   ├── axa-cursor-pagination/ # Cursor pagination
│   ├── axa-openapi-docs/     # OpenAPI documentation
│   ├── axa-prometheus-metrics # Prometheus metrics
│   ├── axa-opentelemetry/     # OpenTelemetry integration
│   ├── axa-accepts-version... # API versioning
│   ├── axa-tracing/           # Advanced tracing
│   └── axa-test/              # Test helpers
│
├── docs/
│   ├── QUICKSTART.md           # This file
│   ├── FEATURES.md             # Feature selection guide
│   ├── CONFIGURATION.md        # Configuration reference
│   ├── EXAMPLES.md             # Examples and recipes
│   └── ARCHITECTURE.md          # Architecture overview
│
├── spec/
│   └── index.md                # Single source of truth
│
├── help/
│   ├── upgrading-to-0.3/      # Upgrade guide
│   ├── from-loco-into-axa/     # Loco migration
│   ├── from-loco-seaorm...    # SeaORM migration
│   ├── background-jobs/       # Job documentation
│   ├── transactional-outbox/   # Outbox documentation
│   └── idempotency-keys/       # Idempotency documentation
│
├── skill/
│   ├── axa-skill/              # For users of axa
│   └── axa-maintainer-skill/   # For maintainers of axa
│
├── .github/
│   └── workflows/
│       └── ci.yml              # CI configuration
│
├── AGENTS.md                  # Agent instructions
├── CLAUDE.md                   # Claude Code instructions
├── llms.txt                    # LLM index
├── llms.json                   # LLM index (machine-readable)
├── CHANGELOG.md                # Release notes
└── README.md                   # Project overview
```

---

## Design Decisions

### Why No Framework Features?

axa deliberately excludes certain features to maintain auditability:

| Feature | Why Not | Alternative |
|---------|----------|-------------|
| Route macros | Hides route definitions | Explicit route tables |
| Dependency injection | Hides dependencies | Explicit function parameters |
| Code generation | Hides generated code | Hand-written code |
| Plugin system | Hides plugin behavior | Explicit composition |
| Config files | Hides config source | Environment + code defaults |
| Database abstraction | Hides SQL | Direct sqlx/SeaORM usage |
| Generic CRUD | Hides business logic | Hand-written handlers |

---

### Why Spec-First?

1. **Consistency**: All behavior is documented in one place
2. **Testability**: Each requirement has a test
3. **Maintainability**: Changes require updating spec, code, and tests
4. **Auditability**: Auditors can verify behavior matches specification
5. **Discipline**: Prevents ad-hoc changes without documentation

---

### Why Fail Closed?

1. **Security**: Prevents accidental insecure deployments
2. **Reliability**: Catches configuration errors early
3. **Auditability**: Forces explicit configuration
4. **Safety**: Defaults are always safe, even if not optimal

---

## See Also

- [Quick Start](QUICKSTART.md) - Get started in 5 minutes
- [Feature Selection](FEATURES.md) - Choose the right features
- [Configuration](CONFIGURATION.md) - All configuration options
- [Examples](EXAMPLES.md) - Real-world patterns
- [Full API Docs](../README.md) - Complete reference
