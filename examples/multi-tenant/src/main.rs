//! A todo API shared by many tenants, on Postgres, with Axa.
//!
//! The tenant comes from the caller's verified PASETO token, never from a header or a
//! path the caller chooses. Every row names its tenant, every query filters on it, and
//! Postgres row-level security refuses another tenant's rows as a second fence.
//!
//! ```sh
//! cargo run
//! curl localhost:1241/api/todos -H "authorization: Bearer $TOKEN"
//! ```

use std::{path::Path as FilePath, sync::Arc};

use authentication_verifier::{Policy, ReloadablePolicy, ReloadableVerifier, Verifier};
use axa::{
    Environment, Error, Result,
    config::{Config, Defaults},
    extract::{Json, Path},
    logger, middleware,
    paseto::{self, Guard, GuardConfig, Tenant},
    postgres_sqlx::{
        self, Pool,
        sqlx::{self, Postgres, Transaction, migrate::Migrator},
    },
    prelude::{
        AppContext, AppRoutes, Response, Routes, State, bad_request, format, get, not_found,
    },
    server,
};
use serde::{Deserialize, Serialize};

/// The longest title, which the table's check also holds to.
const MAX_TITLE_CHARS: usize = 200;

/// A transaction in which row-level security sees only `tenant`'s rows. Axa sets
/// `app.tenant` bound, for the transaction only, so a pooled connection never carries
/// one tenant into the next request.
async fn in_tenant(pool: &Pool, tenant: &Tenant) -> Result<Transaction<'static, Postgres>> {
    Ok(postgres_sqlx::begin_with(pool, &[("app.tenant", tenant.as_str())]).await?)
}

/// A todo, as the API sends it. The tenant is not sent: the caller knows it.
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

/// A new todo. A tenant in the body would be ignored, so there is no field for one.
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

async fn list(State(ctx): State<AppContext>, tenant: Tenant) -> Result<Response> {
    let mut tx = in_tenant(&ctx.db, &tenant).await?;
    let rows: Vec<(i64, String, bool)> = sqlx::query_as(
        "SELECT id, title, completed FROM todos WHERE tenant_id = $1 ORDER BY id LIMIT 100",
    )
    .bind(tenant.as_str())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    format::json(rows.into_iter().map(Todo::from).collect::<Vec<_>>())
}

async fn create(
    State(ctx): State<AppContext>,
    tenant: Tenant,
    Json(todo): Json<NewTodo>,
) -> Result<Response> {
    let title = checked_title(&todo.title)?;
    let mut tx = in_tenant(&ctx.db, &tenant).await?;
    let row: (i64, String, bool) = sqlx::query_as(
        "INSERT INTO todos (tenant_id, title) VALUES ($1, $2) RETURNING id, title, completed",
    )
    .bind(tenant.as_str())
    .bind(title)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    format::json(Todo::from(row))
}

/// Another tenant's todo is `404`, as a missing one is, so that a caller cannot learn
/// which ids other tenants hold.
async fn show(
    State(ctx): State<AppContext>,
    tenant: Tenant,
    Path(id): Path<i64>,
) -> Result<Response> {
    let mut tx = in_tenant(&ctx.db, &tenant).await?;
    let row: Option<(i64, String, bool)> =
        sqlx::query_as("SELECT id, title, completed FROM todos WHERE tenant_id = $1 AND id = $2")
            .bind(tenant.as_str())
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    tx.commit().await?;
    row.map_or_else(not_found, |row| format::json(Todo::from(row)))
}

async fn update(
    State(ctx): State<AppContext>,
    tenant: Tenant,
    Path(id): Path<i64>,
    Json(change): Json<TodoChange>,
) -> Result<Response> {
    let title = change.title.as_deref().map(checked_title).transpose()?;
    let mut tx = in_tenant(&ctx.db, &tenant).await?;
    let row: Option<(i64, String, bool)> = sqlx::query_as(
        "UPDATE todos SET title = COALESCE($3, title), completed = COALESCE($4, completed)
         WHERE tenant_id = $1 AND id = $2 RETURNING id, title, completed",
    )
    .bind(tenant.as_str())
    .bind(id)
    .bind(title)
    .bind(change.completed)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    row.map_or_else(not_found, |row| format::json(Todo::from(row)))
}

async fn remove(
    State(ctx): State<AppContext>,
    tenant: Tenant,
    Path(id): Path<i64>,
) -> Result<Response> {
    let mut tx = in_tenant(&ctx.db, &tenant).await?;
    let deleted = sqlx::query("DELETE FROM todos WHERE tenant_id = $1 AND id = $2")
        .bind(tenant.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
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

/// The routes behind `guard`, inside Axa's standard middleware, so that a refusal has
/// a request id and the security headers.
fn app(ctx: AppContext, guard: Guard) -> Result<axum::Router> {
    let http = ctx.config.http.clone();
    let router = routes()
        .into_router(ctx)
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(guard),
            paseto::middleware,
        ));
    middleware::apply(router, &http)
}

/// A guard that verifies tokens against the key set in `PASETO_KEYS_FILE`, for
/// `PASETO_ISSUER` and `PASETO_AUDIENCE`, under the default policy: reads for any
/// verified caller, writes for `access=write`, deletes for `access=admin`.
fn guard_from_env() -> Result<Guard> {
    let variable =
        |name: &str| std::env::var(name).map_err(|_| Error::string(&format!("{name} must be set")));
    let keys = std::fs::read_to_string(variable("PASETO_KEYS_FILE")?)?;
    let keys: serde_json::Value = serde_json::from_str(&keys)?;
    let verifier = Verifier::from_paseto_keys_value(
        &keys,
        &variable("PASETO_ISSUER")?,
        &variable("PASETO_AUDIENCE")?,
    )
    .map_err(Error::wrap)?;
    Ok(Guard::new(
        GuardConfig::new("todo"),
        Arc::new(ReloadableVerifier::new(verifier)),
        Arc::new(ReloadablePolicy::new(Policy::default_policy())),
    ))
}

/// The settings for `environment`, with a database named `multi_tenant`.
fn config(environment: &Environment) -> Result<Config> {
    let defaults = Defaults {
        database: "multi_tenant",
        ..Defaults::default()
    };
    Config::from_env(environment, &defaults)
}

/// The migrations in `migrations/`.
async fn migrator() -> Result<Migrator> {
    let directory = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    Migrator::new(directory).await.map_err(Error::wrap)
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = config(&environment)?;
    logger::init("multi_tenant", &config.logger);
    let guard = guard_from_env()?;

    let pool = axa::db::connect(&config.database).await?;
    if config.database.auto_migrate {
        postgres_sqlx::migrate(&pool, &migrator().await?)
            .await
            .map_err(Error::wrap)?;
    }

    // Refuse to serve, in production, where the policy would not keep tenants apart.
    axa::db::require_row_security(&pool, &["todos"], &environment).await?;

    let server_config = config.server.clone();
    let router = app(AppContext::new(environment, pool, config), guard)?;
    server::serve(router, &server_config).await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use authentication_verifier::Claims;

    use super::*;
    use axa_test::paseto::TestKeys;
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    const ISSUER: &str = "authentication-service";
    const AUDIENCE: &str = "multi-tenant";

    /// A token for a caller with `attrs`, an object of lists, signed under Axa's test
    /// key as the auth service would sign it.
    fn token(attrs: &Value) -> String {
        let mut token = TestKeys::new().token(ISSUER, AUDIENCE);
        for (name, values) in attrs.as_object().unwrap() {
            let values = values
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap());
            token = token.attr(name, values);
        }
        token.sign()
    }

    fn guard() -> Guard {
        Guard::new(
            GuardConfig::new("todo"),
            Arc::new(ReloadableVerifier::new(
                TestKeys::new().verifier(ISSUER, AUDIENCE),
            )),
            Arc::new(ReloadablePolicy::new(Policy::default_policy())),
        )
    }

    fn test_app(pool: Pool) -> axum::Router {
        let config = config(&Environment::Test).unwrap();
        app(AppContext::new(Environment::Test, pool, config), guard()).unwrap()
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
        token: Option<&str>,
        body: Option<&str>,
    ) -> (StatusCode, String) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
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

    fn claims_with(attrs: &[(&str, &[&str])]) -> Claims {
        let attrs: BTreeMap<String, Vec<String>> = attrs
            .iter()
            .map(|(key, values)| {
                (
                    (*key).to_string(),
                    values.iter().map(ToString::to_string).collect(),
                )
            })
            .collect();
        serde_json::from_value(json!({
            "sub": "s", "email": "e", "name": "n", "iss": ISSUER, "aud": AUDIENCE,
            "exp": 0, "iat": 0, "sid": "x", "attrs": attrs,
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn a_caller_with_no_token_or_no_tenant_is_refused_before_the_database() {
        let app = test_app(unused_pool());
        let (status, _) = send(&app, "GET", "/api/todos", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let no_tenant = token(&json!({ "access": ["write"] }));
        let (status, body) = send(&app, "GET", "/api/todos", Some(&no_tenant), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("no_tenant"), "{body}");

        let two = token(&json!({ "tenant": ["acme", "globex"] }));
        let (status, _) = send(&app, "GET", "/api/todos", Some(&two), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_policy_decides_writes_before_the_tenant_is_asked() {
        let app = test_app(unused_pool());
        let reader = token(&json!({ "tenant": ["acme"] }));
        let body = Some(r#"{"title":"Milk"}"#);
        let (status, _) = send(&app, "POST", "/api/todos", Some(&reader), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(&app, "DELETE", "/api/todos/1", Some(&reader), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// Against a real Postgres, on a database this test may wipe, as a role that is
    /// not a superuser and has no `BYPASSRLS`, so that row-level security applies:
    ///
    /// ```sh
    /// AXA_TEST_DB_URL=postgres://... cargo test -- --ignored
    /// ```
    #[tokio::test]
    #[ignore = "needs AXA_TEST_DB_URL"]
    async fn one_tenant_never_sees_or_changes_another_tenants_todos() {
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
        let app = test_app(pool.clone());
        let acme = token(&json!({ "tenant": ["acme"], "access": ["admin"] }));
        let globex = token(&json!({ "tenant": ["globex"], "access": ["admin"] }));

        let body = Some(r#"{"title":"Acme's secret plan"}"#);
        let (status, created) = send(&app, "POST", "/api/todos", Some(&acme), body).await;
        assert_eq!(status, StatusCode::OK);
        let todo: Todo = serde_json::from_str(&created).unwrap();
        let path = format!("/api/todos/{}", todo.id);

        let (_, listed) = send(&app, "GET", "/api/todos", Some(&globex), None).await;
        assert_eq!(listed, "[]");
        for (method, body) in [
            ("GET", None),
            ("PATCH", Some(r#"{"completed":true}"#)),
            ("DELETE", None),
        ] {
            let (status, _) = send(&app, method, &path, Some(&globex), body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
        }
        let (status, _) = send(&app, "GET", &path, Some(&acme), None).await;
        assert_eq!(status, StatusCode::OK);

        // The second fence: a query that forgets the filter still sees no other
        // tenant's rows.
        let globex_tenant =
            Tenant::from_claims(&claims_with(&[("tenant", &["globex"])]), "tenant").unwrap();
        let mut tx = in_tenant(&pool, &globex_tenant).await.unwrap();
        let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM todos")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count, 0, "row-level security does not apply to this role");
    }
}
