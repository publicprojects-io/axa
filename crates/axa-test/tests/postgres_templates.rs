//! `axa_test::postgres::drop_templates`, against a real Postgres. It is a binary of its
//! own, as it drops the templates the tests in `postgres.rs` clone, and cargo runs one
//! test binary at a time:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test -p axa-test --features postgres-sqlx -- --ignored
//! ```
#![cfg(feature = "postgres-sqlx")]

use std::path::Path;

use axa_postgres_sqlx::sqlx::{
    self, Connection, PgConnection, Row, migrate::Migrator, postgres::PgConnectOptions,
};
use axa_test::postgres::{TestDatabase, URL_VAR, drop_templates, template_name};

async fn migrator(name: &str) -> Migrator {
    Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/migrations")
            .join(name),
    )
    .await
    .unwrap()
}

fn server() -> PgConnectOptions {
    std::env::var(URL_VAR).expect(URL_VAR).parse().unwrap()
}

async fn exists(name: &str) -> bool {
    let mut connection = PgConnection::connect_with(&server()).await.unwrap();
    sqlx::query("SELECT EXISTS (SELECT FROM pg_database WHERE datname = $1) AS found")
        .bind(name)
        .fetch_one(&mut connection)
        .await
        .unwrap()
        .get("found")
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn changed_migrations_get_another_template_and_old_ones_can_be_dropped() {
    let (one, two) = (migrator("one").await, migrator("two").await);
    assert_ne!(template_name(&one), template_name(&two));
    TestDatabase::new(&one).await.unwrap().close().await;
    TestDatabase::new(&two).await.unwrap().close().await;
    assert!(drop_templates(&server(), &two).await.unwrap() >= 1);
    assert!(!exists(&template_name(&one)).await);
    assert!(exists(&template_name(&two)).await);
}
