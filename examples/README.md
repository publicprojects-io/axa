# axa Examples

This directory contains real-world examples demonstrating how to use axa in different scenarios.

## Available Examples

| Example | Description | Complexity | Features Used |
|---------|-------------|------------|---------------|
| [basic-service](./basic-service) | Minimal axa service | ⭐ | startup, health check, JSON API |
| [todo-api](./todo-api) | RESTful Todo API with database | ⭐⭐ | db, sqlx, migrations, validation |
| [multi-tenant](./multi-tenant) | Multi-tenant service | ⭐⭐⭐ | db, multi-tenancy, tenant isolation |

## Learning Path

### 1. Start Here: Basic Service

Begin with the **basic-service** example to understand the fundamental pattern:

```bash
cd examples/basic-service
cargo run
```

This demonstrates:
- How to start an axa service
- Health check endpoint
- JSON responses
- Error handling

### 2. Add a Database: Todo API

Once you understand the basics, move to **todo-api** to see database integration:

```bash
cd examples/todo-api
# Make sure PostgreSQL is running (see podman-compose.yml in project root)
DATABASE_URL=postgres://axa:axa@localhost:5432/todo_api cargo run
```

This adds:
- SQLx for database access
- Migrations
- CRUD operations
- Request validation

### 3. Scale Up: Multi-tenant

Finally, the **multi-tenant** example shows how to handle multiple tenants:

```bash
cd examples/multi-tenant
DATABASE_URL=postgres://axa:axa@localhost:5432/multi_tenant cargo run
```

This demonstrates:
- Schema-per-tenant isolation
- Tenant context propagation
- Automatic schema provisioning
- Tenant-specific data access

## Running with podman-compose

All examples can use the project's podman-compose setup:

```bash
# From project root, start PostgreSQL and Redis
podman-compose -f podman-compose.yml up -d

# Create databases for examples
podman exec -it axa-postgres-1 psql -U axa -c "CREATE DATABASE todo_api;"
podman exec -it axa-postgres-1 psql -U axa -c "CREATE DATABASE multi_tenant;"

# Now run any example
cd examples/todo-api
DATABASE_URL=postgres://axa:axa@localhost:5432/todo_api cargo run
```

## Example Structure

Each example follows the same structure:

```
example-name/
├── Cargo.toml          # Dependencies specific to this example
├── src/
│   └── main.rs         # Application code
├── migrations/         # Database migrations (if applicable)
│   └── *.sql           # Migration files
├── .env.example        # Example environment configuration
└── README.md           # Example-specific documentation
```

## Using Examples as Templates

Each example can serve as a template for your own service:

1. **Copy the example**:
   ```bash
   cp -r examples/basic-service my-service
   cd my-service
   ```

2. **Rename and customize**:
   ```bash
   # Edit Cargo.toml
   sed -i 's/basic-service/my-service/g' Cargo.toml
   sed -i 's/name = "basic-service"/name = "my-service"/g' Cargo.toml
   ```

3. **Add your features**:
   ```bash
   # Add axa features as needed
   cargo add axa --features db,auth,queue
   ```

4. **Update the dependency path**:
   ```toml
   # Change from path dependency to crates.io
   axa = { version = "0.12" }
   ```

## Common Patterns

### Starting a Service

All axa services start the same way:

```rust
use axa::start_app;
use axum::Router;

fn app() -> Router {
    Router::new()
        .route("/health", get(health_check))
        // Add your routes here
}

#[tokio::main]
async fn main() {
    let addr = ([0, 0, 0, 0], 3000).into();
    start_app(app(), addr).await.unwrap();
}
```

### Database Integration

Use axa's database helpers:

```rust
use axa::db::{DbPool, init_db_pool};

#[derive(Clone)]
struct AppState {
    pool: DbPool,
}

#[tokio::main]
async fn main() {
    let pool = init_db_pool().await.expect("Failed to connect to database");
    let state = AppState { pool };
    
    // Pass state to your router
    let app = Router::new()
        .route("/todos", get(list_todos))
        .with_state(state);
    
    start_app(app, addr).await.unwrap();
}
```

### Error Handling

Use axa's error type for consistent error responses:

```rust
use axa::error::AppError;

async fn get_todo(/* ... */) -> Result<Json<TodoResponse>, AppError> {
    // Return 404 if not found
    if todo.is_none() {
        return Err(AppError::not_found("Todo not found"));
    }
    
    // Return 400 for bad requests
    if title.is_empty() {
        return Err(AppError::bad_request("Title cannot be empty"));
    }
    
    // Return 500 for internal errors
    let result = database_query().await.map_err(|e| {
        AppError::internal(format!("Database error: {}", e))
    })?;
    
    Ok(Json(result))
}
```

## Contributing Examples

Have an example to share? Contributions are welcome!

1. Create a new directory in `examples/`
2. Add a `Cargo.toml`, `src/main.rs`, and `README.md`
3. Include clear instructions and use cases
4. Open a pull request

## Need Help?

- Check the [axa Documentation](../docs) for detailed guides
- See [CONTRIBUTING.md](../CONTRIBUTING.md) for development setup
- Join the community (link coming soon!)
