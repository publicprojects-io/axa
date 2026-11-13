//! A todo API on Postgres, with sqlx and Axa.
//!
//! `AppContext::db` is the pool, a violated constraint answers as a problem, and the
//! migrations in `migrations/` run at boot outside production.
//!
//! ```sh
//! cargo run
//! curl -X POST localhost:1241/api/todos -H 'content-type: application/json' -d '{"title":"Milk"}'
//! curl localhost:1241/api/todos
//! ```

use std::path::Path as FilePath;

use axa::{
    Environment, Result,
    config::{Config, Defaults},
    extract::{Json, Path},
    logger,
    postgres_sqlx::{
        self, Pool,
        sqlx::{self, migrate::Migrator},
    },
    prelude::{AppContext, AppRoutes, Response, Routes, State, bad_request, format, get, not_found},
    server,
};
use serde::{Deserialize, Serialize};

/// The longest title, which the table's check also holds to.
const MAX_TITLE_CHARS: usize = 200;

/// A todo, as the API sends it.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Todo {
    id: i64,
    title: String,
    completed: bool,
}

impl From<(i64, String, bool)> for Todo {
    fn from((id, title, completed): (i64, String, bool)) -> Self {
        Self {
            id,
            title,
            completed,
        }
    }
}

/// A new todo.
#[derive(Deserialize)]
struct NewTodo {
    title: String,
}

/// A change to a todo. A missing field is left as it is.
#[derive(Deserialize)]
struct TodoChange {
    title: Option<String>,
    completed: Option<bool>,
}

/// A title, trimmed, refused when blank or too long.
fn checked_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        return bad_request("title is blank");
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return bad_request("title is longer than 200 characters");
    }
    Ok(title.to_string())
}

async fn list(State(ctx): State<AppContext>) -> Result<Response> {
    let rows: Vec<(i64, String, bool)> =
        sqlx::query_as("SELECT id, title, completed FROM todos ORDER BY id LIMIT 100")
            .fetch_all(&ctx.db)
            .await?;
    format::json(rows.into_iter().map(Todo::from).collect::<Vec<_>>())
}

async fn create(State(ctx): State<AppContext>, Json(todo): Json<NewTodo>) -> Result<Response> {
    let title = checked_title(&todo.title)?;
    let row: (i64, String, bool) =
        sqlx::query_as("INSERT INTO todos (title) VALUES ($1) RETURNING id, title, completed")
            .bind(title)
            .fetch_one(&ctx.db)
            .await?;
    format::json(Todo::from(row))
}

async fn show(State(ctx): State<AppContext>, Path(id): Path<i64>) -> Result<Response> {
    let row: Option<(i64, String, bool)> =
        sqlx::query_as("SELECT id, title, completed FROM todos WHERE id = $1")
            .bind(id)
            .fetch_optional(&ctx.db)
            .await?;
    match row {
        Some(row) => format::json(Todo::from(row)),
        None => not_found(),
    }
}

async fn update(
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
    Json(change): Json<TodoChange>,
) -> Result<Response> {
    let title = change.title.as_deref().map(checked_title).transpose()?;
    let row: Option<(i64, String, bool)> = sqlx::query_as(
        "UPDATE todos SET title = COALESCE($2, title), completed = COALESCE($3, completed)
         WHERE id = $1 RETURNING id, title, completed",
    )
    .bind(id)
    .bind(title)
    .bind(change.completed)
    .fetch_optional(&ctx.db)
    .await?;
    match row {
        Some(row) => format::json(Todo::from(row)),
        None => not_found(),
    }
}

async fn remove(State(ctx): State<AppContext>, Path(id): Path<i64>) -> Result<Response> {
    let deleted = sqlx::query("DELETE FROM todos WHERE id = $1")
        .bind(id)
        .execute(&ctx.db)
        .await?
        .rows_affected();
    if deleted == 0 {
        return not_found();
    }
    format::empty()
}

/// The service's routes, with the health routes Axa adds.
fn routes() -> AppRoutes {
    AppRoutes::with_default_routes().add_route(
        Routes::new()
            .prefix("/api/todos")
            .add("/", get(list).post(create))
            .add("/{id}", get(show).patch(update).delete(remove)),
    )
}

/// The settings for `environment`, with a database named `todo_api`.
fn config(environment: &Environment) -> Result<Config> {
    let defaults = Defaults {
        database: "todo_api",
        ..Defaults::default()
    };
    Config::from_env(environment, &defaults)
}

/// The migrations in `migrations/`.
async fn migrator() -> Result<Migrator> {
    let directory = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    Migrator::new(directory).await.map_err(axa::Error::wrap)
}

/// Run the migrations when the settings say to.
async fn migrate_if_asked(pool: &Pool, config: &Config) -> Result<()> {
    if config.database.auto_migrate {
        postgres_sqlx::migrate(pool, &migrator().await?)
            .await
            .map_err(axa::Error::wrap)?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = config(&environment)?;
    logger::init("todo_api", &config.logger);

    let pool = axa::db::connect(&config.database).await?;
    migrate_if_asked(&pool, &config).await?;

    let server_config = config.server.clone();
    let ctx = AppContext::new(environment, pool, config);
    let router = routes().into_app(ctx)?;
    server::serve(router, &server_config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    /// The app over `pool`.
    fn app(pool: Pool) -> axum::Router {
        let config = config(&Environment::Test).unwrap();
        routes()
            .into_app(AppContext::new(Environment::Test, pool, config))
            .unwrap()
    }

    /// A pool that connects only when first used, for tests that never reach it.
    fn unused_pool() -> Pool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap()
    }

    async fn send(
        app: &axum::Router,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> (StatusCode, String) {
        let mut request = Request::builder().method(method).uri(path);
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let request = request
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[test]
    fn a_title_is_trimmed_and_refused_when_blank_or_too_long() {
        assert_eq!(checked_title("  Milk ").unwrap(), "Milk");
        assert!(checked_title("   ").is_err());
        assert!(checked_title(&"a".repeat(201)).is_err());
        assert!(checked_title(&"é".repeat(200)).is_ok());
    }

    #[tokio::test]
    async fn a_bad_request_is_a_problem_before_the_database_is_asked() {
        let app = app(unused_pool());
        let (status, body) = send(&app, "POST", "/api/todos", Some(r#"{"title":" "}"#)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("title is blank"));
        let (status, _) = send(&app, "POST", "/api/todos", Some(r#"{"name":"x"}"#)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, _) = send(&app, "GET", "/api/todos/not-a-number", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Against a real Postgres, on a database this test may wipe:
    ///
    /// ```sh
    /// AXA_TEST_DB_URL=postgres://... cargo test -- --ignored
    /// ```
    #[tokio::test]
    #[ignore = "needs AXA_TEST_DB_URL"]
    async fn a_todo_is_created_read_changed_and_removed() {
        let url = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect(&url)
            .await
            .unwrap();
        sqlx::query("DROP TABLE IF EXISTS todos, _sqlx_migrations")
            .execute(&pool)
            .await
            .unwrap();
        postgres_sqlx::migrate(&pool, &migrator().await.unwrap())
            .await
            .unwrap();
        let app = app(pool);

        let (status, body) = send(&app, "POST", "/api/todos", Some(r#"{"title":"Milk"}"#)).await;
        assert_eq!(status, StatusCode::OK);
        let todo: Todo = serde_json::from_str(&body).unwrap();
        assert_eq!(todo.title, "Milk");
        assert!(!todo.completed);

        let path = format!("/api/todos/{}", todo.id);
        let (status, body) = send(&app, "PATCH", &path, Some(r#"{"completed":true}"#)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(serde_json::from_str::<Todo>(&body).unwrap().completed);

        let (_, body) = send(&app, "GET", "/api/todos", None).await;
        assert_eq!(serde_json::from_str::<Vec<Todo>>(&body).unwrap().len(), 1);

        assert_eq!(send(&app, "DELETE", &path, None).await.0, StatusCode::OK);
        assert_eq!(send(&app, "GET", &path, None).await.0, StatusCode::NOT_FOUND);
    }
}
