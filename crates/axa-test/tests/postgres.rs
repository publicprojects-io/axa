//! `axa_test::postgres`, against a real Postgres:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test -p axa-test --features postgres-sqlx -- --ignored
//! ```
//!
//! The role needs `CREATEDB`. These create and drop databases named `axa_test_`.
#![cfg(feature = "postgres-sqlx")]

use std::path::Path;

use axa_postgres_sqlx::sqlx::{
    self, Connection, PgConnection, Row, migrate::Migrator, postgres::PgConnectOptions,
};
use axa_test::postgres::{TestDatabase, URL_VAR, template_name};

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

/// Whether `name` is gone within ten seconds.
async fn goes(name: &str) -> bool {
    for _ in 0..100 {
        if !exists(name).await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

async fn count(db: &TestDatabase) -> i64 {
    sqlx::query("SELECT count(*) AS n FROM notes")
        .fetch_one(&db.pool)
        .await
        .unwrap()
        .get("n")
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn each_test_gets_a_migrated_database_of_its_own() {
    let migrator = migrator("one").await;
    let (first, second) = (
        TestDatabase::new(&migrator).await.unwrap(),
        TestDatabase::new(&migrator).await.unwrap(),
    );
    assert_ne!(first.name(), second.name());
    sqlx::query("INSERT INTO notes (body) VALUES ('mine')")
        .execute(&first.pool)
        .await
        .unwrap();
    assert_eq!((count(&first).await, count(&second).await), (1, 0));
    let applied: i64 = sqlx::query("SELECT count(*) AS n FROM _sqlx_migrations")
        .fetch_one(&second.pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(applied, 1);
    assert!(exists(&template_name(&migrator)).await);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn the_database_is_dropped_by_close_or_with_the_guard_even_on_a_panic() {
    let migrator = migrator("one").await;
    let db = TestDatabase::new(&migrator).await.unwrap();
    let name = db.name().to_string();
    assert!(exists(&name).await);
    db.close().await;
    assert!(!exists(&name).await);

    let db = TestDatabase::new(&migrator).await.unwrap();
    let name = db.name().to_string();
    drop(db);
    assert!(goes(&name).await, "{name}");

    let db = TestDatabase::new(&migrator).await.unwrap();
    let name = db.name().to_string();
    let panicked = std::thread::spawn(move || {
        let _db = db;
        panic!("a failing test");
    })
    .join();
    assert!(panicked.is_err());
    assert!(goes(&name).await, "{name}");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn many_tests_starting_together_share_one_template() {
    let migrator = std::sync::Arc::new(migrator("two").await);
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let migrator = migrator.clone();
            tokio::spawn(async move {
                let db = TestDatabase::new(&migrator).await.unwrap();
                sqlx::query("INSERT INTO notes (body, tags) VALUES ('a', '{x}')")
                    .execute(&db.pool)
                    .await
                    .unwrap();
                count(&db).await
            })
        })
        .collect();
    for task in tasks {
        assert_eq!(task.await.unwrap(), 1);
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_failed_migration_leaves_no_template() {
    let migrator = migrator("broken").await;
    let template = template_name(&migrator);
    assert!(TestDatabase::new(&migrator).await.is_err());
    assert!(!exists(&template).await);
    assert!(!exists(&format!("{template}_partial")).await);
}
