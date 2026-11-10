//! Behaviour against a real SQLite. Each test has a database file of its own
//! under Cargo's target directory, so these run in a plain `cargo test`.

use std::path::{Path, PathBuf};

use axa_sqlite_sqlx::{
    Db, PoolConfig, connect, dangerously_recreate, is_unique_violation, migrate, pending, ping,
    revert,
    sqlx::{self, Row, SqlitePool, migrate::Migrator},
    status, truncate,
};

/// A path for a new database file, with any left from a previous run removed.
fn fresh_path(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("axa-sqlite-sqlx");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

/// A pool on a new database file of this test's own.
async fn pool(name: &str) -> SqlitePool {
    let uri = format!("sqlite://{}?mode=rwc", fresh_path(name).display());
    connect(&PoolConfig::new(uri)).await.expect("connect")
}

async fn migrator() -> Migrator {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/migrations");
    Migrator::new(path).await.expect("load migrations")
}

async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count(pool: &SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_item(pool: &SqlitePool, name: &str) -> Result<i64, sqlx::Error> {
    Ok(
        sqlx::query("INSERT INTO items (name) VALUES (?1) RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await?
            .get("id"),
    )
}

async fn insert_note(pool: &SqlitePool, item_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO notes (item_id, body) VALUES (?1, 'note')")
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn every_connection_has_foreign_keys_wal_and_a_busy_timeout() {
    let pool = pool("pragmas").await;
    ping(&pool).await.unwrap();
    // Hold several at once, so that each is a different connection.
    let mut held = Vec::new();
    for _ in 0..3 {
        let mut conn = pool.acquire().await.unwrap();
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(foreign_keys, 1);
        assert_eq!(journal, "wal");
        assert_eq!(busy, 5_000);
        held.push(conn);
    }
}

#[tokio::test]
async fn a_missing_file_is_not_created_unless_asked() {
    let path = fresh_path("missing");
    let uri = format!("sqlite://{}", path.display());
    assert!(connect(&PoolConfig::new(uri)).await.is_err());
    assert!(!path.exists());
}

#[tokio::test]
async fn an_in_memory_pool_is_one_database_with_foreign_keys_on() {
    let pool = connect(&PoolConfig::new("sqlite::memory:")).await.unwrap();
    ping(&pool).await.unwrap();
    migrate(&pool, &migrator().await).await.unwrap();
    let mut first = pool.acquire().await.unwrap();
    let mut second = pool.acquire().await.unwrap();
    for conn in [&mut first, &mut second] {
        let found: i64 = sqlx::query_scalar("SELECT count(*) FROM items")
            .fetch_one(&mut **conn)
            .await
            .unwrap();
        assert_eq!(found, 0);
        let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut **conn)
            .await
            .unwrap();
        assert_eq!(journal, "memory");
    }
    drop((first, second));
    let error = insert_note(&pool, 99).await.unwrap_err();
    assert!(!is_unique_violation(&error));
}

#[tokio::test]
async fn migrations_apply_revert_and_list_what_is_pending() {
    let pool = pool("migrations").await;
    let migrator = migrator().await;

    // A database never migrated has everything pending.
    assert_eq!(
        pending(&pool, &migrator).await.unwrap(),
        ["1_create items", "2_create notes"]
    );
    assert!(
        status(&pool, &migrator)
            .await
            .unwrap()
            .iter()
            .all(|row| !row.applied)
    );

    migrate(&pool, &migrator).await.unwrap();
    assert!(table_exists(&pool, "items").await);
    assert!(table_exists(&pool, "notes").await);
    assert_eq!(
        pending(&pool, &migrator).await.unwrap(),
        Vec::<String>::new()
    );
    let rows = status(&pool, &migrator).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].description, "create items");

    // Migrating again changes nothing.
    migrate(&pool, &migrator).await.unwrap();

    // Reverting one step removes only the newest migration.
    revert(&pool, &migrator, 1).await.unwrap();
    assert!(table_exists(&pool, "items").await);
    assert!(!table_exists(&pool, "notes").await);
    assert_eq!(pending(&pool, &migrator).await.unwrap(), ["2_create notes"]);

    // Reverting past the start reverts everything, and migrating restores it.
    revert(&pool, &migrator, 5).await.unwrap();
    assert!(!table_exists(&pool, "items").await);
    migrate(&pool, &migrator).await.unwrap();
    assert!(table_exists(&pool, "notes").await);
}

#[tokio::test]
async fn a_duplicate_is_a_unique_violation_and_other_errors_are_not() {
    let pool = pool("unique").await;
    migrate(&pool, &migrator().await).await.unwrap();
    insert_item(&pool, "one").await.unwrap();
    let error = insert_item(&pool, "one").await.unwrap_err();
    assert!(is_unique_violation(&error));

    let error = sqlx::query("INSERT INTO items (id, name) VALUES (1, 'other')")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(is_unique_violation(&error), "a duplicate primary key");

    let error = insert_note(&pool, 99).await.unwrap_err();
    assert!(!is_unique_violation(&error), "a foreign key violation");
    let error = sqlx::query("SELECT * FROM missing_table")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(!is_unique_violation(&error));
}

#[tokio::test]
async fn truncate_empties_tables_and_restarts_their_counters() {
    let pool = pool("truncate").await;
    migrate(&pool, &migrator().await).await.unwrap();
    insert_item(&pool, "one").await.unwrap();
    insert_item(&pool, "two").await.unwrap();

    truncate(&pool, &["items"]).await.unwrap();
    assert_eq!(count(&pool, "items").await, 0);
    assert_eq!(insert_item(&pool, "three").await.unwrap(), 1);

    // No tables is not an error.
    truncate::<&str>(&pool, &[]).await.unwrap();
}

#[tokio::test]
async fn truncate_refuses_an_unsafe_name_before_emptying_anything() {
    let pool = pool("truncate_unsafe").await;
    migrate(&pool, &migrator().await).await.unwrap();
    insert_item(&pool, "one").await.unwrap();

    assert!(
        truncate(&pool, &["items", "items; DROP TABLE items"])
            .await
            .is_err()
    );
    assert!(table_exists(&pool, "items").await);
    assert_eq!(count(&pool, "items").await, 1);
}

#[tokio::test]
async fn truncate_keeps_foreign_keys_whatever_the_order() {
    let pool = pool("truncate_references").await;
    migrate(&pool, &migrator().await).await.unwrap();
    let id = insert_item(&pool, "one").await.unwrap();
    insert_note(&pool, id).await.unwrap();

    // Emptying a table still referenced from one not listed changes nothing.
    assert!(truncate(&pool, &["items"]).await.is_err());
    assert_eq!(count(&pool, "items").await, 1);
    assert_eq!(count(&pool, "notes").await, 1);

    // Listing both works in either order.
    truncate(&pool, &["items", "notes"]).await.unwrap();
    assert_eq!(count(&pool, "items").await, 0);
    assert_eq!(count(&pool, "notes").await, 0);
    let id = insert_item(&pool, "two").await.unwrap();
    insert_note(&pool, id).await.unwrap();
    truncate(&pool, &["notes", "items"]).await.unwrap();
    assert_eq!(count(&pool, "items").await, 0);
}

#[tokio::test]
async fn dangerously_recreate_rebuilds_from_the_first_migration() {
    // One connection, so the checks below see the one that dropped the tables.
    let uri = format!("sqlite://{}?mode=rwc", fresh_path("recreate").display());
    let pool = connect(&PoolConfig {
        max_connections: 1,
        ..PoolConfig::new(uri)
    })
    .await
    .unwrap();
    let migrator = migrator().await;

    // From a database never migrated.
    dangerously_recreate(&pool, &migrator).await.unwrap();
    assert_eq!(
        pending(&pool, &migrator).await.unwrap(),
        Vec::<String>::new()
    );

    // Data, a table no migration made, a view, and a reference between tables.
    let id = insert_item(&pool, "one").await.unwrap();
    insert_note(&pool, id).await.unwrap();
    for sql in [
        "CREATE TABLE stray (id INTEGER PRIMARY KEY)",
        "CREATE VIEW item_names AS SELECT name FROM items",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    // With the newest migration reverted, recreate still applies it again.
    revert(&pool, &migrator, 1).await.unwrap();

    dangerously_recreate(&pool, &migrator).await.unwrap();
    assert!(!table_exists(&pool, "stray").await);
    let views: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_schema WHERE type = 'view'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(views, 0);
    assert_eq!(count(&pool, "items").await, 0);
    assert_eq!(count(&pool, "notes").await, 0);
    assert!(
        status(&pool, &migrator)
            .await
            .unwrap()
            .iter()
            .all(|row| row.applied)
    );
    assert_eq!(insert_item(&pool, "two").await.unwrap(), 1);

    // The connection it used checks foreign keys again.
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(foreign_keys, 1);
    assert!(insert_note(&pool, 99).await.is_err());
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
async fn one_function_serves_the_pool_and_a_transaction() {
    let pool = pool("db").await;
    migrate(&pool, &migrator().await).await.unwrap();

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
