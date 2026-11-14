# Examples and Recipes

This document contains real-world patterns and recipes for common tasks with axa.

---

## Table of Contents

1. [Basic Patterns](#basic-patterns)
2. [Configuration Patterns](#configuration-patterns)
3. [Database Patterns](#database-patterns)
4. [Authentication Patterns](#authentication-patterns)
5. [Error Handling Patterns](#error-handling-patterns)
6. [Middleware Patterns](#middleware-patterns)
7. [Testing Patterns](#testing-patterns)
8. [Production Patterns](#production-patterns)

---

## Basic Patterns

### Minimal Service

The absolute minimum axa service:

```rust
use axa::{Environment, config::{Config, Defaults}, logger, prelude::*, server};

async fn hello() -> Result<Response> {
    format::json(serde_json::json!({"hello": "world"}))
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    logger::init("my_service", &config.logger);
    
    let ctx = AppContext::new(environment, (), config);
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
        .into_app(ctx)?;
    
    server::serve(router, &config.server).await
}
```

---

### Service with Database

```rust
use axa::{Environment, config::{Config, Defaults}, db, logger, prelude::*, server};

async fn get_users() -> Result<Response> {
    // Your handler logic here
    format::json(serde_json::json!([]))
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let defaults = Defaults {
        database: "my_service", // Service name for database
        ..Defaults::default()
    };
    let config = Config::from_env(&environment, &defaults)?;
    logger::init("my_service", &config.logger);
    
    // Connect to database
    let pool = db::connect(&config.database).await?;
    let ctx = AppContext::new(environment, pool, config);
    
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/users", get(get_users)))
        .into_app(ctx)?;
    
    server::serve(router, &config.server).await
}
```

**Database URL in development**: `postgres://postgres:postgres@localhost:5432/my_service_development`

---

### Service with Custom State

```rust
use axa::{Environment, config::{Config, Defaults}, logger, prelude::*, server};
use axum::extract::State;

#[derive(Clone)]
struct AppState {
    app_name: String,
    version: String,
}

async fn info(State(state): State<AppState>) -> Result<Response> {
    format::json(serde_json::json!({
        "app": state.app_name,
        "version": state.version
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    logger::init("my_service", &config.logger);
    
    let app_state = AppState {
        app_name: "My Service".to_string(),
        version: "1.0.0".to_string(),
    };
    
    // Pass custom state as third argument
    let ctx = AppContext::new(environment, app_state, config);
    
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/info", get(info)))
        .into_app(ctx)?;
    
    server::serve(router, &config.server).await
}
```

---

## Configuration Patterns

### Custom Defaults

```rust
use axa::config::{Defaults, Config};
use axa::Environment;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let defaults = Defaults {
        database: "my_app",
        port: 3000,                    // Development port
        production_port: Some(8080), // Production port
        body_limit: 10 * 1_000_000,    // 10 MB
        max_connections: 50,          // Pool size
        connect_timeout: 10_000,      // 10 seconds
        idle_timeout: 300_000,        // 5 minutes
        auto_migrate: false,          // Don't auto-migrate in prod
    };
    
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &defaults)?;
    
    Ok(())
}
```

---

### Environment-Specific Configuration

```rust
use axa::{Environment, config::{Config, Defaults}};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let environment = Environment::from_env();
    
    let defaults = if environment.is_production() {
        Defaults {
            database: "my_app",
            port: 8080,
            production_port: Some(8080),
            auto_migrate: false,
            ..Defaults::default()
        }
    } else {
        Defaults {
            database: "my_app",
            port: 3000,
            auto_migrate: true,
            ..Defaults::default()
        }
    };
    
    let config = Config::from_env(&environment, &defaults)?;
    Ok(())
}
```

---

### Testing Configuration

```rust
use axa::{Environment, config::{Config, Defaults}};

#[test]
fn test_config_parsing() {
    let environment = Environment::Development;
    
    let config = Config::from_lookup(
        &environment,
        &Defaults::default(),
        |name| match name {
            "PORT" => Some("8080".to_string()),
            "DATABASE_URL" => Some("postgres://localhost/test".to_string()),
            _ => None,
        },
    ).unwrap();
    
    assert_eq!(config.server.port, 8080);
}
```

---

## Database Patterns

### Using sqlx

```rust
use axa::{Environment, config::{Config, Defaults}, db, logger, prelude::*, server};
use sqlx::FromRow;

#[derive(Debug, FromRow)]
struct User {
    id: i64,
    name: String,
    email: String,
}

async fn get_users(axa::extract::Db(pool): axa::extract::Db<sqlx::PgPool>) -> Result<Response> {
    let users = sqlx::query_as::<_, User>("SELECT id, name, email FROM users")
        .fetch_all(&pool)
        .await?;
    
    format::json(users)
}

async fn create_user(
    axa::extract::Db(pool): axa::extract::Db<sqlx::PgPool>,
    axa::extract::Json(body): axa::extract::Json<serde_json::Value>,
) -> Result<Response> {
    let user = sqlx::query("INSERT INTO users (name, email) VALUES ($1, $2) RETURNING id, name, email")
        .bind(body["name"].as_str().unwrap())
        .bind(body["email"].as_str().unwrap())
        .map(|row: sqlx::postgres::PgRow| User {
            id: row.get("id"),
            name: row.get("name"),
            email: row.get("email"),
        })
        .fetch_one(&pool)
        .await?;
    
    format::json(user)
}
```

---

### Database Transactions

```rust
use axa::{Environment, config::{Config, Defaults}, db, logger, prelude::*, server};
use sqlx::{PgPool, Postgres, Transaction};

async fn transfer_funds(
    axa::extract::Db(pool): axa::extract::Db<PgPool>,
    axa::extract::Json(body): axa::extract::Json<serde_json::Value>,
) -> Result<Response> {
    let mut tx = pool.begin().await?;
    
    // Decrement source account
    sqlx::query("UPDATE accounts SET balance = balance - $1 WHERE id = $2")
        .bind(body["amount"].as_f64().unwrap())
        .bind(body["from_account"].as_i64().unwrap())
        .execute(&mut *tx)
        .await?;
    
    // Increment destination account
    sqlx::query("UPDATE accounts SET balance = balance + $1 WHERE id = $2")
        .bind(body["amount"].as_f64().unwrap())
        .bind(body["to_account"].as_i64().unwrap())
        .execute(&mut *tx)
        .await?;
    
    // Commit transaction
    tx.commit().await?;
    
    format::json(serde_json::json!({"status": "success"}))
}
```

---

### Using SeaORM

```rust
// In your Cargo.toml:
// [dependencies]
// axa = { version = "0.13.0", features = ["postgres-seaorm"] }
// sea-orm = { version = "2.0", features = ["postgres", "runtime-tokio-rustls"] }

use axa::{Environment, config::{Config, Defaults}, logger, prelude::*, server};
use sea_orm::{Database, DbErr, EntityTrait, ActiveModelTrait};

// Define your entity
#[derive(DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model;

#[derive(Copy, Clone, Debug, EntityTrait)]
pub struct Entity;

async fn get_all_users(ctx: AppContext) -> Result<Response> {
    let db = ctx.db(); // Get SeaORM connection from context
    
    let users = Entity::find()
        .all(db)
        .await
        .map_err(|e| axa::Error::Internal(e.to_string()))?;
    
    format::json(users)
}
```

---

## Authentication Patterns

### Basic Route Protection

```rust
use axa::{Environment, config::{Config, Defaults}, logger, prelude::*, server};

async fn public_hello() -> Result<Response> {
    format::json(serde_json::json!({"message": "Hello, world!"}))
}

async fn private_hello() -> Result<Response> {
    format::json(serde_json::json!({"message": "Hello, authenticated user!"}))
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    logger::init("my_service", &config.logger);
    
    let ctx = AppContext::new(environment, (), config);
    
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new()
            .prefix("/api")
            .add("/public", get(public_hello))
            .add("/private", get(private_hello)))
        .into_app(ctx)?;
    
    server::serve(router, &config.server).await
}
```

---

### Using PASETO Authentication

```rust
// In your Cargo.toml:
// [dependencies]
// axa = { version = "0.13.0", features = ["paseto"] }

use axa::{Environment, config::{Config, Defaults}, logger, prelude::*, server, paseto};
use axa::paseto::{Guard, Policy};

async fn protected_route() -> Result<Response> {
    format::json(serde_json::json!({"message": "You have access!"}))
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    logger::init("my_service", &config.logger);
    
    let ctx = AppContext::new(environment, (), config.clone());
    
    // Create a guard with your policy
    let guard = Guard::new(
        // Your PASETO public key
        std::env::var("PASETO_PUBLIC_KEY").unwrap(),
        // Your policy
        Policy {
            // Which paths are public
            public_prefixes: vec!["/api/public".to_string()],
            // Default policy: read-only
            default: paseto::PolicyDecision::ReadOnly,
            // Turn enforcement on (default)
            enforcement: paseto::Enforcement::On,
        },
    );
    
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new()
            .prefix("/api")
            .add("/public", get(public_hello))
            .add("/protected", get(protected_route)))
        .into_app(ctx)?;
    
    // Apply the guard
    let app = router.layer(paseto::middleware(guard));
    
    server::serve(app, &config.server).await
}
```

---

### Extracting Auth User

```rust
use axa::paseto::AuthUser;

async fn get_current_user(user: AuthUser) -> Result<Response> {
    format::json(serde_json::json!({
        "user_id": user.claims.sub,
        "email": user.claims.email,
        "roles": user.claims.roles
    }))
}
```

---

### ABAC Policy Example

```rust
use axa::paseto::{Guard, Policy, Action};

// Define your policy
let policy = Policy {
    public_prefixes: vec![],
    default: PolicyDecision::DenyAll,
    enforcement: Enforcement::On,
    // Rule: users with "admin" role can do anything
    rules: vec![
        PolicyRule {
            description: "Admins can do anything".to_string(),
            condition: Condition::Role("admin".to_string()),
            decisions: Decisions {
                create: true,
                read: true,
                update: true,
                delete: true,
                ..Default::default()
            },
        },
        // Rule: users can read their own data
        PolicyRule {
            description: "Users can read their own data".to_string(),
            condition: Condition::SubjectEqualsResource,
            decisions: Decisions {
                read: true,
                ..Default::default()
            },
        },
    ],
};

let guard = Guard::new(public_key, policy);
```

---

## Error Handling Patterns

### Custom Error Type

```rust
use axa::{Error, Result};
use axum::http::StatusCode;

#[derive(Debug)]
pub enum AppError {
    NotFound(String),
    Validation(String),
    Database(sqlx::Error),
}

impl From<AppError> for Error {
    fn from(err: AppError) -> Self {
        match err {
            AppError::NotFound(msg) => Error::NotFound(msg),
            AppError::Validation(msg) => Error::BadRequest(msg),
            AppError::Database(_) => Error::Internal("Database error".to_string()),
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        AppError::Database(err)
    }
}

async fn get_user(Path(id): Path<i64>) -> Result<Response> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&pool)
        .await?;
    
    match user {
        Some(user) => format::json(user),
        None => Err(AppError::NotFound(format!("User {} not found", id))),
    }
}
```

---

### Using axa's Error Helpers

```rust
use axa::{not_found, bad_request, Error};

async fn create_user(
    axa::extract::Json(body): axa::extract::Json<serde_json::Value>,
) -> Result<Response> {
    // Validate input
    if !body.get("name").is_some() {
        return bad_request("Name is required");
    }
    
    if !body.get("email").is_some() {
        return bad_request("Email is required");
    }
    
    // Check if user exists
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM users WHERE email = $1"
    )
    .bind(body["email"].as_str().unwrap())
    .fetch_one(&pool)
    .await?;
    
    if existing > 0 {
        return Err(Error::CustomError(
            StatusCode::CONFLICT,
            "Email already exists"
        ));
    }
    
    // Create user
    // ...
    
    format::json(serde_json::json!({"status": "created"}))
}
```

---

### Custom Error with Details

```rust
use axa::{Error, ErrorDetail};

async fn validate_input(input: String) -> Result<(), Error> {
    if input.is_empty() {
        return Err(Error::CustomError(
            axum::http::StatusCode::BAD_REQUEST,
            ErrorDetail::new(
                "validation_failed",
                "Input cannot be empty"
            )
        ));
    }
    
    if input.len() > 100 {
        return Err(Error::CustomError(
            axum::http::StatusCode::BAD_REQUEST,
            ErrorDetail::new(
                "validation_failed",
                "Input is too long"
            )
        ));
    }
    
    Ok(())
}
```

---

## Middleware Patterns

### Custom Middleware

```rust
use axa::prelude::*;
use axum::{
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

pub async fn timing_middleware<B>(request: Request<B>, next: Next<B>) -> Result<Response> {
    let start = std::time::Instant::now();
    
    let response = next.run(request).await;
    
    let duration = start.elapsed();
    tracing::info!(
        method = %request.method(),
        uri = %request.uri(),
        duration = ?duration,
        status = %response.status(),
        "Request completed"
    );
    
    Ok(response)
}

// Usage:
let router = Router::new()
    .route("/api/hello", get(hello))
    .layer(from_fn(timing_middleware));
```

---

### Conditional Middleware

```rust
use axa::{Environment, config::{Config, Defaults}, logger, prelude::*, server, middleware};

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    logger::init("my_service", &config.logger);
    
    let ctx = AppContext::new(environment.clone(), (), config);
    
    let mut router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/hello", get(hello)));
    
    // Only apply in development
    if !environment.is_production() {
        router = router.layer(from_fn(timing_middleware));
    }
    
    let app = router.into_app(ctx)?;
    server::serve(app, &config.server).await
}
```

---

### Combining Middleware

```rust
use axa::{middleware, prelude::*};
use axum::Router;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, cors::CorsLayer};

let router = Router::new()
    .route("/api/hello", get(hello));

// Build middleware stack
let stack = ServiceBuilder::new()
    .layer(CompressionLayer::new())
    .layer(CorsLayer::new())
    .layer(from_fn(timing_middleware));

let app = router.layer(stack);

// Apply axa's standard middleware last
let app = middleware::apply(app, &config.http)?;
```

---

## Testing Patterns

### Basic Test with axa-test

```toml
# In your Cargo.toml
[dev-dependencies]
axa-test = "0.1"
```

```rust
use axa_test::prelude::*;
use axa::prelude::*;

#[tokio::test]
async fn test_hello_endpoint() {
    request::<App, _, _>(|server, _ctx| async move {
        let response = server.get("/api/hello").await;
        assert_eq!(response.status_code(), 200);
        
        let body: serde_json::Value = response.json().await;
        assert_eq!(body["message"], "Hello from axa!");
    }).await;
}
```

---

### Testing with State

```rust
#[derive(Clone, Default)]
struct AppState {
    counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

async fn increment(State(state): State<AppState>) -> Result<Response> {
    state.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format::json(serde_json::json!({"count": state.counter.load(std::sync::atomic::Ordering::SeqCst)}))
}

#[tokio::test]
async fn test_counter() {
    request::<App, _, _>(|server, ctx| async move {
        let state = ctx.state::<AppState>().unwrap();
        
        // Initial count should be 0
        let response = server.get("/api/counter").await;
        let body: serde_json::Value = response.json().await;
        assert_eq!(body["count"], 0);
        
        // After increment
        let response = server.post("/api/counter/increment").await;
        let body: serde_json::Value = response.json().await;
        assert_eq!(body["count"], 1);
    }).await;
}
```

---

### Testing Error Responses

```rust
#[tokio::test]
async fn test_not_found() {
    request::<App, _, _>(|server, _ctx| async move {
        let response = server.get("/api/nonexistent").await;
        assert_eq!(response.status_code(), 404);
        
        // Check it's a problem details response
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/problem+json"
        );
        
        let body: serde_json::Value = response.json().await;
        assert_eq!(body["type"], "about:blank");
        assert_eq!(body["status"], 404);
    }).await;
}
```

---

### Integration Test with Database

```rust
#[tokio::test]
async fn test_user_creation() {
    request::<App, _, _>(|server, ctx| async move {
        // Get the database pool from context
        let pool = ctx.db::<sqlx::PgPool>().unwrap();
        
        // Clean up before test
        sqlx::query("DELETE FROM users").execute(pool).await.unwrap();
        
        // Create user
        let response = server
            .post("/api/users")
            .json(&serde_json::json!({
                "name": "Test User",
                "email": "test@example.com"
            }))
            .await;
        
        assert_eq!(response.status_code(), 201);
        
        // Verify in database
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(pool)
            .await
            .unwrap();
        
        assert_eq!(count, 1);
    }).await;
}
```

---

## Production Patterns

### Graceful Shutdown

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    logger::init("my_service", &config.logger);
    
    let ctx = AppContext::new(environment, (), config);
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
        .into_app(ctx)?;
    
    // server::serve handles graceful shutdown automatically
    // It listens for SIGTERM and SIGINT
    // Requests in flight are allowed to complete
    server::serve(router, &config.server).await?;
    
    // Cleanup code here runs after shutdown
    tracing::info!("Server shut down gracefully");
    
    Ok(())
}
```

---

### Health Checks

```rust
use axa::health::{Checks, HealthCheck};

async fn database_check(pool: sqlx::PgPool) -> Result<(), String> {
    sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&pool)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn redis_check(client: redis::Client) -> Result<(), String> {
    let mut con = client.get_async_connection().await
        .map_err(|e| e.to_string())?;
    
    redis::cmd::PING::new()
        .query_async(&mut con)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    logger::init("my_service", &config.logger);
    
    let pool = db::connect(&config.database).await?;
    let redis_client = redis::Client::open("redis://localhost").unwrap();
    
    // Add custom health checks
    let checks = Checks::new()
        .add("database", || database_check(pool.clone()))
        .add("redis", || redis_check(redis_client.clone()));
    
    let ctx = AppContext::with_health_checks(environment, pool, config, checks);
    
    let router = AppRoutes::with_default_routes()
        .add_route(Routes::new().prefix("/api").add("/hello", get(hello)))
        .into_app(ctx)?;
    
    server::serve(router, &config.server).await
}
```

The `/_readiness` endpoint will now check database and Redis connectivity.

---

### Configuration Validation

```rust
fn validate_config(config: &Config) -> Result<(), String> {
    if config.server.port < 1024 {
        return Err("Port must be >= 1024".to_string());
    }
    
    if config.server.shutdown_timeout_seconds > 300 {
        return Err("Shutdown timeout must be <= 300 seconds".to_string());
    }
    
    if config.http.body_limit_bytes > 100 * 1_000_000 {
        return Err("Body limit must be <= 100 MB".to_string());
    }
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = Config::from_env(&environment, &Defaults::default())?;
    
    // Validate configuration
    if let Err(e) = validate_config(&config) {
        eprintln!("Configuration error: {}", e);
        std::process::exit(1);
    }
    
    logger::init("my_service", &config.logger);
    // ...
}
```

---

### Request ID Tracking

```rust
use axa::client::ClientAddr;

async fn tracked_handler(
    extensions: Extract<axum::http::Extensions>,
    client_addr: ClientAddr,
) -> Result<Response> {
    // Get request ID from extensions
    let request_id = extensions.get::<String>()
        .map(|s| s.as_str())
        .unwrap_or("unknown");
    
    tracing::info!(
        request_id = %request_id,
        client_ip = %client_addr.0,
        "Handling request"
    );
    
    // The request ID is automatically included in all log lines
    // and returned in the x-request-id header
    
    format::json(serde_json::json!({
        "request_id": request_id,
        "client_ip": client_addr.0.to_string()
    }))
}
```

---

## Common Patterns Summary

| Pattern | Use Case | Files |
|---------|----------|-------|
| Minimal Service | Simple API | `main.rs` |
| With Database | Data storage | `main.rs`, `db.rs` |
| Custom State | Shared data | `main.rs`, `state.rs` |
| Authentication | Secure endpoints | `main.rs`, `auth.rs` |
| Error Handling | Consistent errors | `error.rs` |
| Middleware | Cross-cutting concerns | `middleware.rs` |
| Testing | Verify behavior | `tests/*` |
| Production | Reliable deployment | `main.rs`, `config.rs` |

---

## See Also

- [Quick Start](../quickstart/index.md) - Get started in 5 minutes
- [Feature Selection](../features/index.md) - Choose the right features
- [Configuration](../configuration/index.md) - All configuration options
