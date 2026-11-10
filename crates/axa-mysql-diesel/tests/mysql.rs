//! Behaviour against a real MySQL. Ignored by default, because these drop
//! every table in the database they are given. The database is created if it
//! is missing. Run them with:
//!
//! ```sh
//! AXA_TEST_MYSQL_URL=mysql://root:axa@127.0.0.1:53306/axa_mysql_diesel \
//!     cargo test -p axa-mysql-diesel -- --ignored --test-threads=1
//! ```

use std::{path::Path, time::Duration};

use axa_mysql_diesel::{
    Error, Pool, PoolConfig, connect, dangerously_recreate,
    diesel::{
        self, QueryableByName,
        result::{DatabaseErrorKind, Error as QueryError},
        sql_types::{BigInt, Text},
    },
    diesel_async::{RunQueryDsl, SimpleAsyncConnection},
    diesel_migrations::FileBasedMigrations,
    is_unique_violation, migrate, pending, ping, quote_identifier, revert, truncate,
};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[derive(QueryableByName)]
struct Value {
    #[diesel(sql_type = Text)]
    v: String,
}

/// The test URL, with `parameter` added to its query.
fn url_with(parameter: Option<&str>) -> String {
    let uri = std::env::var("AXA_TEST_MYSQL_URL").expect("AXA_TEST_MYSQL_URL");
    match parameter {
        None => uri,
        Some(parameter) if uri.contains('?') => format!("{uri}&{parameter}"),
        Some(parameter) => format!("{uri}?{parameter}"),
    }
}

/// Create the test URL's database if it is missing, through a connection to
/// the server with no database named.
async fn ensure_database() {
    let uri = url_with(None);
    let (path, query) = uri.split_once('?').unwrap_or((&uri, ""));
    let (server, database) = path.rsplit_once('/').expect("a database in the URL");
    let database = quote_identifier(database).expect("a plain database name");
    let server = if query.is_empty() {
        format!("{server}/")
    } else {
        format!("{server}/?{query}")
    };
    let pool = connect(&PoolConfig::new(server)).await.expect("connect");
    let mut connection = pool.get().await.unwrap();
    connection
        .batch_execute(&format!("CREATE DATABASE IF NOT EXISTS {database}"))
        .await
        .unwrap();
}

async fn pool_from(config: PoolConfig) -> Pool {
    ensure_database().await;
    connect(&config).await.expect("connect")
}

async fn pool() -> Pool {
    pool_from(PoolConfig::new(url_with(None))).await
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

async fn table_exists(pool: &Pool, name: &str) -> bool {
    let sql = format!(
        "SELECT count(*) AS n FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = '{name}'"
    );
    count(pool, &sql).await == 1
}

async fn execute(pool: &Pool, sql: &str) -> Result<usize, QueryError> {
    let mut connection = pool.get().await.unwrap();
    diesel::sql_query(sql).execute(&mut connection).await
}

async fn insert_item(pool: &Pool, name: &str) -> Result<usize, QueryError> {
    execute(pool, &format!("INSERT INTO items (name) VALUES ('{name}')")).await
}

/// The cipher of the pool's TLS session, empty for plain text.
async fn cipher(pool: &Pool) -> String {
    let mut connection = pool.get().await.unwrap();
    let row: Value = diesel::sql_query(
        "SELECT CAST(variable_value AS CHAR) AS v FROM performance_schema.session_status
         WHERE variable_name = 'Ssl_cipher'",
    )
    .get_result(&mut connection)
    .await
    .unwrap();
    row.v
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn pooling_and_migrations_against_mysql() {
    let pool = pool().await;
    ping(&pool).await.unwrap();

    // Recreating from anything leaves exactly the migrated schema.
    dangerously_recreate(&pool, migrations()).await.unwrap();
    assert!(table_exists(&pool, "items").await);
    assert!(table_exists(&pool, "notes").await);
    assert_eq!(
        pending(&pool, migrations()).await.unwrap(),
        Vec::<String>::new()
    );

    // Migrating again changes nothing.
    assert_eq!(migrate(&pool, migrations()).await.unwrap(), 0);

    // A violated unique constraint is recognised.
    insert_item(&pool, "one").await.unwrap();
    let error = insert_item(&pool, "one").await.unwrap_err();
    assert!(is_unique_violation(&error));

    // Truncating empties the table, even though another table references it.
    execute(
        &pool,
        "INSERT INTO notes (item_id, body) SELECT id, 'hi' FROM items",
    )
    .await
    .unwrap();
    truncate(&pool, &["items", "notes"]).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM notes").await, 0);

    // A bad name is refused before any SQL runs, even after a good one.
    insert_item(&pool, "two").await.unwrap();
    assert!(matches!(
        truncate(&pool, &["items", "items; DROP TABLE items"]).await,
        Err(Error::InvalidIdentifier(_))
    ));
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 1);

    // No tables is not an error.
    truncate::<&str>(&pool, &[]).await.unwrap();

    // Reverting one step removes only the newest migration.
    assert_eq!(revert(&pool, migrations(), 1).await.unwrap(), 1);
    assert!(table_exists(&pool, "items").await);
    assert!(!table_exists(&pool, "notes").await);
    assert_eq!(
        pending(&pool, migrations()).await.unwrap(),
        vec!["00000000000002_create_notes".to_string()]
    );

    // Reverting past the start stops at the start, and migrating restores it.
    assert_eq!(revert(&pool, migrations(), 5).await.unwrap(), 1);
    assert!(!table_exists(&pool, "items").await);
    assert_eq!(
        pending(&pool, migrations()).await.unwrap(),
        vec![
            "00000000000001_create_items".to_string(),
            "00000000000002_create_notes".to_string()
        ]
    );
    assert_eq!(migrate(&pool, migrations()).await.unwrap(), 2);

    // Recreating clears data, then works a second time.
    insert_item(&pool, "three").await.unwrap();
    dangerously_recreate(&pool, migrations()).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn truncating_restarts_counters_and_turns_foreign_key_checks_back_on() {
    // One connection, so the checks are seen on the connection that turned them off.
    let pool = pool_from(PoolConfig {
        max_connections: 1,
        ..PoolConfig::new(url_with(None))
    })
    .await;
    dangerously_recreate(&pool, migrations()).await.unwrap();
    insert_item(&pool, "one").await.unwrap();
    execute(&pool, "INSERT INTO notes (item_id, body) VALUES (1, 'hi')")
        .await
        .unwrap();

    // Only the table named is emptied, though another references it.
    truncate(&pool, &["items"]).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM notes").await, 1);

    // The counter starts again.
    insert_item(&pool, "two").await.unwrap();
    assert_eq!(count(&pool, "SELECT max(id) AS n FROM items").await, 1);

    // Foreign keys are checked again.
    let error = execute(
        &pool,
        "INSERT INTO notes (item_id, body) VALUES (999, 'no')",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        QueryError::DatabaseError(DatabaseErrorKind::ForeignKeyViolation, _)
    ));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn recreating_drops_tables_the_migrations_do_not_know() {
    let pool = pool().await;
    dangerously_recreate(&pool, migrations()).await.unwrap();
    // A stray table that references a migrated one, so the drop order matters
    // unless foreign key checks are off.
    execute(
        &pool,
        "CREATE TABLE stray (item_id BIGINT NOT NULL, FOREIGN KEY (item_id) REFERENCES items (id))",
    )
    .await
    .unwrap();
    insert_item(&pool, "one").await.unwrap();
    dangerously_recreate(&pool, migrations()).await.unwrap();
    assert!(!table_exists(&pool, "stray").await);
    assert!(table_exists(&pool, "items").await);
    assert!(table_exists(&pool, "notes").await);
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);
    assert_eq!(
        pending(&pool, migrations()).await.unwrap(),
        Vec::<String>::new()
    );
    // Rebuilt from the first migration, so the counter starts again.
    insert_item(&pool, "two").await.unwrap();
    assert_eq!(count(&pool, "SELECT max(id) AS n FROM items").await, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn a_pool_that_is_all_in_use_answers_with_an_error_in_time() {
    let config = PoolConfig {
        max_connections: 1,
        wait_timeout: Duration::from_millis(100),
        ..PoolConfig::new(url_with(None))
    };
    let pool = pool_from(config).await;
    let held = pool.get().await.unwrap();
    let waited = tokio::time::timeout(Duration::from_secs(2), pool.get()).await;
    assert!(waited.expect("the pool's own limit comes first").is_err());
    drop(held);
    assert!(pool.get().await.is_ok());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn an_unreachable_database_is_an_error() {
    let config = PoolConfig::new("mysql://nobody:nothing@127.0.0.1:1/none");
    assert!(connect(&config).await.is_err());
}

// The TLS tests expect the test server to offer TLS with a certificate that is
// not from a public root, as MySQL's own generated certificate is.

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn with_no_ssl_mode_tls_is_used_when_offered() {
    let pool = pool().await;
    assert_ne!(cipher(&pool).await, "");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn ssl_mode_disabled_connects_in_plain_text() {
    let pool = pool_from(PoolConfig::new(url_with(Some("ssl-mode=DISABLED")))).await;
    assert_eq!(cipher(&pool).await, "");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn ssl_mode_required_connects_with_tls() {
    let pool = pool_from(PoolConfig::new(url_with(Some("ssl-mode=REQUIRED")))).await;
    assert_ne!(cipher(&pool).await, "");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn ssl_mode_verify_ca_and_verify_identity_refuse_an_untrusted_server() {
    ensure_database().await;
    for mode in ["VERIFY_CA", "VERIFY_IDENTITY"] {
        let config = PoolConfig::new(url_with(Some(&format!("ssl-mode={mode}"))));
        let Err(error) = connect(&config).await else {
            panic!("{mode} connected");
        };
        assert!(error.to_string().contains("certificate"), "{mode}: {error}");
    }
}
