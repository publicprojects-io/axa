# Architecture

This document describes the architecture decisions for this service.

## Overview

This service follows the **axa** philosophy:

1. **Library, not framework**: We use axa as a library, not a framework. Our `main.rs` stays ordinary Axum code.
2. **Spec-first**: Every behavior change requires a spec edit, code edit, and test edit.
3. **Fail-closed**: Production refuses unsafe configurations by default.
4. **Audit-friendly**: Every request path must be traceable from `main.rs`.

## Technology Stack

| Component | Technology | Purpose |
|-----------|------------|---------|
| Web Framework | Axum | Async HTTP server |
| Infrastructure | axa | Production-ready helpers |
| Configuration | Environment Variables | 12-factor app |
| Logging | tracing + tracing-subscriber | Structured logging |
| Serialization | serde + serde_json | JSON handling |

## Directory Structure

```
.
├── src/
│   ├── main.rs          # Entry point, server setup
│   ├── app.rs           # Router and application state
│   ├── config.rs        # Configuration management
│   ├── error.rs         # Error handling
│   └── health.rs        # Health check endpoints
├── tests/
│   └── integration.rs   # Integration tests
├── docs/
│   ├── architecture.md  # Architecture decisions
│   └── api.md           # API documentation
├── .github/
│   └── workflows/
│       └── ci.yml       # CI configuration
├── .devcontainer/       # VS Code dev container
├── podman-compose.yml   # Development containers
└── README.md            # Project documentation
```

## Key Principles

### 1. Keep main.rs Ordinary

Our `main.rs` should look like any Axum application. We don't use:
- Route macros
- Lifecycle hooks
- Code generation
- Dependency injection containers

This keeps the code audit-friendly and easy to understand.

### 2. Fail Closed

In production, we refuse unsafe configurations:
- No database URL? Don't start.
- Wildcard CORS? Don't start.
- Insecure settings? Don't start.

This is enforced at startup, not at runtime.

### 3. Every Path is Traceable

From `main.rs`, you should be able to trace every possible request path:
- No hidden middleware
- No dynamic route registration
- No reflection

### 4. Request-Scoped State

We use Axum's extractor pattern for request-scoped state:
- Database connections from the connection pool
- Configuration from AppState
- Request context from extensions

## Error Handling

We follow RFC 9457 (Problem Details) for error responses:

```json
{
  "type": "https://example.com/errors/validation",
  "title": "Validation Error",
  "status": 400,
  "detail": "The request was invalid",
  "instance": "/api/users"
}
```

### Error Classification

| Type | Status Code | When to Use |
|------|-------------|-------------|
| Validation | 400 | Client sent invalid data |
| Authentication | 401 | Client not authenticated |
| Authorization | 403 | Client authenticated but not authorized |
| Not Found | 404 | Resource doesn't exist |
| Conflict | 409 | Resource state conflict |
| Rate Limit | 429 | Too many requests |
| Internal | 500 | Server error (should be rare) |
| Service Unavailable | 503 | Service not ready |

## Health Checks

All services must implement three health endpoints:

| Endpoint | Purpose | Should Return |
|----------|---------|---------------|
| `/_health/live` | Process is alive | 200 always |
| `/_health/ready` | Ready to accept traffic | 200 or 503 |
| `/_health/startup` | Startup completed | 200 or 503 |

## Logging

We use structured logging with `tracing`:

```rust
use tracing::{info, error, debug, warn};

info!(user_id = 123, action = "login", "User logged in");
error!(error = %err, "Failed to connect to database");
```

In production, we use JSON format for easier parsing.

## Security

### Security Headers

All responses include security headers:
- `Strict-Transport-Security` (HSTS)
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY`
- `Content-Security-Policy` (CSP)
- `Referrer-Policy: strict-origin-when-cross-origin`

### Rate Limiting

Rate limiting is implemented at the edge (load balancer, API gateway).
For per-endpoint limits, use `axa::ratelimit`.

### Authentication

For authenticated endpoints:
1. Use JWT tokens (PASETO or Keycloak)
2. Validate tokens in middleware
3. Extract claims into request extensions

## Performance

### Connection Limits

Use `axa::server` for connection limits:
- Max connections per IP
- Request timeouts
- Graceful shutdown

### Caching

For caching:
- Use `axa::redis` for distributed cache
- Use in-memory cache for local caching
- Always set appropriate Cache-Control headers

## Testing

### Unit Tests

Unit tests verify individual functions and modules.

### Integration Tests

Integration tests verify the full stack:
1. Start the application
2. Send real HTTP requests
3. Verify responses

Use `axa-test` for request-level testing.

## Deployment

### Container Image

Build a minimal container image:

```dockerfile
FROM rust:slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:stable-slim
COPY --from=builder /app/target/release/service-template /usr/local/bin/service
COPY README.md /usr/local/share/doc/service/
USER nonroot
CMD ["service"]
```

### Kubernetes

For Kubernetes deployments:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: service-template
spec:
  replicas: 3
  selector:
    matchLabels:
      app: service-template
  template:
    metadata:
      labels:
        app: service-template
    spec:
      containers:
      - name: service
        image: your-registry/service-template:v1.0.0
        ports:
        - containerPort: 3000
        livenessProbe:
          httpGet:
            path: /_health/live
            port: 3000
          initialDelaySeconds: 5
          periodSeconds: 10
        readinessProbe:
          httpGet:
            path: /_health/ready
            port: 3000
          initialDelaySeconds: 5
          periodSeconds: 10
        resources:
          limits:
            cpu: 1000m
            memory: 512Mi
          requests:
            cpu: 100m
            memory: 128Mi
```

## Monitoring

### Metrics

Use `axa::prometheus` for metrics:
- Request counts by route
- Request durations
- Error rates
- Database query times

### Alerting

Set up alerts for:
- Error rate > 1%
- Response time > 1s (p99)
- Memory usage > 80%
- CPU usage > 80%

## Decision Log

| Date | Decision | Context |
|------|----------|---------|
| 2026-11-XX | Use axa | Chose axa for production-ready infrastructure |
| 2026-11-XX | Use Axum | Chose Axum for async HTTP |
| 2026-11-XX | Use Podman | Chose Podman for containerization |

## Future Considerations

- [ ] Add database (PostgreSQL, MySQL, SQLite)
- [ ] Add authentication (PASETO, Keycloak)
- [ ] Add metrics (Prometheus)
- [ ] Add distributed tracing (OpenTelemetry)
- [ ] Add rate limiting
- [ ] Add job queue
- [ ] Add caching (Redis)
