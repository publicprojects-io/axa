//! Against a real Postgres. The tests share one database, so run them with
//! `--test-threads=1`, on a database this suite may wipe.
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test -p axa-postgres-toasty -- --ignored --test-threads=1
//! ```

#![allow(
    clippy::used_underscore_items,
    reason = "the toasty::Model derive generates underscore items"
)]

use axa_postgres_toasty::{
    PoolConfig, connect, dangerously_recreate, is_unique_violation, migrate, pending, ping,
    truncate,
};
use toasty::migration::{MigrationFile, MigrationSet};

#[derive(Debug, toasty::Model)]
#[table = "toasty_people"]
struct Person {
    #[key]
    #[auto]
    id: u64,
    #[unique]
    email: String,
}

static MIGRATIONS: MigrationSet = MigrationSet::new(&[
    MigrationFile::new(
        1,
        "0001_people",
        "CREATE TABLE toasty_people (id BIGSERIAL PRIMARY KEY, email TEXT NOT NULL UNIQUE)",
    ),
    MigrationFile::new(
        2,
        "0002_notes",
        "CREATE TABLE toasty_notes (id BIGSERIAL PRIMARY KEY)",
    ),
]);

fn uri() -> String {
    std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL")
}

async fn fresh() -> axa_postgres_toasty::Pool {
    let db = connect(&PoolConfig::new(uri()), toasty::models!(Person))
        .await
        .expect("connect");
    let mut handle = db.clone();
    for table in ["toasty_notes", "toasty_people", "__toasty_migrations"] {
        toasty::sql::statement(format!("DROP TABLE IF EXISTS {table}"))
            .exec(&mut handle)
            .await
            .unwrap();
    }
    db
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn connect_checks_the_database_answers() {
    let db = fresh().await;
    ping(&db).await.unwrap();
    let unreachable = PoolConfig {
        connect_timeout_ms: 500,
        ..PoolConfig::new("postgresql://axa:axa@127.0.0.1:1/none")
    };
    assert!(
        connect(&unreachable, toasty::models!(Person))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn migrations_apply_once_and_pending_names_the_rest() {
    let db = fresh().await;
    assert_eq!(
        pending(&db, &MIGRATIONS).await.unwrap(),
        ["0001_people", "0002_notes"]
    );
    assert_eq!(migrate(&db, &MIGRATIONS).await.unwrap(), 2);
    assert_eq!(
        pending(&db, &MIGRATIONS).await.unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(migrate(&db, &MIGRATIONS).await.unwrap(), 0, "applied once");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn truncate_empties_tables_and_refuses_unsafe_names() {
    let mut db = fresh().await;
    migrate(&db, &MIGRATIONS).await.unwrap();
    toasty::create!(Person {
        email: "a@example.org"
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert!(
        truncate(&db, &["toasty_people; DROP TABLE x"])
            .await
            .is_err()
    );
    truncate(&db, &["toasty_people"]).await.unwrap();
    assert!(Person::all().exec(&mut db).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_duplicate_is_a_unique_violation() {
    let mut db = fresh().await;
    migrate(&db, &MIGRATIONS).await.unwrap();
    toasty::create!(Person {
        email: "a@example.org"
    })
    .exec(&mut db)
    .await
    .unwrap();
    let error = toasty::create!(Person {
        email: "a@example.org"
    })
    .exec(&mut db)
    .await
    .unwrap_err();
    assert!(is_unique_violation(&error), "{error}");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL and drops every table in it"]
async fn dangerously_recreate_rebuilds_from_the_first_migration() {
    let mut db = fresh().await;
    migrate(&db, &MIGRATIONS).await.unwrap();
    toasty::create!(Person {
        email: "a@example.org"
    })
    .exec(&mut db)
    .await
    .unwrap();
    dangerously_recreate(&db, &MIGRATIONS).await.unwrap();
    assert!(Person::all().exec(&mut db).await.unwrap().is_empty());
    assert_eq!(
        pending(&db, &MIGRATIONS).await.unwrap(),
        Vec::<String>::new()
    );
}

/// The one setting, or an empty string when unset.
async fn tenant_on(executor: &mut dyn toasty::Executor) -> String {
    let rows =
        toasty::sql::query("SELECT coalesce(current_setting('app.tenant', true), '') AS tenant")
            .exec(executor)
            .await
            .unwrap();
    match rows.first() {
        Some(toasty::stmt::Value::Record(record)) => match record.fields.first() {
            Some(toasty::stmt::Value::String(tenant)) => tenant.clone(),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL as a superuser, and changes its schema"]
async fn settings_last_for_their_transaction_and_row_security_is_checked() {
    use axa_postgres_toasty::{begin_with, row_security};
    let uri = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    // One connection, so a leak, or SET ROLE, reaches the next query.
    let mut config = PoolConfig::new(uri);
    config.max_connections = 1;
    let mut db = connect(&config, toasty::models!(Person)).await.unwrap();

    let mut tx = begin_with(&mut db, &[("app.tenant", "acme")])
        .await
        .unwrap();
    assert_eq!(tenant_on(&mut tx).await, "acme");
    tx.commit().await.unwrap();
    assert_eq!(tenant_on(&mut db).await, "", "a committed setting leaked");
    let tx = begin_with(&mut db, &[("app.tenant", "globex")])
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(tenant_on(&mut db).await, "", "a rolled-back setting leaked");
    let error = begin_with(&mut db, &[("role", "postgres")])
        .await
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("not a custom setting"),
        "{error}"
    );

    for statement in [
        "DROP TABLE IF EXISTS toasty_rls_plain, toasty_rls_forced",
        "CREATE TABLE toasty_rls_plain (tenant_id TEXT)",
        "CREATE TABLE toasty_rls_forced (tenant_id TEXT)",
        "ALTER TABLE toasty_rls_forced ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE toasty_rls_forced FORCE ROW LEVEL SECURITY",
        "DROP ROLE IF EXISTS axa_toasty_rls_app",
        "CREATE ROLE axa_toasty_rls_app NOLOGIN",
    ] {
        toasty::sql::statement(statement)
            .exec(&mut db)
            .await
            .unwrap();
    }
    let found = row_security(
        &db,
        &[
            "toasty_rls_plain",
            "toasty_rls_forced",
            "toasty_rls_missing",
        ],
    )
    .await
    .unwrap();
    assert!(found.bypasses);
    assert_eq!(
        found.unprotected,
        ["toasty_rls_plain", "toasty_rls_missing"]
    );
    toasty::sql::statement("SET ROLE axa_toasty_rls_app")
        .exec(&mut db)
        .await
        .unwrap();
    let found = row_security(&db, &["toasty_rls_forced"]).await.unwrap();
    assert_eq!(found.role, "axa_toasty_rls_app");
    assert!(found.holds(), "{found:?}");
    for statement in [
        "RESET ROLE",
        "DROP TABLE toasty_rls_plain, toasty_rls_forced",
        "DROP ROLE axa_toasty_rls_app",
    ] {
        toasty::sql::statement(statement)
            .exec(&mut db)
            .await
            .unwrap();
    }
}

#[test]
fn only_custom_settings_can_be_set() {
    use axa_postgres_toasty::is_custom_setting;
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
