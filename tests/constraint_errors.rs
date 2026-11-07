//! What a violated constraint answers, against a real Postgres, because a driver only
//! reports a violation from a real database error.

#![cfg(any(feature = "queue", feature = "sqlx", feature = "sea-orm"))]

use axa::Error;
use axum::{
    body::to_bytes,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

const SCHEMA: [&str; 4] = [
    "DROP TABLE IF EXISTS axa_constraint_child",
    "DROP TABLE IF EXISTS axa_constraint_parent",
    "CREATE TABLE axa_constraint_parent (id INT PRIMARY KEY, email TEXT UNIQUE)",
    "CREATE TABLE axa_constraint_child (id INT PRIMARY KEY, \
     parent INT NOT NULL REFERENCES axa_constraint_parent (id))",
];
const INSERT_PARENT: &str = "INSERT INTO axa_constraint_parent VALUES (1, 'carol@example.org')";
const DUPLICATE: &str = "INSERT INTO axa_constraint_parent VALUES (2, 'carol@example.org')";
const NO_PARENT: &str = "INSERT INTO axa_constraint_child VALUES (1, 99)";
const NOT_A_CONSTRAINT: &str = "SELECT * FROM axa_constraint_no_such_table";

fn url() -> String {
    std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL")
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
}

#[cfg(any(feature = "queue", feature = "sqlx"))]
#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_violated_constraint_through_sqlx_is_a_conflict() {
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
    let other = fails(NOT_A_CONSTRAINT).await;
    assert_answers(other, StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}

#[cfg(feature = "sea-orm")]
#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL"]
async fn a_violated_constraint_through_sea_orm_is_a_conflict() {
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
    let other = fails(NOT_A_CONSTRAINT).await;
    assert_answers(other, StatusCode::INTERNAL_SERVER_ERROR, "internal").await;

    // The model layer wraps the same error, and answers the same.
    let error = db.execute_unprepared(DUPLICATE).await.unwrap_err();
    let wrapped = Error::Model(axa::model::ModelError::DbErr(error)).into_response();
    assert_answers(wrapped, StatusCode::CONFLICT, "conflict").await;
}
