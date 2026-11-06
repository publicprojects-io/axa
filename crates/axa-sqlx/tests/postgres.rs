//! Behaviour against a real Postgres. Ignored by default, because these drop
//! every table in the database they are given. Run them with:
//!
//! ```sh
//! AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-sqlx -- --ignored --test-threads=1
//! ```

use std::path::Path;

use axa_sqlx::{
    Db, PoolConfig, connect, dangerously_recreate, is_unique_violation, migrate, ping, revert,
    sqlx::{self, PgPool, Row, migrate::Migrator},
    status, truncate,
};

async fn pool() -> PgPool {
    let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
    connect(&PoolConfig::new(uri)).await.expect("connect")
}

async fn migrator() -> Migrator {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/migrations");
    Migrator::new(path).await.expect("load migrations")
}

async fn table_exists(pool: &PgPool, name: &str) -> bool {
    sqlx::query("SELECT to_regclass($1) IS NOT NULL AS found")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
        .get("found")
}

async fn count_items(pool: &PgPool) -> i64 {
    sqlx::query("SELECT count(*) AS n FROM items")
        .fetch_one(pool)
        .await
        .unwrap()
        .get("n")
}

async fn insert_item(pool: &PgPool, name: &str) -> Result<i64, sqlx::Error> {
    Ok(
        sqlx::query("INSERT INTO items (name) VALUES ($1) RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await?
            .get("id"),
    )
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL and drops every table in it"]
async fn pooling_and_migrations_against_postgres() {
    let pool = pool().await;
    let migrator = migrator().await;
    ping(&pool).await.unwrap();

    // Recreating from anything leaves exactly the migrated schema.
    dangerously_recreate(&pool, &migrator).await.unwrap();
    assert!(table_exists(&pool, "items").await);
    assert!(table_exists(&pool, "notes").await);

    // Status lists both, oldest first, applied.
    let rows = status(&pool, &migrator).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].description, "create items");
    assert!(rows.iter().all(|row| row.applied));

    // Migrating again changes nothing.
    migrate(&pool, &migrator).await.unwrap();

    // A violated unique constraint is recognised, and other errors are not.
    insert_item(&pool, "one").await.unwrap();
    let error = insert_item(&pool, "one").await.unwrap_err();
    assert!(is_unique_violation(&error));
    let error = sqlx::query("SELECT * FROM missing_table")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(!is_unique_violation(&error));

    // Truncating empties the table and restarts its sequence, including when
    // another table references it.
    truncate(&pool, &["items"]).await.unwrap();
    assert_eq!(count_items(&pool).await, 0);
    assert_eq!(insert_item(&pool, "two").await.unwrap(), 1);

    // A bad name is refused before any SQL runs, and leaves the data alone.
    assert!(truncate(&pool, &["items; DROP TABLE items"]).await.is_err());
    assert_eq!(count_items(&pool).await, 1);

    // No tables is not an error.
    truncate::<&str>(&pool, &[]).await.unwrap();

    // Reverting one step removes only the newest migration.
    revert(&pool, &migrator, 1).await.unwrap();
    assert!(table_exists(&pool, "items").await);
    assert!(!table_exists(&pool, "notes").await);
    let rows = status(&pool, &migrator).await.unwrap();
    assert!(rows[0].applied);
    assert!(!rows[1].applied);

    // Reverting past the start reverts everything, and migrating restores it.
    revert(&pool, &migrator, 5).await.unwrap();
    assert!(!table_exists(&pool, "items").await);
    migrate(&pool, &migrator).await.unwrap();
    assert!(table_exists(&pool, "notes").await);

    // Recreating clears data and enum types, then works a second time.
    insert_item(&pool, "three").await.unwrap();
    dangerously_recreate(&pool, &migrator).await.unwrap();
    assert_eq!(count_items(&pool).await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL and drops every table in it"]
async fn recreating_keeps_extensions() {
    let pool = pool().await;
    let migrator = migrator().await;
    sqlx::query("CREATE EXTENSION IF NOT EXISTS citext")
        .execute(&pool)
        .await
        .unwrap();
    dangerously_recreate(&pool, &migrator).await.unwrap();
    let found: i64 = sqlx::query("SELECT count(*) AS n FROM pg_extension WHERE extname = 'citext'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(found, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL and drops every table in it"]
async fn services_booting_together_all_migrate() {
    let pool = pool().await;
    let migrator = migrator().await;
    dangerously_recreate(&pool, &migrator).await.unwrap();
    revert(&pool, &migrator, 5).await.unwrap();
    let boots: Vec<_> = (0..6)
        .map(|_| {
            tokio::spawn(async {
                let pool = self::pool().await;
                let migrator = self::migrator().await;
                migrate(&pool, &migrator).await
            })
        })
        .collect();
    for boot in boots {
        boot.await.unwrap().unwrap();
    }
    assert!(table_exists(&pool, "notes").await);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL and drops every table in it"]
async fn status_on_a_fresh_database_shows_nothing_applied() {
    let pool = pool().await;
    let migrator = migrator().await;
    dangerously_recreate(&pool, &migrator).await.unwrap();
    sqlx::query("DROP TABLE _sqlx_migrations")
        .execute(&pool)
        .await
        .unwrap();
    let rows = status(&pool, &migrator).await.unwrap();
    assert!(rows.iter().all(|row| !row.applied));
    dangerously_recreate(&pool, &migrator).await.unwrap();
}

/// One function, reached from the pool and from inside a transaction, in a
/// future that can be spawned.
async fn item_count(db: impl Into<Db<'_>>) -> Result<i64, sqlx::Error> {
    let mut conn = db.into().acquire().await?;
    sqlx::query_scalar("SELECT count(*) FROM items")
        .fetch_one(&mut *conn)
        .await
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DATABASE_URL and drops every table in it"]
async fn one_function_serves_the_pool_and_a_transaction() {
    let pool = pool().await;
    let migrator = migrator().await;
    dangerously_recreate(&pool, &migrator).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO items (name) VALUES ('held')")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(item_count(&mut *tx).await.unwrap(), 1);
    // The pool cannot see what the transaction has not committed.
    assert_eq!(item_count(&pool).await.unwrap(), 0);
    tx.rollback().await.unwrap();

    let spawned = tokio::spawn({
        let pool = pool.clone();
        async move { item_count(&pool).await }
    });
    assert_eq!(spawned.await.unwrap().unwrap(), 0);
}
