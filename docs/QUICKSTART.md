# Quick Start Guide

**axa** is a shared library for web services built with Axum. It provides route tables, middleware, health checks, configuration, logging, and more — all while keeping your `main.rs` as ordinary Rust.

This guide gets you from zero to a working axa service in 5 minutes.

---

## Prerequisites

- Rust 1.97 or newer (edition 2024)
- Basic familiarity with Axum

Check your Rust version:
```bash
rustc --version
```

If you need to update:
```bash
rustup update
```

---

## Step 1: Create a New Project

```bash
cargo new hello-axa --edition 2024
cd hello-axa
```

---

## Step 2: Add axa to Your Dependencies

Edit `Cargo.toml`:

```toml
[package]
name = "hello-axa"
version = "0.1.0"
edition = "2024"

[dependencies]
axa = "0.10"
axum = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

For a service with a database, add the `sqlx` feature:
```toml
[dependencies]
axa = { version = "0.10", features = ["sqlx"] }
```

---

## Step 3: Write Your First Handler

Edit `src/main.rs`:

```rust
use axa::{
    Environment, Result,
    config::{Config, Defaults},
    logger,
    prelude::*,
    server,
};

async fn hello() -> Result<Response> {
    format::json(serde_json::json!({
        "message": "Hello from axa!",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Load environment and configuration
    let environment = Environment::from_env();
    let defaults = Defaults {
        database: "", // No database for this example
        ..Defaults::default()
    };
    let config = Config::from_env(&environment, &defaults)?;
    
    // 2. Initialize logging
    logger::init("hello_axa", &config.logger);

    // 3. Create application context
    let ctx = AppContext::new(environment, (), config);

    // 4. Define your routes
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new()
            .prefix("/api")
            .add("/hello", get(hello)))
        .into_app(ctx)?;

    // 5. Start the server
    server::serve(router, &config.server).await
}
```

---

## Step 4: Run Your Service

```bash
cargo run
```

Your service will start on port 1241 (a=1, x=24, a=1).

---

## Step 5: Test It

Open another terminal and test your endpoints:

```bash
# Your custom route
curl http://localhost:1241/api/hello

# Built-in health checks
curl http://localhost:1241/_ping
curl http://localhost:1241/_health
curl http://localhost:1241/_readiness
```

You should see:
```json
{"message":"Hello from axa!","timestamp":"2026-11-09T10:00:00Z"}
```

---

## Step 6: Add Configuration (Optional)

axa reads configuration from environment variables. Try these:

```bash
# Change the port
PORT=8080 cargo run

# Enable compression in development
HTTP_COMPRESSION=true cargo run

# Set a custom binding address
BINDING=0.0.0.0 cargo run
```

See [Configuration](CONFIGURATION.md) for all available options.

---

## Step 7: Add a Database (Optional)

If you added the `sqlx` feature, you can connect to PostgreSQL:

```rust
use axa::db;

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let defaults = Defaults {
        database: "hello_axa", // Service name
        ..Defaults::default()
    };
    let config = Config::from_env(&environment, &defaults)?;
    logger::init("hello_axa", &config.logger);

    // Connect to the database
    let pool = db::connect(&config.database).await?;
    
    // Create context with the pool
    let ctx = AppContext::new(environment, pool, config);

    // ... rest of your code
}
```

In development, axa will automatically connect to `postgres://postgres:postgres@localhost:5432/hello_axa_development`.

---

## Step 8: Add More Routes

```rust
async fn get_user(Path(user_id): Path<u64>) -> Result<Response> {
    format::json(serde_json::json!({
        "user_id": user_id,
        "name": "Alice"
    }))
}

async fn create_user() -> Result<Response> {
    format::json(serde_json::json!({
        "id": 1,
        "name": "New User"
    }))
}

// In your router setup:
let router = AppRoutes::with_default_routes()
    .add_route(Routes::new()
        .prefix("/api")
        .add("/hello", get(hello))
        .add("/users", get(list_users))
        .add("/users/{id}", get(get_user))
        .add("/users", post(create_user)))
    .into_app(ctx)?;
```

---

## Step 9: Use Error Handling

axa provides convenient error helpers:

```rust
use axa::{not_found, bad_request};

async fn get_user(Path(user_id): Path<u64>) -> Result<Response> {
    if user_id == 0 {
        return bad_request("User ID cannot be zero");
    }
    
    if user_id > 100 {
        return not_found();
    }
    
    format::json(serde_json::json!({"id": user_id, "name": "Alice"}))
}
```

All errors automatically return RFC 9457 problem details with appropriate HTTP status codes.

---

## Step 10: Add Middleware

axa applies a standard middleware stack automatically. You can customize it:

```rust
use axa::middleware;
use axum::Router;
use tower_http::compression::CompressionLayer;

let router = AppRoutes::with_default_routes()
    .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
    .into_router(ctx.clone())?;

// Add custom middleware before the standard stack
let router = router.layer(CompressionLayer::new());

// Apply the standard stack last
let app = middleware::apply(router, &ctx.config.http)?;
```

---

## Next Steps

You now have a working axa service! Here's what to explore next:

- [Choosing Features](FEATURES.md) - Which crates to use for your use case
- [Configuration Guide](CONFIGURATION.md) - All environment variables and defaults
- [Examples](EXAMPLES.md) - Real-world patterns and recipes
- [Full Documentation](../README.md) - Complete feature reference

---

## Need Help?

- Check the [specification](../spec/index.md) for exact behavior
- Review the [upgrade guide](../help/upgrading-to-0.3/index.md) if migrating from an older version
- See [From Loco into axa](../help/from-loco-into-axa/index.md) if converting from Loco
