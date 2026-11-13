//! Whether production refuses a role that row-level security cannot hold, against a
//! real Postgres. Ignored by default. Run with a superuser's URL, on a database this
//! test may change:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test --features postgres-sqlx --test row_security -- --ignored
//! ```

#![cfg(axa_db = "postgres_sqlx")]

use axa::{Environment, db::require_row_security, postgres_sqlx::sqlx};

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL as a superuser"]
async fn production_refuses_what_row_level_security_cannot_hold_and_development_warns() {
    let url = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    for statement in [
        "DROP TABLE IF EXISTS axa_rls_guarded",
        "CREATE TABLE axa_rls_guarded (tenant_id TEXT NOT NULL)",
        "ALTER TABLE axa_rls_guarded ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE axa_rls_guarded FORCE ROW LEVEL SECURITY",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }

    // A superuser skips every policy.
    let error = require_row_security(&pool, &["axa_rls_guarded"], &Environment::Production)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("superuser or has BYPASSRLS"), "{error}");
    require_row_security(&pool, &["axa_rls_guarded"], &Environment::Development)
        .await
        .unwrap();

    // A table that is missing is never protected, whoever asks.
    let error = require_row_security(&pool, &["axa_rls_missing"], &Environment::Production)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("axa_rls_missing"), "{error}");

    sqlx::query("DROP TABLE axa_rls_guarded")
        .execute(&pool)
        .await
        .unwrap();
}
