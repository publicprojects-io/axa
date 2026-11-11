//! Multi-tenant Service Example
//!
//! This example demonstrates multi-tenancy patterns using axa infrastructure.
//! It shows:
//! - Tenant isolation at the database level (schema-per-tenant)
//! - Tenant identification from JWT or header
//! - Tenant-specific request context
//! - Schema management for tenants
//!
//! Run with: cargo run
//! Requires: PostgreSQL database

use axa::{
    db::{DbPool, init_db_pool},
    error::AppError,
    start_app,
    web::AppState,
};
use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Tenant context extracted from request
#[derive(Debug, Clone)]
struct TenantContext {
    tenant_id: String,
    schema_name: String,
}

/// Application state
#[derive(Clone)]
struct AppState {
    pool: DbPool,
}

/// Todo item for a specific tenant
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct TenantTodo {
    id: Uuid,
    title: String,
    description: Option<String>,
    completed: bool,
    tenant_id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Create Todo request
#[derive(Debug, Deserialize, Serialize)]
struct CreateTodoRequest {
    title: String,
    description: Option<String>,
}

/// Update Todo request
#[derive(Debug, Deserialize, Serialize)]
struct UpdateTodoRequest {
    title: Option<String>,
    description: Option<String>,
    completed: Option<bool>,
}

/// Todo response
#[derive(Debug, Serialize)]
struct TodoResponse {
    id: Uuid,
    title: String,
    description: Option<String>,
    completed: bool,
    tenant_id: String,
    created_at: String,
    updated_at: String,
}

/// List response
#[derive(Debug, Serialize)]
struct TodoListResponse {
    todos: Vec<TodoResponse>,
    total: usize,
    tenant_id: String,
}

/// Middleware to extract tenant from header
/// 
/// Expects: X-Tenant-ID header containing the tenant identifier
/// Creates: Tenant-specific schema name and sets up context
async fn tenant_middleware<B>(
    request: Request<B>,
    next: Next<B>,
) -> Result<Response, StatusCode> {
    let tenant_id = request
        .headers()
        .get("X-Tenant-ID")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Validate tenant ID (simple validation for demo)
    if tenant_id.is_empty() || tenant_id.len() > 64 {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Create safe schema name from tenant ID
    let schema_name = format!("tenant_{}", tenant_id.to_lowercase().replace('-', "_"));

    // Insert tenant context into request extensions
    let ctx = TenantContext {
        tenant_id: tenant_id.to_string(),
        schema_name,
    };

    let mut request = request;
    request.extensions_mut().insert(ctx);

    Ok(next.run(request).await)
}

/// Ensure tenant schema exists
async fn ensure_tenant_schema(pool: &DbPool, schema_name: &str) -> Result<(), sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = $1)",
    )
    .bind(schema_name)
    .fetch_one(pool)
    .await?;

    if !exists {
        sqlx::query("CREATE SCHEMA ")
            .push(schema_name)
            .execute(pool)
            .await?;

        // Create todos table in tenant schema
        sqlx::query(&format!(
            "CREATE TABLE {}.todos (
                id UUID PRIMARY KEY NOT NULL,
                title VARCHAR(255) NOT NULL,
                description TEXT,
                completed BOOLEAN NOT NULL DEFAULT FALSE,
                tenant_id VARCHAR(64) NOT NULL,
                created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
            )",
            schema_name
        ))
        .execute(pool)
        .await?;

        sqlx::query(&format!(
            "CREATE INDEX {0}.idx_todos_tenant_created ON {0}.todos(tenant_id, created_at)",
            schema_name
        ))
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Execute a query in a tenant schema
async fn query_in_schema<T: sqlx::FromRow + Send + Unpin>(
    pool: &DbPool,
    schema: &str,
    query: &str,
    args: impl sqlx::IntoArguments + Send,
) -> Result<Vec<T>, sqlx::Error> {
    let full_query = format!("SET search_path TO {},public; {}", schema, query);
    sqlx::query_as::<_, T>(&full_query)
        .into_arguments(args)
        .fetch_all(pool)
        .await
}

/// Execute a scalar query in a tenant schema
async fn query_scalar_in_schema<T: sqlx::FromRow + Send + Unpin>(
    pool: &DbPool,
    schema: &str,
    query: &str,
    args: impl sqlx::IntoArguments + Send,
) -> Result<Option<T>, sqlx::Error> {
    let full_query = format!("SET search_path TO {},public; {}", schema, query);
    sqlx::query_as::<_, T>(&full_query)
        .into_arguments(args)
        .fetch_optional(pool)
        .await
}

/// Health check endpoint
async fn health_check() -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "multi-tenant"
    })))
}

/// List all todos for the current tenant
async fn list_todos(State(state): State<AppState>, request: Request<()>) -> Result<Json<TodoListResponse>, AppError> {
    let ctx = request
        .extensions()
        .get::<TenantContext>()
        .ok_or_else(|| AppError::bad_request("Tenant context not found"))?;

    // Ensure schema exists
    ensure_tenant_schema(&state.pool, &ctx.schema_name).await?;

    let todos = query_in_schema::<TenantTodo>(
        &state.pool,
        &ctx.schema_name,
        "SELECT id, title, description, completed, tenant_id, created_at, updated_at FROM todos WHERE tenant_id = $1 ORDER BY created_at DESC",
        (ctx.tenant_id.clone(),)
    ).await?;

    let responses: Vec<TodoResponse> = todos
        .into_iter()
        .map(|todo| TodoResponse {
            id: todo.id,
            title: todo.title,
            description: todo.description,
            completed: todo.completed,
            tenant_id: todo.tenant_id,
            created_at: todo.created_at.to_rfc3339(),
            updated_at: todo.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(TodoListResponse {
        todos: responses,
        total: responses.len(),
        tenant_id: ctx.tenant_id.clone(),
    }))
}

/// Get a specific todo
async fn get_todo(
    State(state): State<AppState>,
    request: Request<()>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<TodoResponse>, AppError> {
    let ctx = request
        .extensions()
        .get::<TenantContext>()
        .ok_or_else(|| AppError::bad_request("Tenant context not found"))?;

    ensure_tenant_schema(&state.pool, &ctx.schema_name).await?;

    let todo = query_scalar_in_schema::<TenantTodo>(
        &state.pool,
        &ctx.schema_name,
        "SELECT id, title, description, completed, tenant_id, created_at, updated_at FROM todos WHERE id = $1 AND tenant_id = $2",
        (id, ctx.tenant_id.clone()),
    ).await?;

    match todo {
        Some(todo) => Ok(Json(TodoResponse {
            id: todo.id,
            title: todo.title,
            description: todo.description,
            completed: todo.completed,
            tenant_id: todo.tenant_id,
            created_at: todo.created_at.to_rfc3339(),
            updated_at: todo.updated_at.to_rfc3339(),
        })),
        None => Err(AppError::not_found(format!("Todo {} not found", id))),
    }
}

/// Create a new todo
async fn create_todo(
    State(state): State<AppState>,
    request: Request<Json<CreateTodoRequest>>,
) -> Result<Json<TodoResponse>, AppError> {
    let ctx = request
        .extensions()
        .get::<TenantContext>()
        .ok_or_else(|| AppError::bad_request("Tenant context not found"))?;

    let Json(body) = request.into_parts().0 ;

    ensure_tenant_schema(&state.pool, &ctx.schema_name).await?;

    let id = Uuid::new_v4();
    let now = Utc::now();

    let todo = query_in_schema::<TenantTodo>(
        &state.pool,
        &ctx.schema_name,
        "INSERT INTO todos (id, title, description, completed, tenant_id, created_at, updated_at) 
         VALUES ($1, $2, $3, $4, $5, $6, $7) 
         RETURNING id, title, description, completed, tenant_id, created_at, updated_at",
        (id, body.title, body.description, false, ctx.tenant_id.clone(), now, now),
    ).await?;

    let todo = todo.into_iter().next().ok_or_else(|| AppError::internal("Failed to create todo"))?;

    Ok(Json(TodoResponse {
        id: todo.id,
        title: todo.title,
        description: todo.description,
        completed: todo.completed,
        tenant_id: todo.tenant_id,
        created_at: todo.created_at.to_rfc3339(),
        updated_at: todo.updated_at.to_rfc3339(),
    }))
}

/// Update a todo
async fn update_todo(
    State(state): State<AppState>,
    request: Request<Json<UpdateTodoRequest>>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<TodoResponse>, AppError> {
    let ctx = request
        .extensions()
        .get::<TenantContext>()
        .ok_or_else(|| AppError::bad_request("Tenant context not found"))?;

    let Json(body) = request.into_parts().0;

    ensure_tenant_schema(&state.pool, &ctx.schema_name).await?;

    let existing: Option<i64> = query_scalar_in_schema(
        &state.pool,
        &ctx.schema_name,
        "SELECT COUNT(*) FROM todos WHERE id = $1 AND tenant_id = $2",
        (id, ctx.tenant_id.clone()),
    ).await?;

    if existing.is_none() || existing.unwrap() == 0 {
        return Err(AppError::not_found(format!("Todo {} not found", id)));
    }

    let title = body.title.unwrap_or_else(|| "".to_string());
    let description = body.description;
    let completed = body.completed.unwrap_or(false);
    let now = Utc::now();

    let todo = query_in_schema::<TenantTodo>(
        &state.pool,
        &ctx.schema_name,
        "UPDATE todos 
         SET title = COALESCE($1, title), 
             description = COALESCE($2, description), 
             completed = COALESCE($3, completed), 
             updated_at = $4 
         WHERE id = $5 AND tenant_id = $6 
         RETURNING id, title, description, completed, tenant_id, created_at, updated_at",
        (title, description, completed, now, id, ctx.tenant_id.clone()),
    ).await?;

    let todo = todo.into_iter().next().ok_or_else(|| AppError::internal("Failed to update todo"))?;

    Ok(Json(TodoResponse {
        id: todo.id,
        title: todo.title,
        description: todo.description,
        completed: todo.completed,
        tenant_id: todo.tenant_id,
        created_at: todo.created_at.to_rfc3339(),
        updated_at: todo.updated_at.to_rfc3339(),
    }))
}

/// Delete a todo
async fn delete_todo(
    State(state): State<AppState>,
    request: Request<()>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ctx = request
        .extensions()
        .get::<TenantContext>()
        .ok_or_else(|| AppError::bad_request("Tenant context not found"))?;

    ensure_tenant_schema(&state.pool, &ctx.schema_name).await?;

    let deleted = sqlx::query_scalar::<_, i64>(&format!(
        "SET search_path TO {},public; DELETE FROM todos WHERE id = $1 AND tenant_id = $2",
        ctx.schema_name
    ))
    .bind(id)
    .bind(&ctx.tenant_id)
    .execute(&state.pool)
    .await?;

    if deleted.rows_affected() == 0 {
        return Err(AppError::not_found(format!("Todo {} not found", id)));
    }

    Ok(Json(serde_json::json!({
        "status": "deleted",
        "id": id.to_string(),
        "tenant_id": ctx.tenant_id
    })))
}

/// Create the application router
fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/todos", get(list_todos))
        .route("/todos", post(create_todo))
        .route("/todos/:id", get(get_todo))
        .route("/todos/:id", put(update_todo))
        .route("/todos/:id", delete(delete_todo))
        .layer(axum::middleware::from_fn(tenant_middleware))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("multi_tenant=info,tower_http=info,axa=info,sqlx=warn")
        .init();

    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    // Initialize database pool
    let pool = init_db_pool().await.expect("Failed to initialize database pool");

    // Create shared schema for common tables
    sqlx::query("CREATE SCHEMA IF NOT EXISTS shared")
        .execute(&pool)
        .await
        .expect("Failed to create shared schema");

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));

    tracing::info!("Starting multi-tenant service on {}", addr);

    let state = AppState { pool };

    // Start the application
    start_app(app(state), addr).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_check() {
        let pool = init_db_pool().await.unwrap();
        let state = AppState { pool };
        let app = app(state);

        let request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_missing_tenant_header() {
        let pool = init_db_pool().await.unwrap();
        let state = AppState { pool };
        let app = app(state);

        let request = Request::builder()
            .uri("/todos")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
