//! Todo API example
//!
//! This example demonstrates a RESTful Todo API using axa infrastructure.
//! It shows:
//! - Database integration with axa::db
//! - CRUD operations
//! - Request validation
//! - Error handling
//! - Structured responses
//!
//! Run with: cargo run
//! Requires: PostgreSQL database (see .env.example)

use axa::{
    db::{DbPool, init_db_pool},
    error::AppError,
    start_app,
    web::AppState,
};
use axum::{
    extract::State,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;
use validator::Validate;

/// Todo item DTO
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct Todo {
    id: Uuid,
    title: String,
    description: Option<String>,
    completed: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Create Todo request
#[derive(Debug, Deserialize, Serialize, Validate)]
struct CreateTodoRequest {
    #[validate(length(min = 1, max = 255, message = "Title must be 1-255 characters"))]
    title: String,
    description: Option<String>,
}

/// Update Todo request
#[derive(Debug, Deserialize, Serialize, Validate)]
struct UpdateTodoRequest {
    #[validate(length(min = 1, max = 255, message = "Title must be 1-255 characters"))]
    title: Option<String>,
    description: Option<String>,
    completed: Option<bool>,
}

/// Todo response (single item)
#[derive(Debug, Serialize)]
struct TodoResponse {
    id: Uuid,
    title: String,
    description: Option<String>,
    completed: bool,
    created_at: String,
    updated_at: String,
}

/// List response
#[derive(Debug, Serialize)]
struct TodoListResponse {
    todos: Vec<TodoResponse>,
    total: usize,
}

/// Application state
#[derive(Clone)]
struct AppState {
    pool: DbPool,
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
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "todo-api"
    })))
}

/// List all todos
async fn list_todos(State(state): State<AppState>) -> Result<Json<TodoListResponse>, AppError> {
    let todos = sqlx::query_as::<_, Todo>(
        "SELECT id, title, description, completed, created_at, updated_at FROM todos ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;

    let responses: Vec<TodoResponse> = todos
        .into_iter()
        .map(|todo| TodoResponse {
            id: todo.id,
            title: todo.title,
            description: todo.description,
            completed: todo.completed,
            created_at: todo.created_at.to_rfc3339(),
            updated_at: todo.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(TodoListResponse {
        todos: responses,
        total: responses.len(),
    }))
}

/// Get a single todo
async fn get_todo(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<TodoResponse>, AppError> {
    let todo = sqlx::query_as::<_, Todo>(
        "SELECT id, title, description, completed, created_at, updated_at FROM todos WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    match todo {
        Some(todo) => Ok(Json(TodoResponse {
            id: todo.id,
            title: todo.title,
            description: todo.description,
            completed: todo.completed,
            created_at: todo.created_at.to_rfc3339(),
            updated_at: todo.updated_at.to_rfc3339(),
        })),
        None => Err(AppError::not_found(format!("Todo {} not found", id))),
    }
}

/// Create a new todo
async fn create_todo(
    State(state): State<AppState>,
    Json(request): Json<CreateTodoRequest>,
) -> Result<Json<TodoResponse>, AppError> {
    request.validate()?;

    let id = Uuid::new_v4();
    let now = Utc::now();

    let todo = sqlx::query_as::<_, Todo>(
        "INSERT INTO todos (id, title, description, completed, created_at, updated_at) 
         VALUES ($1, $2, $3, $4, $5, $6) 
         RETURNING id, title, description, completed, created_at, updated_at",
    )
    .bind(id)
    .bind(&request.title)
    .bind(&request.description)
    .bind(false)
    .bind(now)
    .bind(now)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(TodoResponse {
        id: todo.id,
        title: todo.title,
        description: todo.description,
        completed: todo.completed,
        created_at: todo.created_at.to_rfc3339(),
        updated_at: todo.updated_at.to_rfc3339(),
    }))
}

/// Update a todo
async fn update_todo(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Json(request): Json<UpdateTodoRequest>,
) -> Result<Json<TodoResponse>, AppError> {
    request.validate()?;

    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM todos WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;

    if existing == 0 {
        return Err(AppError::not_found(format!("Todo {} not found", id)));
    }

    let title = request.title.unwrap_or_else(|| "".to_string());
    let description = request.description;
    let completed = request.completed.unwrap_or(false);
    let now = Utc::now();

    let todo = sqlx::query_as::<_, Todo>(
        "UPDATE todos 
         SET title = COALESCE($1, title), 
             description = COALESCE($2, description), 
             completed = COALESCE($3, completed), 
             updated_at = $4 
         WHERE id = $5 
         RETURNING id, title, description, completed, created_at, updated_at",
    )
    .bind(&title)
    .bind(&description)
    .bind(completed)
    .bind(now)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(TodoResponse {
        id: todo.id,
        title: todo.title,
        description: todo.description,
        completed: todo.completed,
        created_at: todo.created_at.to_rfc3339(),
        updated_at: todo.updated_at.to_rfc3339(),
    }))
}

/// Delete a todo
async fn delete_todo(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let deleted = sqlx::query_scalar::<_, i64>(
        "DELETE FROM todos WHERE id = $1",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;

    if deleted.rows_affected() == 0 {
        return Err(AppError::not_found(format!("Todo {} not found", id)));
    }

    Ok(Json(serde_json::json!({
        "status": "deleted",
        "id": id.to_string()
    })))
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("todo_api=info,tower_http=info,axa=info,sqlx=warn")
        .init();

    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    // Initialize database pool using axa's helper
    let pool = init_db_pool().await.expect("Failed to initialize database pool");

    // Run migrations
    sqlx::migrate!().run(&pool).await.expect("Failed to run migrations");

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));

    tracing::info!("Starting todo-api on {}", addr);

    let state = AppState { pool };

    // Start the application
    start_app(app(state), addr).await.unwrap();
}
