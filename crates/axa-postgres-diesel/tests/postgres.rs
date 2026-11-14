//! Behaviour against a real Postgres. Ignored by default, because these drop
//! every table in the database they are given. Run them with:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test -p axa-postgres-diesel -- --ignored --test-threads=1
//! ```

use std::{path::Path, time::Duration};

use axa_postgres_diesel::{
    Error, Pool, PoolConfig, connect, dangerously_recreate,
    diesel::{self, QueryableByName, sql_types::BigInt},
    diesel_async::RunQueryDsl,
    diesel_migrations::FileBasedMigrations,
    is_unique_violation, migrate, pending, ping, revert, truncate,
};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

async fn pool() -> Pool {
    let uri = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    connect(&PoolConfig::new(uri)).await.expect("connect")
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
    let sql = format!("SELECT count(*) AS n FROM pg_tables WHERE tablename = '{name}'");
    count(pool, &sql).await == 1
}

async fn insert_item(pool: &Pool, name: &str) -> Result<usize, diesel::result::Error> {
    let mut connection = pool.get().await.unwrap();
    diesel::sql_query(format!("INSERT INTO items (name) VALUES ('{name}')"))
        .execute(&mut connection)
        .await
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL and drops every table in it"]
async fn pooling_and_migrations_against_postgres() {
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
    truncate(&pool, &["items"]).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);

    // A bad name is refused before any SQL runs.
    insert_item(&pool, "two").await.unwrap();
    assert!(matches!(
        truncate(&pool, &["items; DROP TABLE items"]).await,
        Err(Error::InvalidIdentifier(_))
    ));
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 1);

    // No tables is not an error.
    truncate::<&str>(&pool, &[]).await.unwrap();

    // Reverting one step removes only the newest migration.
    assert_eq!(revert(&pool, migrations(), 1).await.unwrap(), 1);
    assert!(table_exists(&pool, "items").await);
    assert!(!table_exists(&pool, "notes").await);
    assert_eq!(pending(&pool, migrations()).await.unwrap().len(), 1);

    // Reverting past the start stops at the start, and migrating restores it.
    assert_eq!(revert(&pool, migrations(), 5).await.unwrap(), 1);
    assert!(!table_exists(&pool, "items").await);
    assert_eq!(migrate(&pool, migrations()).await.unwrap(), 2);

    // Recreating clears data and enum types, then works a second time.
    insert_item(&pool, "three").await.unwrap();
    dangerously_recreate(&pool, migrations()).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) AS n FROM items").await, 0);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL and drops every table in it"]
async fn recreating_keeps_extensions() {
    let pool = pool().await;
    {
        let mut connection = pool.get().await.unwrap();
        diesel::sql_query("CREATE EXTENSION IF NOT EXISTS citext")
            .execute(&mut connection)
            .await
            .unwrap();
    }
    dangerously_recreate(&pool, migrations()).await.unwrap();
    let found = count(
        &pool,
        "SELECT count(*) AS n FROM pg_extension WHERE extname = 'citext'",
    )
    .await;
    assert_eq!(found, 1);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_pool_that_is_all_in_use_answers_with_an_error_in_time() {
    let uri = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let config = PoolConfig {
        max_connections: 1,
        wait_timeout: Duration::from_millis(100),
        ..PoolConfig::new(uri)
    };
    let pool = connect(&config).await.unwrap();
    let held = pool.get().await.unwrap();
    let waited = tokio::time::timeout(Duration::from_secs(2), pool.get()).await;
    assert!(waited.expect("the pool's own limit comes first").is_err());
    drop(held);
    assert!(pool.get().await.is_ok());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn an_unreachable_database_is_an_error() {
    let config = PoolConfig::new("postgres://nobody:nothing@127.0.0.1:1/none");
    assert!(connect(&config).await.is_err());
}

#[derive(QueryableByName)]
struct Tenant {
    #[diesel(sql_type = diesel::sql_types::Text)]
    tenant: String,
}

const READ_TENANT: &str = "SELECT coalesce(current_setting('app.tenant', true), '') AS tenant";

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL as a superuser, and changes its schema"]
async fn settings_last_for_their_transaction_and_row_security_is_checked() {
    use axa_postgres_diesel::{
        diesel::result::Error as QueryError, diesel_async::AsyncConnection, row_security, set_local,
    };
    let uri = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let mut config = PoolConfig::new(uri);
    // One connection, so a leak, or SET ROLE, reaches the next query.
    config.max_connections = 1;
    let pool = connect(&config).await.unwrap();
    {
        let mut connection = pool.get().await.unwrap();
        // Outside a transaction, the setting would last one statement, so it is refused.
        let error = set_local(&mut connection, &[("app.tenant", "acme")])
            .await
            .unwrap_err();
        assert!(matches!(error, QueryError::NotInTransaction), "{error:?}");

        let seen = connection
            .transaction(async |connection| {
                set_local(connection, &[("app.tenant", "acme")]).await?;
                let row: Tenant = diesel::sql_query(READ_TENANT)
                    .get_result(connection)
                    .await?;
                Ok::<_, QueryError>(row.tenant)
            })
            .await
            .unwrap();
        assert_eq!(seen, "acme");
        let after: Tenant = diesel::sql_query(READ_TENANT)
            .get_result(&mut connection)
            .await
            .unwrap();
        assert_eq!(after.tenant, "", "a committed setting leaked");

        let refused = connection
            .transaction(async |connection| {
                set_local(connection, &[("search_path", "evil")]).await?;
                Ok::<_, QueryError>(())
            })
            .await
            .unwrap_err();
        assert!(
            refused.to_string().contains("not a custom setting"),
            "{refused}"
        );

        for statement in [
            "DROP TABLE IF EXISTS diesel_rls_plain, diesel_rls_forced",
            "CREATE TABLE diesel_rls_plain (tenant_id TEXT)",
            "CREATE TABLE diesel_rls_forced (tenant_id TEXT)",
            "ALTER TABLE diesel_rls_forced ENABLE ROW LEVEL SECURITY",
            "ALTER TABLE diesel_rls_forced FORCE ROW LEVEL SECURITY",
            "DROP ROLE IF EXISTS axa_diesel_rls_app",
            "CREATE ROLE axa_diesel_rls_app NOLOGIN",
        ] {
            diesel::sql_query(statement)
                .execute(&mut connection)
                .await
                .unwrap();
        }
    }
    let found = row_security(
        &pool,
        &[
            "diesel_rls_plain",
            "diesel_rls_forced",
            "diesel_rls_missing",
        ],
    )
    .await
    .unwrap();
    assert!(found.bypasses);
    assert_eq!(
        found.unprotected,
        ["diesel_rls_plain", "diesel_rls_missing"]
    );
    let run = async |sql: &str| {
        let mut connection = pool.get().await.unwrap();
        diesel::sql_query(sql)
            .execute(&mut connection)
            .await
            .unwrap();
    };
    run("SET ROLE axa_diesel_rls_app").await;
    let found = row_security(&pool, &["diesel_rls_forced"]).await.unwrap();
    assert_eq!(found.role, "axa_diesel_rls_app");
    assert!(found.holds(), "{found:?}");
    run("RESET ROLE").await;
    run("DROP TABLE diesel_rls_plain, diesel_rls_forced").await;
    run("DROP ROLE axa_diesel_rls_app").await;
}

#[test]
fn only_custom_settings_can_be_set() {
    use axa_postgres_diesel::is_custom_setting;
    for name in ["app.tenant", "app.user_id", "a.b.c", "_x.y"] {
        assert!(is_custom_setting(name), "{name}");
    }
    for name in [
        "role",
        "search_path",
        "session_authorization",
        "app.",
        ".tenant",
        "app..tenant",
        "App.Tenant",
        "app.tenant; RESET ROLE",
        "app.2fa",
        "",
    ] {
        assert!(!is_custom_setting(name), "{name}");
    }
}

#[derive(QueryableByName)]
struct Setting {
    #[diesel(sql_type = diesel::sql_types::Text)]
    value: String,
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn every_connection_starts_with_the_session_settings() {
    let uri = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let mut config = PoolConfig::new(uri);
    config.statement_timeout_ms = 1234;
    config.application_name = "axa-session-test".to_string();
    config.read_only = true;
    let pool = connect(&config).await.unwrap();
    let mut connection = pool.get().await.unwrap();
    let mut show = async |name: &str| -> String {
        diesel::sql_query("SELECT current_setting($1) AS value")
            .bind::<diesel::sql_types::Text, _>(name)
            .get_result::<Setting>(&mut connection)
            .await
            .unwrap()
            .value
    };
    assert_eq!(show("statement_timeout").await, "1234ms");
    assert_eq!(show("application_name").await, "axa-session-test");
    assert_eq!(show("default_transaction_read_only").await, "on");
    let refused = diesel::sql_query("CREATE TABLE axa_read_only_refused (x INT)")
        .execute(&mut connection)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("read-only"), "{refused}");
}
