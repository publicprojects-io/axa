# Service Template

A template for creating new services using axa. This provides a solid foundation
for building auditable, production-ready Axum services.

## Quick Start

Click the "Use this template" button on GitHub to create a new repository from this template.

Then:

```bash
# Clone your new repository
git clone https://github.com/your-org/your-service.git
cd your-service

# Update the project name
find . -type f -name "*.rs" -o -name "*.toml" -o -name "*.md" | xargs sed -i 's/service-template/your-service/g'

# Install dependencies
cargo build

# Run the service
cargo run
```

## What's Included

This template provides:

### Core Structure

```
service-template/
├── Cargo.toml              # Main package configuration
├── src/
│   ├── main.rs             # Application entry point
│   ├── app.rs              # Application router and state
│   ├── config.rs           # Configuration management
│   ├── error.rs            # Error handling
│   └── health.rs           # Health check endpoints
├── .github/
│   └── workflows/
│       └── ci.yml          # CI configuration
├── .gitignore              # Git ignore patterns
├── .pre-commit-config.yaml # Pre-commit hooks
├── .devcontainer/          # VS Code dev container
│   ├── devcontainer.json
│   └── Dockerfile
├── podman-compose.yml      # Development database
├── docs/
│   ├── architecture.md     # Architecture decisions
│   └── api.md              # API documentation
└── README.md               # Project documentation
```

### Features

- ✅ axa integration with standard patterns
- ✅ Health checks (liveness, readiness, startup)
- ✅ Structured logging with tracing
- ✅ Configuration from environment variables
- ✅ Error handling with RFC 9457 Problem Details
- ✅ CI pipeline with fmt, clippy, test, doc checks
- ✅ Pre-commit hooks
- ✅ Dev container for VS Code
- ✅ Podman Compose for local development
- ✅ Documentation structure

### Production Ready

- Security headers (HSTS, CSP, etc.)
- Request timeouts
- Connection limits
- Graceful shutdown
- Structured logging
- Metrics (optional)

## Usage

### Development

```bash
# Start database containers
podman-compose -f podman-compose.yml up -d

# Run the service
cargo run

# Run tests
cargo test

# Run all checks
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps
```

### Production

```bash
# Build for production
cargo build --release

# Run with production configuration
RUST_LOG=info ./target/release/your-service
```

## Customization

### Add Database

To add a database:

1. Choose a database crate from axa:
   - `axa-postgres-sqlx` - PostgreSQL with sqlx
   - `axa-postgres-seaorm` - PostgreSQL with SeaORM
   - `axa-mysql-sqlx` - MySQL with sqlx
   - `axa-mysql-seaorm` - MySQL with SeaORM

2. Add to Cargo.toml:
   ```toml
   [dependencies]
   axa = { version = "0.12", features = ["db"] }
   axa-postgres-sqlx = "0.12"
   ```

3. Update configuration in `src/config.rs`

4. Add migrations in a `migrations/` directory

### Add Authentication

To add authentication:

1. Choose an auth crate:
   - `axa-paseto-auth` - PASETO tokens
   - `axa-keycloak-auth` - Keycloak/OIDC

2. Add to Cargo.toml:
   ```toml
   [dependencies]
   axa = { version = "0.12", features = ["auth"] }
   axa-paseto-auth = "0.12"
   ```

3. Add auth middleware in `src/app.rs`

### Add Features

Available axa features:

- `db` - Database integration
- `auth` - Authentication helpers
- `queue` - Job queue
- `outbox` - Transactional outbox
- `cursor-pagination` - Signed cursor pagination
- `prometheus` - Prometheus metrics
- `ratelimit` - Request rate limiting
- `idempotency` - Idempotency keys
- `versioning` - API versioning
- `tracing` - Distributed tracing
- `opentelemetry` - OpenTelemetry integration
- `redis` - Redis cache
- `openapi` - OpenAPI documentation
- `session` - Web sessions

## Conventions

This template follows axa conventions:

1. **Library, not framework**: Your `main.rs` stays ordinary Axum
2. **Spec-first**: Document behavior before implementing
3. **Fail-closed**: Production refuses unsafe configurations
4. **Audit-friendly**: Every request path is traceable
5. **Zero indirection**: No macros, no code generation

## Learn More

- [axa Documentation](https://github.com/publicprojects-io/axa/tree/main/docs)
- [axa Specification](https://github.com/publicprojects-io/axa/blob/main/spec/index.md)
- [axa Crates](https://github.com/publicprojects-io/axa#crates)

## License

This template is provided under the same license as axa: MIT OR Apache-2.0 OR BSD-3-Clause OR GPL-2.0-only OR GPL-3.0-only
