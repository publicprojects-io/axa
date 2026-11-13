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

Copy the directory out of the axa repository, rename it, and run it:

```sh
cp -R templates/service-template ../your-service
cd ../your-service
git init
cargo run
```

Then replace `service-template` and `service_template` with your service's name, as
[its README](service-template/README.md) describes.

### Customization

Turn on what the service needs as features of `axa` in `Cargo.toml`, such as
`axa = { version = "0.12", features = ["postgres-sqlx", "paseto"] }`:

1. **A database**: `postgres-sqlx`, `postgres-seaorm`, `postgres-diesel`,
   `postgres-toasty`, or the `mysql-` forms of each. `AppContext::db` becomes its pool.
2. **Authentication**: `paseto` for PASETO tokens, or `keycloak` for Keycloak and OIDC.
3. **More**: `queue`, `outbox`, `cursor-pagination`, `prometheus`, `ratelimit`,
   `idempotency`, `versioning`, `tracing`, `opentelemetry`, `redis`, `openapi`, and
   `session`. Each is described in the [axa README](../README.md#installing).

### Template Structure

```
service-template/
├── README.md                      # How to start a service from it
├── Cargo.toml                     # axa, axum, serde, and tokio; strict lints
├── .gitignore
├── .pre-commit-config.yaml        # Pre-commit hooks
├── .devcontainer/                 # VS Code dev container
├── podman-compose.yml             # Local Postgres and Redis
├── .github/workflows/ci.yml       # CI pipeline
├── src/
│   ├── main.rs                    # Settings, logger, and server
│   └── routes.rs                  # The service's routes
└── docs/
    ├── architecture.md            # How it is built, and a decision log
    └── api.md                     # What the service answers
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
5. **No indirection of its own**: health, settings, middleware, and errors come from axa, not from code in the template

## Learn More

- [axa Documentation](../docs/README.md)
- [axa Specification](../spec/index.md)
- [axa Examples](../examples/README.md)

## License

All templates are provided under the same license as axa: MIT OR Apache-2.0 OR BSD-3-Clause OR GPL-2.0-only OR GPL-3.0-only
