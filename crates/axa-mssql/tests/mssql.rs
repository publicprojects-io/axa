#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "RCA-SCOPE-4: tests may unwrap"
)]
//! Behaviour against a real SQL Server. Ignored by default, because these drop
//! every table in the database they are given, and share it, so run them one at a
//! time:
//!
//! ```sh
//! AXA_TEST_MSSQL_URL='server=tcp:localhost,1433;user=sa;password=...;database=axa_test;TrustServerCertificate=true' \
//!     cargo test -p axa-mssql -- --ignored --test-threads=1
//! ```

use std::time::{Duration, Instant};

use axa_mssql::{
    Error, Migration, Pool, PoolConfig, Violation, connect, dangerously_recreate, is_unavailable,
    is_unique_violation, migrate, mssql, pending, ping, pool, revert, truncate, violation,
};

fn uri() -> String {
    std::env::var("AXA_TEST_MSSQL_URL").expect("AXA_TEST_MSSQL_URL")
}

async fn database() -> Pool {
    connect(&PoolConfig::new(uri())).await.expect("connect")
}

const ITEMS: Migration = Migration::new(
    1,
    "create items",
    "CREATE TABLE items (id BIGINT IDENTITY PRIMARY KEY, name NVARCHAR(100) NOT NULL UNIQUE,
        size INT NOT NULL DEFAULT 1 CHECK (size > 0))",
)
.with_down("DROP TABLE items");

const NOTES: Migration = Migration::new(
    2,
    "create notes",
    "CREATE TABLE notes (id BIGINT IDENTITY PRIMARY KEY,
        item_id BIGINT NOT NULL REFERENCES items (id), body NVARCHAR(100) NOT NULL)
GO
CREATE VIEW note_count AS SELECT item_id, COUNT(*) AS n FROM notes GROUP BY item_id",
)
.with_down("DROP VIEW note_count\nGO\nDROP TABLE notes");

const MIGRATIONS: &[Migration] = &[ITEMS, NOTES];

/// One number from `sql`.
async fn scalar(pool: &Pool, sql: &str) -> i64 {
    let mut connection = pool.get().await.unwrap();
    let row = connection
        .simple_query(sql)
        .await
        .unwrap()
        .into_row()
        .await
        .unwrap()
        .expect("a row");
    row.try_get::<i64, _>(0)
        .ok()
        .flatten()
        .or_else(|| row.get::<i32, _>(0).map(i64::from))
        .expect("a number")
}

async fn exec(pool: &Pool, sql: &str) -> Result<(), mssql::error::Error> {
    let mut connection = pool.get().await.unwrap();
    connection.simple_query(sql).await?.into_results().await?;
    Ok(())
}

async fn table_exists(pool: &Pool, name: &str) -> bool {
    scalar(
        pool,
        &format!("SELECT CAST(CASE WHEN OBJECT_ID(N'{name}') IS NULL THEN 0 ELSE 1 END AS BIGINT)"),
    )
    .await
        == 1
}

async fn count(pool: &Pool, table: &str) -> i64 {
    scalar(
        pool,
        &format!("SELECT CAST(COUNT(*) AS BIGINT) FROM {table}"),
    )
    .await
}

async fn insert_item(pool: &Pool, name: &str) -> Result<i64, mssql::error::Error> {
    let mut connection = pool.get().await.unwrap();
    let row = connection
        .query(
            "INSERT INTO items (name) OUTPUT INSERTED.id VALUES (@P1)",
            &[&name],
        )
        .await?
        .into_row()
        .await?
        .expect("a row");
    Ok(row.get::<i64, _>(0).expect("an id"))
}

/// An empty database, with nothing migrated.
async fn empty() -> Pool {
    let pool = database().await;
    dangerously_recreate(&pool, &[]).await.unwrap();
    pool
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL and drops every table in it"]
async fn migrations_apply_revert_and_list_what_is_pending() {
    let pool = empty().await;
    ping(&pool).await.unwrap();
    assert_eq!(
        pending(&pool, MIGRATIONS).await.unwrap(),
        ["1_create items", "2_create notes"]
    );
    assert_eq!(migrate(&pool, MIGRATIONS).await.unwrap(), 2);
    assert!(table_exists(&pool, "items").await);
    assert!(
        table_exists(&pool, "note_count").await,
        "the batch after GO ran"
    );
    assert_eq!(
        pending(&pool, MIGRATIONS).await.unwrap(),
        Vec::<String>::new()
    );
    // Migrating again changes nothing.
    assert_eq!(migrate(&pool, MIGRATIONS).await.unwrap(), 0);

    // Reverting one step removes only the newest migration, both its batches.
    assert_eq!(revert(&pool, MIGRATIONS, 1).await.unwrap(), 1);
    assert!(table_exists(&pool, "items").await);
    assert!(!table_exists(&pool, "notes").await);
    assert!(!table_exists(&pool, "note_count").await);
    assert_eq!(
        pending(&pool, MIGRATIONS).await.unwrap(),
        ["2_create notes"]
    );

    // Reverting past the start reverts what there is, and migrating restores it.
    assert_eq!(revert(&pool, MIGRATIONS, 5).await.unwrap(), 1);
    assert!(!table_exists(&pool, "items").await);
    assert_eq!(migrate(&pool, MIGRATIONS).await.unwrap(), 2);
    assert_eq!(count(&pool, "_axa_migrations").await, 2);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL and drops every table in it"]
async fn a_migration_changed_after_it_ran_is_refused_and_one_gone_from_the_list_is_left() {
    let pool = empty().await;
    migrate(&pool, MIGRATIONS).await.unwrap();
    let changed = Migration::new(1, "create items", "CREATE TABLE items (id INT)");
    let error = migrate(&pool, &[changed, NOTES]).await.unwrap_err();
    assert!(
        matches!(&error, Error::Migration(text) if text.contains("changed")),
        "{error:?}"
    );
    // A process with an older list, as during a rolling deploy, runs what it has.
    assert_eq!(migrate(&pool, &[ITEMS]).await.unwrap(), 0);
    // An applied migration that cannot be undone stops a revert.
    let error = revert(
        &pool,
        &[ITEMS, Migration::new(2, "create notes", NOTES.up)],
        1,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&error, Error::Migration(text) if text.contains("cannot be undone")),
        "{error:?}"
    );
    // One missing from the list stops it too.
    let error = revert(&pool, &[ITEMS], 1).await.unwrap_err();
    assert!(matches!(&error, Error::Migration(text) if text.contains("not in the list")));
    assert!(table_exists(&pool, "notes").await);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL and drops every table in it"]
async fn a_failed_migration_is_rolled_back_whole_and_not_recorded() {
    let pool = empty().await;
    let broken = Migration::new(
        3,
        "half done",
        "CREATE TABLE half (x INT)\nGO\nINSERT INTO no_such_table VALUES (1)",
    );
    let error = migrate(&pool, &[ITEMS, broken]).await.unwrap_err();
    assert!(
        matches!(&error, Error::Migration(text) if text.starts_with("3_half done")),
        "{error:?}"
    );
    assert!(
        table_exists(&pool, "items").await,
        "the migration before it stays"
    );
    assert!(
        !table_exists(&pool, "half").await,
        "its first batch was rolled back"
    );
    assert_eq!(
        pending(&pool, &[ITEMS, broken]).await.unwrap(),
        ["3_half done"]
    );
    // The migration lock was let go, so another migrate is not kept waiting.
    let started = Instant::now();
    assert_eq!(migrate(&pool, &[ITEMS]).await.unwrap(), 0);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL and drops every table in it"]
async fn services_booting_together_all_migrate_and_each_migration_runs_once() {
    let pool = empty().await;
    drop(pool);
    let boots: Vec<_> = (0_i32..4_i32)
        .map(|_| {
            tokio::spawn(async {
                let pool = database().await;
                migrate(&pool, MIGRATIONS).await.unwrap()
            })
        })
        .collect();
    let mut ran = 0_usize;
    for boot in boots {
        ran += boot.await.unwrap();
    }
    assert_eq!(ran, 2);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL and drops every table in it"]
async fn each_broken_constraint_is_named_and_none_is_unavailable() {
    let pool = empty().await;
    migrate(&pool, MIGRATIONS).await.unwrap();
    let id = insert_item(&pool, "a").await.unwrap();
    let error = insert_item(&pool, "a").await.unwrap_err();
    assert!(is_unique_violation(&error), "{error:?}");
    assert_eq!(violation(&error), Some(Violation::Unique));
    for (sql, kind) in [
        (
            "INSERT INTO notes (item_id, body) VALUES (999, 'x')",
            Violation::ForeignKey,
        ),
        ("DELETE FROM items", Violation::ForeignKey),
        (
            "INSERT INTO items (name, size) VALUES ('b', 0)",
            Violation::Check,
        ),
        ("INSERT INTO items (name) VALUES (NULL)", Violation::NotNull),
    ] {
        if kind == Violation::ForeignKey && sql.starts_with("DELETE") {
            exec(
                &pool,
                &format!("INSERT INTO notes (item_id, body) VALUES ({id}, 'x')"),
            )
            .await
            .unwrap();
        }
        let error = exec(&pool, sql).await.unwrap_err();
        assert_eq!(violation(&error), Some(kind), "{sql}: {error:?}");
        assert!(!is_unique_violation(&error));
        assert!(!is_unavailable(&error), "{sql}");
        assert!(!Error::Query(error).is_unavailable());
    }
    let error = exec(&pool, "SELECT * FROM no_such_table")
        .await
        .unwrap_err();
    assert_eq!(violation(&error), None);
    assert!(!is_unavailable(&error));
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL and drops every table in it"]
async fn truncate_empties_tables_in_any_order_and_restarts_their_ids() {
    let pool = empty().await;
    migrate(&pool, MIGRATIONS).await.unwrap();
    let id = insert_item(&pool, "a").await.unwrap();
    insert_item(&pool, "b").await.unwrap();
    exec(
        &pool,
        &format!("INSERT INTO notes (item_id, body) VALUES ({id}, 'x')"),
    )
    .await
    .unwrap();
    // Items first, though notes refer to them.
    truncate(&pool, &["items", "notes"]).await.unwrap();
    assert_eq!(count(&pool, "items").await, 0);
    assert_eq!(count(&pool, "notes").await, 0);
    assert_eq!(
        insert_item(&pool, "c").await.unwrap(),
        1,
        "the identity starts again"
    );
    // A table that never had a row keeps its first id too.
    truncate(&pool, &["items", "notes"]).await.unwrap();
    exec(&pool, "INSERT INTO items (name) VALUES ('d')")
        .await
        .unwrap();
    exec(&pool, "INSERT INTO notes (item_id, body) VALUES (1, 'y')")
        .await
        .unwrap();
    assert_eq!(scalar(&pool, "SELECT MIN(id) FROM notes").await, 1);
    truncate::<&str>(&pool, &[]).await.unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL and drops every table in it"]
async fn truncate_refuses_to_leave_a_reference_dangling_and_empties_nothing() {
    let pool = empty().await;
    migrate(&pool, MIGRATIONS).await.unwrap();
    let id = insert_item(&pool, "a").await.unwrap();
    exec(
        &pool,
        &format!("INSERT INTO notes (item_id, body) VALUES ({id}, 'x')"),
    )
    .await
    .unwrap();
    let error = truncate(&pool, &["items"]).await.unwrap_err();
    assert!(
        matches!(&error, Error::Query(error) if violation(error) == Some(Violation::ForeignKey)),
        "{error:?}"
    );
    assert_eq!(count(&pool, "items").await, 1);
    // A bad name is refused before anything is emptied.
    assert!(matches!(
        truncate(&pool, &["notes", "items; DROP TABLE items"]).await,
        Err(Error::InvalidIdentifier(_))
    ));
    assert_eq!(count(&pool, "notes").await, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL and drops every table in it"]
async fn dangerously_recreate_drops_every_table_and_migrates_from_nothing() {
    let pool = empty().await;
    migrate(&pool, MIGRATIONS).await.unwrap();
    insert_item(&pool, "a").await.unwrap();
    exec(
        &pool,
        "CREATE TABLE Stray (id INT PRIMARY KEY, item_id BIGINT REFERENCES items (id))",
    )
    .await
    .unwrap();
    for statement in [
        "CREATE PROCEDURE stray_procedure AS SELECT 1",
        "CREATE FUNCTION stray_function () RETURNS INT AS BEGIN RETURN 1 END",
        "CREATE SEQUENCE stray_sequence START WITH 1",
        "CREATE TYPE stray_type FROM NVARCHAR(10)",
        "CREATE SYNONYM stray_synonym FOR items",
    ] {
        exec(&pool, statement).await.unwrap();
    }
    dangerously_recreate(&pool, MIGRATIONS).await.unwrap();
    assert!(!table_exists(&pool, "Stray").await);
    for name in [
        "stray_procedure",
        "stray_function",
        "stray_sequence",
        "stray_synonym",
    ] {
        assert!(!table_exists(&pool, name).await, "{name}");
    }
    assert_eq!(
        scalar(
            &pool,
            "SELECT CAST(COUNT(*) AS BIGINT) FROM sys.types WHERE name = 'stray_type'"
        )
        .await,
        0
    );
    assert_eq!(count(&pool, "items").await, 0);
    assert_eq!(
        pending(&pool, MIGRATIONS).await.unwrap(),
        Vec::<String>::new()
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL"]
async fn every_connection_starts_with_the_session_settings_and_truncate_keeps_them() {
    let config = PoolConfig {
        max_connections: 1,
        lock_timeout_ms: 5000,
        ..PoolConfig::new(uri())
    };
    let pool = connect(&config).await.unwrap();
    let xact_abort = "SELECT CAST(@@OPTIONS & 16384 AS BIGINT)";
    assert_eq!(scalar(&pool, xact_abort).await, 16384);
    assert_eq!(
        scalar(&pool, "SELECT CAST(@@LOCK_TIMEOUT AS BIGINT)").await,
        5000
    );
    exec(
        &pool,
        "IF OBJECT_ID(N'tags') IS NULL CREATE TABLE tags (id INT)",
    )
    .await
    .unwrap();
    truncate(&pool, &["tags"]).await.unwrap();
    assert_eq!(
        scalar(&pool, xact_abort).await,
        16384,
        "on again after truncate"
    );
    exec(&pool, "DROP TABLE tags").await.unwrap();

    let plain = connect(&PoolConfig {
        xact_abort: false,
        ..PoolConfig::new(uri())
    })
    .await
    .unwrap();
    assert_eq!(scalar(&plain, xact_abort).await, 0);
    assert_eq!(
        scalar(&plain, "SELECT CAST(@@LOCK_TIMEOUT AS BIGINT)").await,
        -1
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL"]
async fn a_transaction_left_open_is_rolled_back_before_the_connection_is_used_again() {
    let pool = connect(&PoolConfig {
        max_connections: 1,
        ..PoolConfig::new(uri())
    })
    .await
    .unwrap();
    exec(
        &pool,
        "IF OBJECT_ID(N'left_open') IS NULL CREATE TABLE left_open (x INT)",
    )
    .await
    .unwrap();
    {
        let mut connection = pool.get().await.unwrap();
        connection
            .simple_query("BEGIN TRANSACTION; INSERT INTO left_open VALUES (1)")
            .await
            .unwrap()
            .into_results()
            .await
            .unwrap();
    }
    assert_eq!(scalar(&pool, "SELECT CAST(@@TRANCOUNT AS BIGINT)").await, 0);
    assert_eq!(count(&pool, "left_open").await, 0);
    exec(&pool, "DROP TABLE left_open").await.unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL"]
async fn a_stalled_server_fails_the_query_and_the_pool_drops_the_connection() {
    let pool = connect(&PoolConfig {
        max_connections: 1,
        command_timeout: Some(Duration::from_secs(1)),
        ..PoolConfig::new(uri())
    })
    .await
    .unwrap();
    let error = exec(&pool, "WAITFOR DELAY '00:00:03'").await.unwrap_err();
    assert!(
        matches!(&error, mssql::error::Error::Io { kind, .. } if *kind == std::io::ErrorKind::TimedOut),
        "{error:?}"
    );
    // The next caller gets a connection that works.
    ping(&pool).await.unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL"]
async fn a_pool_that_is_all_in_use_answers_with_an_error_in_time() {
    let pool = connect(&PoolConfig {
        max_connections: 1,
        wait_timeout: Duration::from_millis(200),
        ..PoolConfig::new(uri())
    })
    .await
    .unwrap();
    let held = pool.get().await.unwrap();
    let started = Instant::now();
    let error = ping(&pool).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(matches!(error, Error::Pool(_)), "{error:?}");
    assert!(error.is_unavailable());
    drop(held);
    ping(&pool).await.unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL"]
async fn a_wrong_password_is_an_error_that_leaves_the_password_out() {
    let uri = uri();
    let wrong = uri
        .split(';')
        .map(|pair| {
            if pair.to_ascii_lowercase().starts_with("password=") {
                "password=not-the-password-at-all"
            } else {
                pair
            }
        })
        .collect::<Vec<_>>()
        .join(";");
    let error = connect(&PoolConfig::new(wrong)).await.unwrap_err();
    assert!(matches!(error, Error::Pool(_)), "{error:?}");
    assert!(!error.to_string().contains("not-the-password"), "{error}");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MSSQL_URL as a login that may create databases"]
async fn a_write_to_a_read_only_database_is_unavailable() {
    let pool = database().await;
    for statement in [
        "IF DB_ID(N'axa_read_only') IS NOT NULL DROP DATABASE axa_read_only",
        "CREATE DATABASE axa_read_only",
        "CREATE TABLE axa_read_only.dbo.t (x INT)",
        "ALTER DATABASE axa_read_only SET READ_ONLY",
    ] {
        exec(&pool, statement).await.unwrap();
    }
    let error = exec(&pool, "INSERT INTO axa_read_only.dbo.t VALUES (1)")
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(3906), "{error:?}");
    assert!(is_unavailable(&error));
    assert!(Error::Query(error).is_unavailable());
    exec(&pool, "DROP DATABASE axa_read_only").await.unwrap();
}

#[tokio::test]
async fn a_closed_pool_fails_its_ping() {
    let pool = pool(&PoolConfig::new("server=tcp:127.0.0.1,1;user=a;password=b")).unwrap();
    pool.close();
    assert!(matches!(ping(&pool).await, Err(Error::Pool(_))));
}

#[tokio::test]
async fn an_unreachable_server_is_unavailable() {
    let pool = pool(&PoolConfig {
        wait_timeout: Duration::from_secs(2),
        ..PoolConfig::new("server=tcp:127.0.0.1,1;user=a;password=b")
    })
    .unwrap();
    let error = ping(&pool).await.unwrap_err();
    assert!(error.is_unavailable(), "{error:?}");
}
