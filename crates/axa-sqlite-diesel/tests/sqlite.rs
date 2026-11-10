//! Behaviour against a real SQLite. Each test opens a database file of its own
//! under Cargo's temporary directory, so these run with plain `cargo test`.

use std::{path::Path, time::Duration};

use axa_sqlite_diesel::{
    Error, Pool, PoolConfig, connect, dangerously_recreate,
    diesel::{
        self, QueryableByName,
        result::{DatabaseErrorKind, Error as QueryError},
        sql_types::{BigInt, Text},
    },
    diesel_async::RunQueryDsl,
    diesel_migrations::FileBasedMigrations,
    is_unique_violation, migrate, pending, ping, revert, truncate,
};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[derive(QueryableByName)]
struct Mode {
    #[diesel(sql_type = Text)]
    journal_mode: String,
}

/// A URL for a fresh database file named after the test.
fn url(name: &str) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("axa-sqlite-diesel");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    format!("sqlite://{}", path.display())
}

async fn pool(name: &str) -> Pool {
    connect(&PoolConfig::new(url(name))).await.expect("connect")
}

/// A pool of one connection, so each call reuses the connection before it.
async fn pool_of_one(name: &str) -> Pool {
    let config = PoolConfig {
        max_connections: 1,
        ..PoolConfig::new(url(name))
    };
    connect(&config).await.expect("connect")
}

fn migrations() -> FileBasedMigrations {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/migrations");
    FileBasedMigrations::from_path(path).expect("load migrations")
}

async fn count(pool: &Pool, sql: &str) -> i64 {
    let mut connection = pool.get().await.unwrap();
    let row: Count = diesel::sql_query(sql)
        .get_result(&mut connection)
        .await
        .unwrap();
    row.n
}

async fn exists(pool: &Pool, name: &str) -> bool {
    let sql = format!("SELECT count(*) AS n FROM sqlite_schema WHERE name = '{name}'");
    count(pool, &sql).await == 1
}

async fn run(pool: &Pool, sql: &str) -> Result<usize, QueryError> {
    let mut connection = pool.get().await.unwrap();
    diesel::sql_query(sql).execute(&mut connection).await
}

async fn insert_item(pool: &Pool, name: &str) -> Result<usize, QueryError> {
    run(pool, &format!("INSERT INTO items (name) VALUES ('{name}')")).await
}

#[tokio::test]
async fn connect_and_ping_open_a_file() {
    let pool = pool("connect_and_ping_open_a_file").await;
    ping(&pool).await.unwrap();
}

#[tokio::test]
async fn every_connection_has_foreign_keys_wal_and_a_busy_timeout() {
    let config = PoolConfig {
        busy_timeout: Duration::from_millis(1234),
        ..PoolConfig::new(url(
            "every_connection_has_foreign_keys_wal_and_a_busy_timeout",
        ))
    };
    let pool = connect(&config).await.unwrap();
    // Hold two at once, so the second is a new connection.
    let mut one = pool.get().await.unwrap();
    let mut two = pool.get().await.unwrap();
    for connection in [&mut one, &mut two] {
        let keys: Count = diesel::sql_query("SELECT foreign_keys AS n FROM pragma_foreign_keys")
            .get_result(connection)
            .await
            .unwrap();
        assert_eq!(keys.n, 1);
        let busy: Count = diesel::sql_query("SELECT timeout AS n FROM pragma_busy_timeout")
            .get_result(connection)
            .await
            .unwrap();
        assert_eq!(busy.n, 1234);
        let mode: Mode = diesel::sql_query("PRAGMA journal_mode")
            .get_result(connection)
            .await
            .unwrap();
        assert_eq!(mode.journal_mode, "wal");
    }
}

#[tokio::test]
async fn an_in_memory_database_is_shared_by_its_pool_and_no_other() {
    let pool = connect(&PoolConfig::new("sqlite::memory:")).await.unwrap();
    let mut held = pool.get().await.unwrap();
    // This runs on a second connection, as the first is held.
    migrate(&pool, migrations()).await.unwrap();
    // The first sees what the second made.
    let tables: Count =
        diesel::sql_query("SELECT count(*) AS n FROM sqlite_schema WHERE name = 'items'")
            .get_result(&mut held)
            .await
            .unwrap();
    assert_eq!(tables.n, 1);
    drop(held);
    let keys = count(&pool, "SELECT foreign_keys AS n FROM pragma_foreign_keys").await;
    assert_eq!(keys, 1);

    let stranger = connect(&PoolConfig::new("sqlite::memory:")).await.unwrap();
    assert!(!exists(&stranger, "items").await);
}

#[tokio::test]
async fn migrate_and_pending_name_what_is_left() {
    let pool = pool("migrate_and_pending_name_what_is_left").await;
    assert_eq!(
        pending(&pool, migrations()).await.unwrap(),
        vec!["00000000000001_create_items", "00000000000002_create_notes"]
    );
    assert_eq!(migrate(&pool, migrations()).await.unwrap(), 2);
    assert!(exists(&pool, "items").await);
    assert!(exists(&pool, "notes").await);
    assert_eq!(
        pending(&pool, migrations()).await.unwrap(),
        Vec::<String>::new()
    );
    // Migrating again changes nothing.
    assert_eq!(migrate(&pool, migrations()).await.unwrap(), 0);
}

#[tokio::test]
async fn revert_undoes_the_newest_steps_and_stops_at_the_start() {
    let pool = pool("revert_undoes_the_newest_steps_and_stops_at_the_start").await;
    migrate(&pool, migrations()).await.unwrap();

    assert_eq!(revert(&pool, migrations(), 1).await.unwrap(), 1);
    assert!(exists(&pool, "items").await);
    assert!(!exists(&pool, "notes").await);
    assert_eq!(
        pending(&pool, migrations()).await.unwrap(),
        vec!["00000000000002_create_notes"]
    );

    assert_eq!(revert(&pool, migrations(), 5).await.unwrap(), 1);
    assert!(!exists(&pool, "items").await);
    assert_eq!(migrate(&pool, migrations()).await.unwrap(), 2);
}

#[tokio::test]
async fn a_duplicate_is_a_unique_violation() {
    let pool = pool("a_duplicate_is_a_unique_violation").await;
    migrate(&pool, migrations()).await.unwrap();
    insert_item(&pool, "one").await.unwrap();
    let error = insert_item(&pool, "one").await.unwrap_err();
    assert!(is_unique_violation(&error), "{error:?}");
    // A different constraint is not.
    let error = run(
        &pool,
        "INSERT INTO items (name, state) VALUES ('two', 'lost')",
    )
    .await
    .unwrap_err();
    assert!(!is_unique_violation(&error), "{error:?}");
}

#[tokio::test]
async fn foreign_keys_are_enforced() {
    let pool = pool("foreign_keys_are_enforced").await;
    migrate(&pool, migrations()).await.unwrap();
    let error = run(
        &pool,
        "INSERT INTO notes (item_id, body) VALUES (99, 'orphan')",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            error,
            QueryError::DatabaseError(DatabaseErrorKind::ForeignKeyViolation, _)
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn truncate_empties_tables_and_resets_their_counters() {
    let pool = pool("truncate_empties_tables_and_resets_their_counters").await;
    migrate(&pool, migrations()).await.unwrap();
    insert_item(&pool, "one").await.unwrap();
    insert_item(&pool, "two").await.unwrap();
    run(
        &pool,
        "INSERT INTO notes (item_id, body) VALUES (1, 'hello')",
    )
    .await
    .unwrap();

    // The parent may come first, as foreign keys are checked at the end.
    truncate(&pool, &["items", "notes"]).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM notes").await, 0);

    // The AUTOINCREMENT counter starts again.
    insert_item(&pool, "three").await.unwrap();
    assert_eq!(count(&pool, "SELECT id AS n FROM items").await, 1);

    // No tables is not an error.
    truncate::<&str>(&pool, &[]).await.unwrap();
}

#[tokio::test]
async fn truncate_refuses_to_leave_a_reference_dangling() {
    let pool = pool_of_one("truncate_refuses_to_leave_a_reference_dangling").await;
    migrate(&pool, migrations()).await.unwrap();
    insert_item(&pool, "one").await.unwrap();
    run(
        &pool,
        "INSERT INTO notes (item_id, body) VALUES (1, 'hello')",
    )
    .await
    .unwrap();

    assert!(truncate(&pool, &["items"]).await.is_err());
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 1);
    // The connection is usable afterwards, with no transaction left open.
    truncate(&pool, &["notes", "items"]).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);
}

#[tokio::test]
async fn truncate_refuses_an_unsafe_name_before_deleting_anything() {
    let pool = pool("truncate_refuses_an_unsafe_name_before_deleting_anything").await;
    migrate(&pool, migrations()).await.unwrap();
    insert_item(&pool, "one").await.unwrap();
    assert!(matches!(
        truncate(&pool, &["items", "items; DROP TABLE items"]).await,
        Err(Error::InvalidIdentifier(_))
    ));
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 1);
}

#[tokio::test]
async fn dangerously_recreate_rebuilds_from_the_first_migration() {
    let pool = pool_of_one("dangerously_recreate_rebuilds_from_the_first_migration").await;
    // Something no migration made, and data, both go.
    run(&pool, "CREATE TABLE stray (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    migrate(&pool, migrations()).await.unwrap();
    insert_item(&pool, "one").await.unwrap();
    run(
        &pool,
        "INSERT INTO notes (item_id, body) VALUES (1, 'hello')",
    )
    .await
    .unwrap();

    dangerously_recreate(&pool, migrations()).await.unwrap();
    assert!(!exists(&pool, "stray").await);
    assert!(exists(&pool, "items").await);
    assert!(exists(&pool, "notes").await);
    assert!(exists(&pool, "item_notes").await);
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);
    assert_eq!(
        pending(&pool, migrations()).await.unwrap(),
        Vec::<String>::new()
    );
    // Foreign keys are back on for the connection that did it, the pool's only one.
    let keys = count(&pool, "SELECT foreign_keys AS n FROM pragma_foreign_keys").await;
    assert_eq!(keys, 1);

    // It works a second time.
    dangerously_recreate(&pool, migrations()).await.unwrap();
    assert!(exists(&pool, "items").await);
}

#[tokio::test]
async fn dangerously_recreate_refuses_a_name_it_cannot_quote_and_drops_nothing() {
    let pool =
        pool_of_one("dangerously_recreate_refuses_a_name_it_cannot_quote_and_drops_nothing").await;
    migrate(&pool, migrations()).await.unwrap();
    run(&pool, "CREATE TABLE \"Odd\" (id INTEGER)")
        .await
        .unwrap();
    assert!(matches!(
        dangerously_recreate(&pool, migrations()).await,
        Err(Error::InvalidIdentifier(name)) if name == "Odd"
    ));
    assert!(exists(&pool, "items").await);
    let keys = count(&pool, "SELECT foreign_keys AS n FROM pragma_foreign_keys").await;
    assert_eq!(keys, 1);
}

#[tokio::test]
async fn a_pool_that_is_all_in_use_answers_with_an_error_in_time() {
    let config = PoolConfig {
        max_connections: 1,
        wait_timeout: Duration::from_millis(100),
        ..PoolConfig::new(url(
            "a_pool_that_is_all_in_use_answers_with_an_error_in_time",
        ))
    };
    let pool = connect(&config).await.unwrap();
    let held = pool.get().await.unwrap();
    let waited = tokio::time::timeout(Duration::from_secs(2), pool.get()).await;
    assert!(waited.expect("the pool's own limit comes first").is_err());
    drop(held);
    assert!(pool.get().await.is_ok());
}

#[tokio::test]
async fn a_file_that_cannot_be_opened_is_an_error() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("axa-sqlite-diesel-no-such-dir");
    let uri = format!("sqlite://{}/app.db", dir.display());
    assert!(matches!(
        connect(&PoolConfig::new(uri)).await,
        Err(Error::Pool(_))
    ));
}

#[tokio::test]
async fn a_url_that_is_not_sqlite_is_refused() {
    for uri in ["postgres://localhost/app", "app.db", "sqlite:"] {
        assert!(
            matches!(connect(&PoolConfig::new(uri)).await, Err(Error::InvalidUri)),
            "{uri}"
        );
    }
}
