# Configuration Guide

This document describes all configuration options available in axa, their defaults, and how they interact with the environment.

---

## Overview

axa uses a **code-first, environment-override** approach to configuration:

1. **Defaults** are defined in code using the `Defaults` struct
2. **Environment variables** override those defaults
3. **Production fails closed** - invalid or missing required values stop the boot

```rust
use axa::{Environment, config::{Config, Defaults}};

let defaults = Defaults {
    database: "my_service",
    port: 3000,
    ..Defaults::default()
};

let environment = Environment::from_env();
let config = Config::from_env(&environment, &defaults)?;
```

---

## Environment Detection

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `APP_ENV` | `development`, `test`, `production`, `staging`, etc. | `development` (debug build) or `production` (release build) | Names the environment |
| `LOCO_ENV` | Same as above | Used if `APP_ENV` is not set | Legacy Loco variable |

**Priority**: `APP_ENV` > `LOCO_ENV` > build type

**Examples**:
```bash
# Development
APP_ENV=development cargo run

# Production
APP_ENV=production cargo run

# Staging
APP_ENV=staging cargo run
```

**Note**: Any environment name other than `development` or `test` is treated as production-like (fails closed).

---

## Server Configuration

### `Server` Struct Fields

| Field | Environment Variable | Default | Description |
|-------|---------------------|---------|-------------|
| `port` | `PORT` | 1241 | Listening port (a=1, x=24, a=1) |
| `binding` | `BINDING` | `127.0.0.1` | Listening address |
| `host` | `HOST` | `http://localhost:1241` | Public base URL |
| `shutdown_timeout_seconds` | `SHUTDOWN_TIMEOUT_SECONDS` | 25 | Graceful shutdown timeout |
| `header_read_timeout_seconds` | `HEADER_READ_TIMEOUT_SECONDS` | 30 | Client header read timeout |
| `keep_alive_timeout_seconds` | `KEEP_ALIVE_TIMEOUT_SECONDS` | 75 | Idle connection timeout |
| `send_stall_timeout_seconds` | `SEND_STALL_TIMEOUT_SECONDS` | 60 | Client read stall timeout |

---

### Server Defaults by Environment

| Setting | Development | Test | Production |
|---------|-------------|------|------------|
| Port | From `Defaults` or `PORT` | From `Defaults` or `PORT` | From `Defaults` or `PORT` |
| Compression | Off | Off | On |
| Request timeout | Off | Off | 60 seconds |
| HSTS | Off | Off | On |
| CORS | Any origin | Any origin | None until listed |
| Max connections | 10,000 | 10,000 | 10,000 |

---

## HTTP Configuration

### `Http` Struct Fields

| Field | Environment Variable | Default | Description |
|-------|---------------------|---------|-------------|
| `body_limit_bytes` | `BODY_LIMIT_BYTES` | 2 MB | Maximum request body size |
| `timeout_seconds` | `REQUEST_TIMEOUT_SECONDS` | 0 (off) in dev/test, 60 in prod | Request timeout |
| `hsts_max_age_seconds` | `HSTS_MAX_AGE_SECONDS` | 31,536,000 (1 year) | HSTS header max-age |
| `compression` | `HTTP_COMPRESSION` | `true` in production | Enable compression |
| `csp` | `HTTP_CSP` | `default-src 'none'; frame-ancestors 'none'` in production | Content Security Policy |
| `cors_allowed_origins` | `CORS_ALLOWED_ORIGINS` | All origins in dev/test, none in prod | CORS allowed origins |
| `timeout_overrides` | N/A | Empty | Per-route timeout overrides |
| `cross_site_paths` | N/A | Empty | Paths, matched exactly, where the cross-site write guard lets a cross-site write through |

---

### CORS Configuration

**Development/Test**: Any origin is allowed by default.

**Production**: No CORS headers are sent until origins are explicitly listed.

```bash
# Allow specific origins
CORS_ALLOWED_ORIGINS=https://app.example.com,https://admin.example.com

# Allow all (refused in production!)
CORS_ALLOWED_ORIGINS=*
```

A listed origin may also write from another site: the cross-site write guard, which
refuses an unsafe method or a WebSocket upgrade from another site with `403`, lets it
through.

---

### Compression Configuration

Compression is enabled in production by default. Supported algorithms:
- Gzip
- Brotli
- Zstd

```bash
# Disable compression
HTTP_COMPRESSION=false

# Enable specific algorithms (comma-separated)
# Note: This is controlled by tower-http, not a direct axa setting
```

---

## Database Configuration

### Connection Settings

| Field | Environment Variable | Default | Description |
|-------|---------------------|---------|-------------|
| `url` | `DATABASE_URL` | Computed from defaults | Connection string |
| `min_connections` | `DB_MIN_CONNECTIONS` | 1 | Minimum pool connections |
| `max_connections` | `DB_MAX_CONNECTIONS` | 10 | Maximum pool connections |
| `connect_timeout` | `DB_CONNECT_TIMEOUT` | 5000 ms | Connection timeout |
| `idle_timeout` | `DB_IDLE_TIMEOUT` | 600000 ms (10 min) | Idle connection timeout |
| `statement_logging` | `DB_LOGGING` | `false` | Log SQL statements |
| `auto_migrate` | `DB_AUTO_MIGRATE` | `true` in dev/test, `false` in prod | Auto-run migrations at boot |
| `allow_insecure` | `DB_ALLOW_INSECURE` | `false` | Allow non-TLS connections in production |

---

### Database URL by Environment

| Environment | URL Pattern | Example |
|-------------|-------------|---------|
| Development | `postgres://postgres:postgres@localhost:5432/{database}_development` | `postgres://postgres:postgres@localhost:5432/my_service_development` |
| Test | `postgres://postgres:postgres@localhost:5432/{database}_test` | `postgres://postgres:postgres@localhost:5432/my_service_test` |
| Production | **Required** | `postgres://user:pass@host:port/database?sslmode=require` |

**Production Requirement**: The `DATABASE_URL` must use TLS (`sslmode=require`, `verify-ca`, or `verify-full`) unless `DB_ALLOW_INSECURE=true`.

---

### Connection Pool Settings

```rust
use axa::config::Defaults;

let defaults = Defaults {
    max_connections: 20,      // Max connections in pool
    connect_timeout: 10_000,   // 10 seconds
    idle_timeout: 300_000,    // 5 minutes
    ..Defaults::default()
};
```

Or via environment:
```bash
DB_MAX_CONNECTIONS=20
DB_CONNECT_TIMEOUT=10000
DB_IDLE_TIMEOUT=300000
```

---

## Logging Configuration

### `Logger` Struct Fields

| Field | Environment Variable | Default | Description |
|-------|---------------------|---------|-------------|
| `level` | `LOG_LEVEL` | `debug` in dev, `info` in prod | Log level |
| `format` | `LOG_FORMAT` | `compact` in dev, `json` in prod | Log format |
| `enable` | `LOG_ENABLE` | `true` | Enable logging |
| `filter` | `LOG_FILTER` | Service name + shared modules | Log filter |

---

### Log Levels

Valid values: `error`, `warn`, `info`, `debug`, `trace`

```bash
LOG_LEVEL=debug
LOG_LEVEL=info
```

---

### Log Formats

| Format | Description | Example |
|--------|-------------|---------|
| `compact` | Single-line, colored | `[2026-11-09T10:00:00Z INFO  my_service] Request completed` |
| `json` | JSON structured | `{"timestamp":"2026-11-09T10:00:00Z","level":"INFO","target":"my_service",...}` |
| `full` | Multi-line, detailed | Verbose output with all fields |
| `pretty` | Multi-line, colored | Human-readable with colors |

```bash
LOG_FORMAT=json
LOG_FORMAT=compact
```

---

### RUST_LOG Integration

`RUST_LOG` takes precedence over axa's logging configuration:

```bash
# Override axa's logging
RUST_LOG=my_service=debug,tower_http=warn

# Disable axa's logging entirely
RUST_LOG=
```

---

## Worker Configuration

### `WorkerMode`

| Mode | Description | Environment Variable | Default in Dev/Test | Default in Prod |
|------|-------------|---------------------|---------------------|------------------|
| `ForegroundBlocking` | Run jobs synchronously | `WORKER_MODE=ForegroundBlocking` | Yes | No |
| `BackgroundAsync` | Spawn jobs on runtime | `WORKER_MODE=BackgroundAsync` | No | No |
| `BackgroundQueue` | Queue jobs in PostgreSQL | `WORKER_MODE=BackgroundQueue` | No | Yes (if queue feature enabled) |

```bash
WORKER_MODE=BackgroundAsync
```

---

## Proxy Configuration

### Trusted Proxies

| Field | Environment Variable | Default | Description |
|-------|---------------------|---------|-------------|
| `trusted_proxies` | `TRUSTED_PROXIES` | Empty (ignore X-Forwarded-For) | Comma-separated list of proxy IPs/CIDRs |

```bash
# Single IP
TRUSTED_PROXIES=10.0.0.1

# Multiple IPs
TRUSTED_PROXIES=10.0.0.1,10.0.0.2

# CIDR ranges
TRUSTED_PROXIES=10.0.0.0/24,172.16.0.0/12

# IPv6
TRUSTED_PROXIES=::1,2001:db8::/32
```

**Effect**: When set, axa will:
1. Parse `X-Forwarded-For` header
2. Trust only entries from configured proxies
3. Use the rightmost untrusted entry as the client IP

---

## All Environment Variables

### Core Variables

| Variable | Type | Required in Prod | Default | Description |
|----------|------|------------------|---------|-------------|
| `APP_ENV` | String | No | `development` or `production` | Environment name |
| `LOCO_ENV` | String | No | (falls back to APP_ENV) | Legacy Loco variable |
| `PORT` | Number | No | 1241 | Server port |
| `BINDING` | String | No | `127.0.0.1` | Server binding |
| `HOST` | String | No | `http://localhost:PORT` | Public URL |

### Server Variables

| Variable | Type | Required in Prod | Default | Description |
|----------|------|------------------|---------|-------------|
| `SHUTDOWN_TIMEOUT_SECONDS` | Number | No | 25 | Graceful shutdown timeout |
| `HEADER_READ_TIMEOUT_SECONDS` | Number | No | 30 | Header read timeout |
| `KEEP_ALIVE_TIMEOUT_SECONDS` | Number | No | 75 | Idle connection timeout |
| `SEND_STALL_TIMEOUT_SECONDS` | Number | No | 60 | Send stall timeout |
| `MAX_CONNECTIONS` | Number | No | 10000 | Maximum connections |

### HTTP Variables

| Variable | Type | Required in Prod | Default | Description |
|----------|------|------------------|---------|-------------|
| `BODY_LIMIT_BYTES` | Number | No | 2,000,000 | Request body limit |
| `REQUEST_TIMEOUT_SECONDS` | Number | No | 60 (prod) / 0 (dev) | Request timeout |
| `HTTP_COMPRESSION` | Boolean | No | `true` (prod) / `false` (dev) | Enable compression |
| `HTTP_CSP` | String | No | `default-src 'none'; frame-ancestors 'none'` (prod) | Content Security Policy |
| `CORS_ALLOWED_ORIGINS` | String | No | All (dev) / None (prod) | CORS allowed origins |
| `HSTS_MAX_AGE_SECONDS` | Number | No | 31,536,000 | HSTS max-age |

### Database Variables

| Variable | Type | Required in Prod | Default | Description |
|----------|------|------------------|---------|-------------|
| `DATABASE_URL` | String | **Yes (if database configured)** | Computed | Connection string |
| `DB_MIN_CONNECTIONS` | Number | No | 1 | Min pool connections |
| `DB_MAX_CONNECTIONS` | Number | No | 10 | Max pool connections |
| `DB_CONNECT_TIMEOUT` | Number | No | 5000 | Connection timeout (ms) |
| `DB_IDLE_TIMEOUT` | Number | No | 600000 | Idle timeout (ms) |
| `DB_LOGGING` | Boolean | No | `false` | Log SQL statements |
| `DB_AUTO_MIGRATE` | Boolean | No | `false` (prod) / `true` (dev) | Auto-migrate at boot |
| `DB_ALLOW_INSECURE` | Boolean | No | `false` | Allow non-TLS in prod |

### Logging Variables

| Variable | Type | Required in Prod | Default | Description |
|----------|------|------------------|---------|-------------|
| `LOG_ENABLE` | Boolean | No | `true` | Enable logging |
| `LOG_LEVEL` | String | No | `info` (prod) / `debug` (dev) | Log level |
| `LOG_FORMAT` | String | No | `json` (prod) / `compact` (dev) | Log format |
| `LOG_FILTER` | String | No | Service name + modules | Log filter |
| `RUST_LOG` | String | No | (not set) | Override log configuration |

### Worker Variables

| Variable | Type | Required in Prod | Default | Description |
|----------|------|------------------|---------|-------------|
| `WORKER_MODE` | String | No | `BackgroundQueue` (prod with queue) / `ForegroundBlocking` (dev) | Worker mode |

### Proxy Variables

| Variable | Type | Required in Prod | Default | Description |
|----------|------|------------------|---------|-------------|
| `TRUSTED_PROXIES` | String | No | (empty) | Comma-separated proxy IPs/CIDRs |

---

## Configuration Validation

In production and production-like environments, axa **fails closed** on invalid configuration:

```bash
# These will cause boot failure in production:

# Invalid number
REQUEST_TIMEOUT_SECONDS=abc

# Out of range
PORT=70000

# Unknown flag value
HTTP_COMPRESSION=maybe

# Missing required database URL
# (when service has a database configured)
DATABASE_URL=
```

In development and test, invalid values fall back to defaults with a warning.

---

## Configuration Precedence

axa resolves configuration in this order (highest to lowest):

1. **Environment variables** (e.g., `PORT=8080`)
2. **CLI overrides** (e.g., `cargo run -- --port 8080`)
3. **Service defaults** (from `Defaults` struct)
4. **axa defaults** (from `Defaults::default()`)
5. **Environment-specific defaults** (e.g., compression on in production)

---

## Testing Configuration

Use `Config::from_lookup` to test configuration without touching the process environment:

```rust
use axa::{Environment, config::{Config, Defaults}};

#[test]
fn test_production_config() {
    let environment = Environment::Production;
    
    let config = Config::from_lookup(
        &environment,
        &Defaults::default(),
        |name| match name {
            "DATABASE_URL" => Some("postgres://user:pass@localhost/db".to_string()),
            _ => None,
        },
    ).unwrap();
    
    assert!(config.server.is_production());
    assert!(config.database.url.is_some());
}
```

---

## Configuration Warnings

Warnings are collected and logged by `logger::init`. To check warnings programmatically:

```rust
use axa::config::Logger;

let config = Config::from_env(&environment, &defaults)?;

// Check for warnings
if !config.logger.warnings.is_empty() {
    eprintln!("Configuration warnings:");
    for warning in &config.logger.warnings {
        eprintln!("  - {}", warning);
    }
}
```

---

## Debug Output

Configuration structures implement `Debug` for inspection:

```rust
println!("{:#?}", config);
```

**Note**: Sensitive values (passwords in URLs) are automatically redacted in debug output.

---

## Complete Example

```rust
use axa::{Environment, config::{Config, Defaults}, logger, prelude::*, server};

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Define defaults
    let defaults = Defaults {
        database: "my_service",
        port: 3000,
        production_port: Some(8080),
        body_limit: 10 * 1_000_000, // 10 MB
        max_connections: 20,
        connect_timeout: 10_000,    // 10 seconds
        idle_timeout: 300_000,     // 5 minutes
        auto_migrate: false,
    };
    
    // 2. Load environment and configuration
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &defaults)?;
    
    // 3. Initialize logging (shows warnings)
    logger::init("my_service", &config.logger);
    
    // 4. Check for warnings
    if !config.logger.warnings.is_empty() {
        tracing::warn!(
            warnings = config.logger.warnings.len(),
            "Configuration warnings detected"
        );
    }
    
    // 5. Create context and start server
    let ctx = AppContext::new(environment, (), config);
    let router = AppRoutes::with_default_routes()
        .into_app(ctx)?;
    
    server::serve(router, &config.server).await
}
```

---

## See Also

- [Quick Start](../quickstart/index.md) - Get started in 5 minutes
- [Feature Selection](../features/index.md) - Choose the right features
- [Examples](../examples/index.md) - Real-world patterns
- [Full API Docs](../README.md) - Complete reference
