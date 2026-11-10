//! What a violated constraint, and a database that cannot serve for now, answer on MySQL
//! and SQLite, against real databases, because a driver only reports either from a real
//! database error. The SQLite tests make a file of their own, so they run in
//! `cargo test`. The MySQL tests need `AXA_TEST_MYSQL_URL`:
//!
//! ```sh
//! AXA_TEST_MYSQL_URL=mysql://... cargo test --all-features --test other_databases -- --ignored --test-threads=1
//! ```

#![cfg(any(
    feature = "mysql-sqlx",
    feature = "mysql-toasty",
    feature = "sqlite-sqlx",
    feature = "sqlite-seaorm",
    feature = "sqlite-diesel"
))]

use axa::Error;
use axum::{
    body::to_bytes,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

/// The tables, for SQLite.
#[cfg(any(
    feature = "sqlite-sqlx",
    feature = "sqlite-seaorm",
    feature = "sqlite-diesel"
))]
const SQLITE_SCHEMA: [&str; 2] = [
    "CREATE TABLE axa_constraint_parent (id INT PRIMARY KEY, email TEXT UNIQUE, \
     sex TEXT CHECK (sex IN ('female', 'male')), name TEXT NOT NULL DEFAULT 'carol')",
    "CREATE TABLE axa_constraint_child (id INT PRIMARY KEY, \
     parent INT NOT NULL REFERENCES axa_constraint_parent (id))",
];

/// The tables, for MySQL, which indexes no `TEXT` column whole.
#[cfg(any(feature = "mysql-sqlx", feature = "mysql-toasty"))]
const MYSQL_SCHEMA: [&str; 4] = [
    "DROP TABLE IF EXISTS axa_constraint_child",
    "DROP TABLE IF EXISTS axa_constraint_parent",
    "CREATE TABLE axa_constraint_parent (id INT PRIMARY KEY, email VARCHAR(255) UNIQUE, \
     sex VARCHAR(16) CHECK (sex IN ('female', 'male')), \
     name VARCHAR(255) NOT NULL DEFAULT 'carol')",
    "CREATE TABLE axa_constraint_child (id INT PRIMARY KEY, \
     parent INT NOT NULL, FOREIGN KEY (parent) REFERENCES axa_constraint_parent (id))",
];

const INSERT_PARENT: &str =
    "INSERT INTO axa_constraint_parent VALUES (1, 'carol@example.org', NULL, 'carol')";
const DUPLICATE: &str =
    "INSERT INTO axa_constraint_parent VALUES (2, 'carol@example.org', NULL, 'dan')";
const NO_PARENT: &str = "INSERT INTO axa_constraint_child VALUES (1, 99)";
const OUT_OF_RANGE: &str = "INSERT INTO axa_constraint_parent (id, sex) VALUES (3, 'banana')";
const MISSING: &str = "INSERT INTO axa_constraint_parent (id, name) VALUES (4, NULL)";
const NOT_A_CONSTRAINT: &str = "SELECT * FROM axa_constraint_no_such_table";

async fn assert_answers(response: Response, status: StatusCode, reason: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        axa::problem::PROBLEM_JSON
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains(reason), "{body}");
    assert!(!body.contains("carol"), "{body}");
    assert!(!body.contains("axa_constraint"), "{body}");
    assert!(!body.contains("banana"), "{body}");
}

/// Each statement that breaks a constraint answers as the caller's mistake, and one that
/// breaks none answers `500`, whatever the database.
async fn assert_each_constraint(fails: impl AsyncFn(&'static str) -> Response) {
    assert_answers(fails(DUPLICATE).await, StatusCode::CONFLICT, "conflict").await;
    assert_answers(fails(NO_PARENT).await, StatusCode::CONFLICT, "conflict").await;
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    assert_answers(
        fails(OUT_OF_RANGE).await,
        unprocessable,
        "outside what this field allows",
    )
    .await;
    assert_answers(
        fails(MISSING).await,
        unprocessable,
        "A required value is missing",
    )
    .await;
    assert_answers(
        fails(NOT_A_CONSTRAINT).await,
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
    )
    .await;
}

/// The answer to a database that cannot serve for now.
#[cfg(any(feature = "mysql-sqlx", feature = "sqlite-sqlx"))]
async fn assert_unavailable(response: Response) {
    assert!(response.headers().contains_key(header::RETRY_AFTER));
    assert_answers(response, StatusCode::SERVICE_UNAVAILABLE, "unavailable").await;
}

/// A new database file's URL, under Cargo's temporary directory for tests.
#[cfg(any(
    feature = "sqlite-sqlx",
    feature = "sqlite-seaorm",
    feature = "sqlite-diesel"
))]
fn sqlite_file(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("axa-{name}-{}.sqlite3", uuid::Uuid::new_v4()));
    format!("sqlite://{}", path.display())
}

#[cfg(feature = "sqlite-sqlx")]
#[tokio::test]
async fn a_violated_constraint_through_sqlite_sqlx_is_the_callers_mistake() {
    use axa::sqlite_sqlx::{PoolConfig, connect, sqlx};

    let file = sqlite_file("sqlx");
    let pool = connect(&PoolConfig::new(format!("{file}?mode=rwc")))
        .await
        .unwrap();
    for statement in SQLITE_SCHEMA.into_iter().chain([INSERT_PARENT]) {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    assert_each_constraint(async |statement| {
        let error = sqlx::query(statement).execute(&pool).await.unwrap_err();
        Error::from(error).into_response()
    })
    .await;

    // Opened read-only, a write is refused by a database that cannot serve it now.
    let read_only = connect(&PoolConfig::new(format!("{file}?mode=ro")))
        .await
        .unwrap();
    let error = sqlx::query("INSERT INTO axa_constraint_parent (id) VALUES (9)")
        .execute(&read_only)
        .await
        .unwrap_err();
    assert_unavailable(Error::from(error).into_response()).await;
}

#[cfg(feature = "sqlite-seaorm")]
#[tokio::test]
async fn a_violated_constraint_through_sqlite_seaorm_is_the_callers_mistake() {
    use axa::sqlite_seaorm::{PoolConfig, connect, sea_orm::ConnectionTrait};

    let db = connect(&PoolConfig::new(format!(
        "{}?mode=rwc",
        sqlite_file("seaorm")
    )))
    .await
    .unwrap();
    for statement in SQLITE_SCHEMA.into_iter().chain([INSERT_PARENT]) {
        db.execute_unprepared(statement).await.unwrap();
    }
    assert_each_constraint(async |statement| {
        let error = db.execute_unprepared(statement).await.unwrap_err();
        Error::from(error).into_response()
    })
    .await;
}

#[cfg(feature = "sqlite-diesel")]
#[tokio::test]
async fn a_violated_constraint_through_sqlite_diesel_is_the_callers_mistake() {
    use axa::sqlite_diesel::{PoolConfig, connect, diesel, diesel_async::RunQueryDsl};

    let pool = connect(&PoolConfig::new(format!(
        "{}?mode=rwc",
        sqlite_file("diesel")
    )))
    .await
    .unwrap();
    let mut connection = pool.get().await.unwrap();
    for statement in SQLITE_SCHEMA.into_iter().chain([INSERT_PARENT]) {
        diesel::sql_query(statement)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    let connection = tokio::sync::Mutex::new(connection);
    assert_each_constraint(async |statement| {
        let mut connection = connection.lock().await;
        let error = diesel::sql_query(statement)
            .execute(&mut *connection)
            .await
            .unwrap_err();
        Error::from(error).into_response()
    })
    .await;
}

#[cfg(any(feature = "mysql-sqlx", feature = "mysql-toasty"))]
fn mysql_url() -> String {
    std::env::var("AXA_TEST_MYSQL_URL").expect("AXA_TEST_MYSQL_URL")
}

#[cfg(feature = "mysql-sqlx")]
#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_violated_constraint_through_mysql_sqlx_is_the_callers_mistake() {
    use axa::mysql_sqlx::{PoolConfig, connect, sqlx};

    let pool = connect(&PoolConfig::new(mysql_url())).await.unwrap();
    for statement in MYSQL_SCHEMA.into_iter().chain([INSERT_PARENT]) {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    assert_each_constraint(async |statement| {
        let error = sqlx::query(statement).execute(&pool).await.unwrap_err();
        Error::from(error).into_response()
    })
    .await;

    // A write in a read-only transaction is refused by a database that cannot serve it.
    let mut connection = pool.acquire().await.unwrap();
    sqlx::query("SET SESSION TRANSACTION READ ONLY")
        .execute(&mut *connection)
        .await
        .unwrap();
    let error = sqlx::query("INSERT INTO axa_constraint_parent (id) VALUES (9)")
        .execute(&mut *connection)
        .await
        .unwrap_err();
    sqlx::query("SET SESSION TRANSACTION READ WRITE")
        .execute(&mut *connection)
        .await
        .unwrap();
    assert_unavailable(Error::from(error).into_response()).await;
}

#[cfg(feature = "mysql-toasty")]
#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_violated_constraint_through_mysql_toasty_is_the_callers_mistake() {
    use axa::mysql_toasty::{PoolConfig, connect, toasty};

    let db = connect(&PoolConfig::new(mysql_url()), toasty::ModelSet::default())
        .await
        .unwrap();
    for statement in MYSQL_SCHEMA.into_iter().chain([INSERT_PARENT]) {
        let mut db = db.clone();
        toasty::sql::statement(statement)
            .exec(&mut db)
            .await
            .unwrap();
    }
    assert_each_constraint(async |statement| {
        let mut db = db.clone();
        let error = toasty::sql::statement(statement)
            .exec(&mut db)
            .await
            .unwrap_err();
        Error::from(error).into_response()
    })
    .await;
}
