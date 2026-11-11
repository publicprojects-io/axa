# Basic Service Example

A minimal example of an Axum service using axa infrastructure.

## Features Demonstrated

- Standard axa startup pattern with `start_app()`
- Health check endpoint (GET /health)
- JSON API endpoints
- Error handling with `axa::error::AppError`
- Environment variable configuration
- Structured logging with tracing

## Quick Start

```bash
# Navigate to the example
cd examples/basic-service

# Run the service
cargo run

# Test the endpoints
curl http://localhost:3000/health
curl http://localhost:3000/
curl http://localhost:3000/greet/World
```

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | /health | Health check - returns service status and version |
| GET | / | Root endpoint - returns welcome message |
| GET | /greet/:name | Greeting endpoint - returns personalized greeting |

## Configuration

| Environment Variable | Default | Description |
|----------------------|---------|-------------|
| PORT | 3000 | Port to listen on |
| RUST_LOG | info | Log level (debug, info, warn, error) |

## Testing

```bash
# Run tests
cargo test

# Run tests with coverage
cargo tarpaulin
```

## Project Structure

```
basic-service/
├── Cargo.toml          # Dependencies and package info
├── src/
│   └── main.rs         # Application entry point
└── README.md           # This file
```

## Next Steps

After understanding this basic example, check out:

- [Todo API Example](../todo-api) - Demonstrates database integration
- [Multi-tenant Example](../multi-tenant) - Demonstrates multi-tenancy patterns
- [axa Documentation](../../docs) - Complete axa documentation
