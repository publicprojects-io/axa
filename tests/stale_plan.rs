#![allow(
    clippy::expect_used,
    reason = "RCA-SCOPE-4: tests may unwrap, expect and index"
)]
//! What a stale plan answers, against a real Postgres, through each library that
//! reports it with its code. Ignored by default:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test --all-features --test stale_plan -- --ignored --test-threads=1
//! ```

#![cfg(any(
    feature = "postgres-sqlx",
    feature = "postgres-seaorm",
    feature = "postgres-toasty"
))]

use axa::Error;
use axum::{http::StatusCode, response::IntoResponse};

fn url() -> String {
    std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL")
}

/// The answer to a stale plan: a `503` to try again.
fn assert_try_again(error: Error) {
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.headers().contains_key("retry-after"));
}

#[cfg(feature = "postgres-sqlx")]
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_stale_plan_is_a_503_to_try_again() {
    use axa::postgres_sqlx::{PoolConfig, connect, sqlx};
    let mut config = PoolConfig::new(url());
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

    assert_try_again(error);
    // And the next attempt, on a fresh connection, works.
    sqlx::query(select).fetch_all(&pool).await.unwrap();
    sqlx::raw_sql("DROP TABLE axa_stale_plan")
        .execute(&pool)
        .await
        .unwrap();
}

#[cfg(feature = "postgres-seaorm")]
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_stale_plan_through_seaorm_is_a_503_to_try_again() {
    use axa::postgres_seaorm::{
        PoolConfig, connect,
        sea_orm::{ConnectionTrait, DbBackend, Statement},
    };
    let mut config = PoolConfig::new(url());
    config.min_connections = 0;
    config.max_connections = 1;
    let db = connect(&config).await.unwrap();
    for statement in [
        "DROP TABLE IF EXISTS axa_stale_plan_seaorm",
        "CREATE TABLE axa_stale_plan_seaorm (x INT)",
    ] {
        db.execute_unprepared(statement).await.unwrap();
    }
    let select = || {
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT * FROM axa_stale_plan_seaorm WHERE $1",
            [true.into()],
        )
    };
    db.query_all_raw(select()).await.unwrap();
    db.execute_unprepared("ALTER TABLE axa_stale_plan_seaorm ALTER COLUMN x TYPE TEXT")
        .await
        .unwrap();
    let error: Error = db.query_all_raw(select()).await.unwrap_err().into();

    assert_try_again(error);
    db.query_all_raw(select()).await.unwrap();
    db.execute_unprepared("DROP TABLE axa_stale_plan_seaorm")
        .await
        .unwrap();
}

#[cfg(feature = "postgres-toasty")]
#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_stale_plan_through_toasty_is_a_503_to_try_again() {
    use axa::postgres_toasty::{PoolConfig, connect, toasty};
    let mut config = PoolConfig::new(url());
    config.max_connections = 1;
    let mut db = connect(&config, toasty::ModelSet::default()).await.unwrap();
    for statement in [
        "DROP TABLE IF EXISTS axa_stale_plan_toasty",
        "CREATE TABLE axa_stale_plan_toasty (x VARCHAR(10))",
    ] {
        toasty::sql::statement(statement)
            .exec(&mut db)
            .await
            .unwrap();
    }
    let select = "SELECT x FROM axa_stale_plan_toasty";
    toasty::sql::query(select).exec(&mut db).await.unwrap();
    toasty::sql::statement("ALTER TABLE axa_stale_plan_toasty ALTER COLUMN x TYPE TEXT")
        .exec(&mut db)
        .await
        .unwrap();
    let error: Error = toasty::sql::query(select)
        .exec(&mut db)
        .await
        .unwrap_err()
        .into();

    assert_try_again(error);
    toasty::sql::query(select).exec(&mut db).await.unwrap();
    toasty::sql::statement("DROP TABLE axa_stale_plan_toasty")
        .exec(&mut db)
        .await
        .unwrap();
}
