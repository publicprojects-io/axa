//! What a violated constraint answers, against a real Postgres, because a driver only
//! reports a violation from a real database error.

#![cfg(any(
    feature = "queue",
    feature = "sqlx",
    feature = "sea-orm",
    feature = "diesel",
    feature = "toasty"
))]

use axa::Error;
use axum::{
    body::to_bytes,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

const SCHEMA: [&str; 4] = [
    "DROP TABLE IF EXISTS axa_constraint_child",
    "DROP TABLE IF EXISTS axa_constraint_parent",
    "CREATE TABLE axa_constraint_parent (id INT PRIMARY KEY, email TEXT UNIQUE, \
     sex TEXT CHECK (sex IN ('female', 'male')), name TEXT NOT NULL DEFAULT 'carol')",
    "CREATE TABLE axa_constraint_child (id INT PRIMARY KEY, \
     parent INT NOT NULL REFERENCES axa_constraint_parent (id))",
];
const INSERT_PARENT: &str = "INSERT INTO axa_constraint_parent VALUES (1, 'carol@example.org')";
const DUPLICATE: &str = "INSERT INTO axa_constraint_parent VALUES (2, 'carol@example.org')";
const NO_PARENT: &str = "INSERT INTO axa_constraint_child VALUES (1, 99)";
const OUT_OF_RANGE: &str = "INSERT INTO axa_constraint_parent (id, sex) VALUES (3, 'banana')";
const MISSING: &str = "INSERT INTO axa_constraint_parent (id, name) VALUES (4, NULL)";
const NOT_A_CONSTRAINT: &str = "SELECT * FROM axa_constraint_no_such_table";

fn url() -> String {
    std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL")
}

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
    assert!(!body.contains("sex"), "{body}");
}

/// A database that refuses a write because it is read-only is the service's trouble,
/// and passes: `503` with `Retry-After`, not a `500` and not the caller's mistake.
#[cfg(any(feature = "queue", feature = "sqlx"))]
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_read_only_database_is_unavailable_not_a_failure() {
    let pool = sqlx::PgPool::connect(&url()).await.unwrap();
    // A table of its own, as the other tests here drop and make theirs.
    sqlx::query("CREATE TABLE IF NOT EXISTS axa_read_only_probe (id INT)")
        .execute(&pool)
        .await
        .unwrap();
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *transaction)
        .await
        .unwrap();
    let error = sqlx::query("INSERT INTO axa_read_only_probe VALUES (1)")
        .execute(&mut *transaction)
        .await
        .unwrap_err();
    let response = Error::from(error).into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.headers().contains_key(header::RETRY_AFTER));
    assert_answers(response, StatusCode::SERVICE_UNAVAILABLE, "unavailable").await;
}

#[cfg(any(feature = "queue", feature = "sqlx"))]
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_violated_constraint_through_sqlx_is_the_callers_mistake() {
    let pool = sqlx::PgPool::connect(&url()).await.unwrap();
    for statement in SCHEMA.into_iter().chain([INSERT_PARENT]) {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    let fails = async |statement: &'static str| {
        let error = sqlx::query(statement).execute(&pool).await.unwrap_err();
        Error::from(error).into_response()
    };
    assert_answers(fails(DUPLICATE).await, StatusCode::CONFLICT, "conflict").await;
    assert_answers(fails(NO_PARENT).await, StatusCode::CONFLICT, "conflict").await;
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    let out_of_range = fails(OUT_OF_RANGE).await;
    assert_answers(
        out_of_range,
        unprocessable,
        "outside what this field allows",
    )
    .await;
    let missing = fails(MISSING).await;
    assert_answers(missing, unprocessable, "A required value is missing").await;
    let other = fails(NOT_A_CONSTRAINT).await;
    assert_answers(other, StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}

#[cfg(feature = "sea-orm")]
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_violated_constraint_through_sea_orm_is_the_callers_mistake() {
    use sea_orm::{ConnectionTrait, Database};

    let db = Database::connect(url()).await.unwrap();
    for statement in SCHEMA.into_iter().chain([INSERT_PARENT]) {
        db.execute_unprepared(statement).await.unwrap();
    }
    let fails = async |statement: &'static str| {
        let error = db.execute_unprepared(statement).await.unwrap_err();
        Error::from(error).into_response()
    };
    assert_answers(fails(DUPLICATE).await, StatusCode::CONFLICT, "conflict").await;
    assert_answers(fails(NO_PARENT).await, StatusCode::CONFLICT, "conflict").await;
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    let out_of_range = fails(OUT_OF_RANGE).await;
    assert_answers(
        out_of_range,
        unprocessable,
        "outside what this field allows",
    )
    .await;
    let missing = fails(MISSING).await;
    assert_answers(missing, unprocessable, "A required value is missing").await;
    let other = fails(NOT_A_CONSTRAINT).await;
    assert_answers(other, StatusCode::INTERNAL_SERVER_ERROR, "internal").await;

    // The model layer wraps the same error, and answers the same.
    let error = db.execute_unprepared(DUPLICATE).await.unwrap_err();
    let wrapped = Error::Model(axa::model::ModelError::DbErr(error)).into_response();
    assert_answers(wrapped, StatusCode::CONFLICT, "conflict").await;
    let error = db.execute_unprepared(OUT_OF_RANGE).await.unwrap_err();
    let wrapped = Error::Model(axa::model::ModelError::DbErr(error)).into_response();
    assert_answers(
        wrapped,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unprocessable_entity",
    )
    .await;
}

#[cfg(feature = "diesel")]
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_violated_constraint_through_diesel_is_the_callers_mistake() {
    use axa::diesel::{PoolConfig, connect, diesel, diesel_async::RunQueryDsl};

    let pool = connect(&PoolConfig::new(url())).await.unwrap();
    let mut connection = pool.get().await.unwrap();
    for statement in SCHEMA.into_iter().chain([INSERT_PARENT]) {
        diesel::sql_query(statement)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    let mut fails = async |statement: &'static str| {
        let error = diesel::sql_query(statement)
            .execute(&mut connection)
            .await
            .unwrap_err();
        Error::from(error).into_response()
    };
    assert_answers(fails(DUPLICATE).await, StatusCode::CONFLICT, "conflict").await;
    assert_answers(fails(NO_PARENT).await, StatusCode::CONFLICT, "conflict").await;
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    let out_of_range = fails(OUT_OF_RANGE).await;
    assert_answers(
        out_of_range,
        unprocessable,
        "outside what this field allows",
    )
    .await;
    let missing = fails(MISSING).await;
    assert_answers(missing, unprocessable, "A required value is missing").await;
    let other = fails(NOT_A_CONSTRAINT).await;
    assert_answers(other, StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}

#[cfg(feature = "toasty")]
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_violated_constraint_through_toasty_is_the_callers_mistake() {
    use axa::toasty::{PoolConfig, connect, toasty};

    let mut db = connect(&PoolConfig::new(url()), toasty::ModelSet::default())
        .await
        .unwrap();
    for statement in SCHEMA.into_iter().chain([INSERT_PARENT]) {
        toasty::sql::statement(statement)
            .exec(&mut db)
            .await
            .unwrap();
    }
    let mut fails = async |statement: &'static str| {
        let error = toasty::sql::statement(statement)
            .exec(&mut db)
            .await
            .unwrap_err();
        Error::from(error).into_response()
    };
    assert_answers(fails(DUPLICATE).await, StatusCode::CONFLICT, "conflict").await;
    assert_answers(fails(NO_PARENT).await, StatusCode::CONFLICT, "conflict").await;
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    let out_of_range = fails(OUT_OF_RANGE).await;
    assert_answers(
        out_of_range,
        unprocessable,
        "outside what this field allows",
    )
    .await;
    let missing = fails(MISSING).await;
    assert_answers(missing, unprocessable, "A required value is missing").await;
    let other = fails(NOT_A_CONSTRAINT).await;
    assert_answers(other, StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}
