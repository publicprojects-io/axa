//! Behaviour against a real MySQL 8.4. Ignored by default, because these drop
//! every table in the database they are given. The tests share one database,
//! so run them one at a time:
//!
//! ```sh
//! mysql -e 'CREATE DATABASE IF NOT EXISTS axa_mysql_sqlx'
//! AXA_TEST_MYSQL_URL=mysql://.../axa_mysql_sqlx cargo test -p axa-mysql-sqlx -- --ignored --test-threads=1
//! ```

use std::path::{Path, PathBuf};

use axa_mysql_sqlx::{
    Db, PoolConfig, connect, dangerously_recreate, is_unique_violation, migrate, pending, ping,
    revert,
    sqlx::{
        self, MySqlPool, Row,
        migrate::{MigrateError, Migrator},
    },
    status, truncate,
};

fn uri() -> String {
    std::env::var("AXA_TEST_MYSQL_URL").expect("AXA_TEST_MYSQL_URL")
}

async fn pool() -> MySqlPool {
    connect(&PoolConfig::new(uri())).await.expect("connect")
}

async fn migrator() -> Migrator {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/migrations");
    Migrator::new(path).await.expect("load migrations")
}

async fn table_exists(pool: &MySqlPool, name: &str) -> bool {
    let found: i64 = sqlx::query(
        "SELECT count(*) AS found FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
    .get("found");
    found == 1
}

async fn count(pool: &MySqlPool, table: &str) -> i64 {
    let sql = format!("SELECT count(*) AS n FROM {table}");
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_one(pool)
        .await
        .unwrap()
        .get("n")
}

async fn insert_item(pool: &MySqlPool, name: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("INSERT INTO items (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await?
        .last_insert_id())
}

async fn insert_note(pool: &MySqlPool, item_id: u64) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO notes (item_id, body) VALUES (?, 'n')")
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn pooling_and_migrations_against_mysql() {
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

    // Reverting one step removes only the newest migration.
    revert(&pool, &migrator, 1).await.unwrap();
    assert!(table_exists(&pool, "items").await);
    assert!(!table_exists(&pool, "notes").await);
    let rows = status(&pool, &migrator).await.unwrap();
    assert!(rows[0].applied);
    assert!(!rows[1].applied);
    assert_eq!(pending(&pool, &migrator).await.unwrap(), ["2_create notes"]);

    // Reverting past the start reverts everything, and migrating restores it.
    revert(&pool, &migrator, 5).await.unwrap();
    assert!(!table_exists(&pool, "items").await);
    migrate(&pool, &migrator).await.unwrap();
    assert!(table_exists(&pool, "notes").await);

    // Recreating clears data, then works a second time.
    insert_item(&pool, "three").await.unwrap();
    dangerously_recreate(&pool, &migrator).await.unwrap();
    assert_eq!(count(&pool, "items").await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn truncating_empties_referenced_tables_and_restores_foreign_key_checks() {
    // One connection, so the check below sees the one truncate used.
    let config = PoolConfig {
        min_connections: 1,
        max_connections: 1,
        ..PoolConfig::new(uri())
    };
    let pool = connect(&config).await.unwrap();
    let migrator = migrator().await;
    dangerously_recreate(&pool, &migrator).await.unwrap();

    let item = insert_item(&pool, "one").await.unwrap();
    insert_note(&pool, item).await.unwrap();

    // `items` is referenced by `notes`, and still empties, with its counter
    // restarted.
    truncate(&pool, &["notes", "items"]).await.unwrap();
    assert_eq!(count(&pool, "items").await, 0);
    assert_eq!(count(&pool, "notes").await, 0);
    assert_eq!(insert_item(&pool, "two").await.unwrap(), 1);

    // Foreign key checks are on again afterwards.
    let checks: i64 = sqlx::query("SELECT @@SESSION.foreign_key_checks AS n")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(checks, 1);
    assert!(insert_note(&pool, 999).await.is_err());

    // A bad name is refused before any SQL runs, and leaves the data alone,
    // even when it comes after a good one.
    assert!(
        truncate(&pool, &["items", "items; DROP TABLE items"])
            .await
            .is_err()
    );
    assert_eq!(count(&pool, "items").await, 1);

    // No tables is not an error.
    truncate::<&str>(&pool, &[]).await.unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn recreating_drops_views_and_tables_outside_the_migrations() {
    let pool = pool().await;
    let migrator = migrator().await;
    dangerously_recreate(&pool, &migrator).await.unwrap();
    for sql in [
        "CREATE TABLE stray (id BIGINT NOT NULL PRIMARY KEY, item_id BIGINT NOT NULL, FOREIGN KEY (item_id) REFERENCES items (id))",
        "CREATE VIEW item_names AS SELECT name FROM items",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    dangerously_recreate(&pool, &migrator).await.unwrap();
    assert!(!table_exists(&pool, "stray").await);
    assert!(!table_exists(&pool, "item_names").await);
    assert!(table_exists(&pool, "notes").await);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn recreating_without_a_database_is_refused() {
    let uri = uri();
    let base = uri.split('?').next().unwrap();
    let (server, _) = base.rsplit_once('/').unwrap();
    let pool = connect(&PoolConfig::new(format!("{server}/")))
        .await
        .unwrap();
    let migrator = migrator().await;
    assert!(dangerously_recreate(&pool, &migrator).await.is_err());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
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
#[ignore = "needs AXA_TEST_MYSQL_URL"]
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
    assert_eq!(
        pending(&pool, &migrator).await.unwrap(),
        ["1_create items", "2_create notes"]
    );
    dangerously_recreate(&pool, &migrator).await.unwrap();
    assert_eq!(
        pending(&pool, &migrator).await.unwrap(),
        Vec::<String>::new()
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_migration_that_fails_after_ddl_is_left_dirty() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("axa-mysql-sqlx-dirty");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("0001_half.sql"),
        "CREATE TABLE half (id BIGINT NOT NULL PRIMARY KEY);\nINSERT INTO missing_table VALUES (1);\n",
    )
    .unwrap();
    let broken = Migrator::new(dir.as_path()).await.unwrap();
    let pool = pool().await;
    let migrator = migrator().await;
    dangerously_recreate(&pool, &migrator).await.unwrap();
    sqlx::query("DROP TABLE items, notes, _sqlx_migrations")
        .execute(&pool)
        .await
        .unwrap();

    assert!(migrate(&pool, &broken).await.is_err());
    // The table the migration made before failing was committed.
    assert!(table_exists(&pool, "half").await);
    assert_eq!(pending(&pool, &broken).await.unwrap(), ["1_half"]);
    assert!(matches!(
        migrate(&pool, &broken).await,
        Err(MigrateError::Dirty(1))
    ));

    // Recreating clears it.
    dangerously_recreate(&pool, &migrator).await.unwrap();
    assert!(!table_exists(&pool, "half").await);
    assert_eq!(
        pending(&pool, &migrator).await.unwrap(),
        Vec::<String>::new()
    );
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
#[ignore = "needs AXA_TEST_MYSQL_URL"]
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
