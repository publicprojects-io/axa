//! What a stale plan answers, against a real Postgres. Ignored by default:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test --features postgres-sqlx --test stale_plan -- --ignored
//! ```

#![cfg(axa_db = "postgres_sqlx")]

use axa::{
    Error,
    postgres_sqlx::{PoolConfig, connect, sqlx},
};
use axum::{http::StatusCode, response::IntoResponse};

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_stale_plan_is_a_503_to_try_again() {
    let uri = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let mut config = PoolConfig::new(uri);
    config.min_connections = 0;
    config.max_connections = 1;
    let pool = connect(&config).await.unwrap();
    for statement in [
        "DROP TABLE IF EXISTS axa_stale_plan",
        "CREATE TABLE axa_stale_plan (x INT)",
    ] {
        sqlx::raw_sql(statement).execute(&pool).await.unwrap();
    }
    let select = "SELECT * FROM axa_stale_plan";
    sqlx::query(select).fetch_all(&pool).await.unwrap();
    sqlx::raw_sql("ALTER TABLE axa_stale_plan ALTER COLUMN x TYPE TEXT")
        .execute(&pool)
        .await
        .unwrap();
    let error: Error = sqlx::query(select)
        .fetch_all(&pool)
        .await
        .unwrap_err()
        .into();

    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.headers().contains_key("retry-after"));
    // And the next attempt, on a fresh connection, works.
    sqlx::query(select).fetch_all(&pool).await.unwrap();
    sqlx::raw_sql("DROP TABLE axa_stale_plan")
        .execute(&pool)
        .await
        .unwrap();
}
