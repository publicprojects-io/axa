# axa Templates

This directory contains templates for creating new services using axa.

## Available Templates

| Template | Description | Best For |
|----------|-------------|----------|
| [service-template/](service-template/) | Complete service template | New services, production-ready foundation |

## Service Template

The **service-template** provides a complete foundation for building production-ready services with axa. It includes:

### Features

- ✅ axa integration with standard patterns
- ✅ Health checks (liveness, readiness, startup)
- ✅ Configuration management from environment variables
- ✅ Error handling with RFC 9457 Problem Details
- ✅ Structured logging with tracing
- ✅ CI pipeline (GitHub Actions)
- ✅ Pre-commit hooks
- ✅ Dev container for VS Code
- ✅ Podman Compose for local development
- ✅ Documentation structure (architecture, API)

### Quick Start

```bash
# Use the template on GitHub
# 1. Click "Use this template" on GitHub
# 2. Create a new repository from this template
# 3. Clone your new repository

cd your-service

# Rename the project
find . -type f \( -name "*.rs" -o -name "*.toml" -o -name "*.md" \) \
  -exec sed -i 's/service-template/your-service/g' {} +\;

# Build and run
cargo build
cargo run
```

### Customization

The template is designed to be customized:

1. **Add Database**: Choose from axa's database crates:
   - `axa-postgres-sqlx` - PostgreSQL with sqlx
   - `axa-postgres-seaorm` - PostgreSQL with SeaORM
   - `axa-mysql-sqlx` - MySQL with sqlx
   - `axa-mysql-seaorm` - MySQL with SeaORM

2. **Add Authentication**: Choose from axa's auth crates:
   - `axa-paseto-auth` - PASETO tokens
   - `axa-keycloak-auth` - Keycloak/OIDC

3. **Add Features**: Enable axa features as needed:
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

### Template Structure

```
service-template/
├── README.md                      # Project documentation
├── Cargo.toml                     # Rust package configuration
├── .gitignore                     # Git ignore patterns
├── .pre-commit-config.yaml        # Pre-commit hooks
├── .devcontainer/                 # VS Code dev container
│   ├── devcontainer.json
│   └── podman-compose.devcontainer.yml
├── podman-compose.yml             # Development containers
├── .github/
│   └── workflows/
│       └── ci.yml                 # CI pipeline
├── src/
│   ├── main.rs                    # Entry point
│   ├── app.rs                     # Router and state
│   ├── config.rs                  # Configuration
│   ├── error.rs                   # Error handling
│   └── health.rs                  # Health checks
└── docs/
    ├── architecture.md             # Architecture decisions
    └── api.md                      # API documentation
```

## Creating Your Own Template

To create a specialized template:

1. Copy the service-template directory
2. Add your specific features and dependencies
3. Update the documentation
4. Test the template works
5. Submit a pull request to axa

## Conventions

All templates follow axa conventions:

1. **Library, not framework**: `main.rs` stays ordinary Axum
2. **Spec-first**: Document behavior before implementing
3. **Fail-closed**: Production refuses unsafe configurations
4. **Audit-friendly**: Every request path is traceable
5. **Zero indirection**: No macros, no code generation

## Learn More

- [axa Documentation](../docs/README.md)
- [axa Specification](../spec/index.md)
- [axa Examples](../examples/README.md)

## License

All templates are provided under the same license as axa: MIT OR Apache-2.0 OR BSD-3-Clause OR GPL-2.0-only OR GPL-3.0-only
