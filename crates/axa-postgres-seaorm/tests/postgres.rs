//! Behaviour against a real Postgres. Ignored by default, because these create
//! and drop tables in the database they are given. Run them with:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test -p axa-postgres-seaorm -- --ignored --test-threads=1
//! ```

use axa_postgres_seaorm::{
    PoolConfig, connect,
    schema::{ColType, add_column, create_table, drop_table, remove_column},
    sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement},
    sea_orm_migration::SchemaManager,
};

async fn database() -> DatabaseConnection {
    let uri = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    connect(&PoolConfig::new(uri)).await.expect("connect")
}

/// One column's type and nullability, or `None` if it does not exist.
async fn column(db: &DatabaseConnection, table: &str, name: &str) -> Option<(String, String)> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT data_type, is_nullable FROM information_schema.columns
             WHERE table_name = $1 AND column_name = $2",
            [table.into(), name.into()],
        ))
        .await
        .unwrap()?;
    Some((
        row.try_get("", "data_type").unwrap(),
        row.try_get("", "is_nullable").unwrap(),
    ))
}

/// The foreign key names on `table`.
async fn foreign_keys(db: &DatabaseConnection, table: &str) -> Vec<String> {
    db.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT constraint_name FROM information_schema.table_constraints
         WHERE table_name = $1 AND constraint_type = 'FOREIGN KEY'",
        [table.into()],
    ))
    .await
    .unwrap()
    .iter()
    .map(|row| row.try_get("", "constraint_name").unwrap())
    .collect()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL and changes its schema"]
async fn schema_helpers_against_postgres() {
    let db = database().await;
    let m = SchemaManager::new(&db);
    drop_table(&m, "axa_notes").await.ok();
    drop_table(&m, "axa_items").await.ok();

    // Names are pluralised, and timestamps are added.
    create_table(
        &m,
        "axa_item",
        &[
            ("id", ColType::PkAuto),
            ("pid", ColType::UuidUniq),
            ("name", ColType::String),
            ("data", ColType::JsonBinary),
            ("active", ColType::BooleanWithDefault(true)),
            ("deleted_at", ColType::TimestampWithTimeZoneNull),
        ],
        &[],
    )
    .await
    .unwrap();
    let timestamp = ("timestamp with time zone".to_string(), "NO".to_string());
    assert_eq!(
        column(&db, "axa_items", "created_at").await,
        Some(timestamp.clone())
    );
    assert_eq!(
        column(&db, "axa_items", "updated_at").await,
        Some(timestamp)
    );

    // A row that names neither timestamp gets the time of the insert.
    db.execute_unprepared(
        "INSERT INTO axa_items (pid, name, data) VALUES (gen_random_uuid(), 'stamped', '{}')",
    )
    .await
    .unwrap();
    let stamped = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT now() - created_at < interval '1 minute'
                AND now() - updated_at < interval '1 minute' AS fresh
             FROM axa_items WHERE name = 'stamped'",
        ))
        .await
        .unwrap()
        .unwrap();
    assert!(stamped.try_get::<bool>("", "fresh").unwrap());
    db.execute_unprepared("DELETE FROM axa_items")
        .await
        .unwrap();
    assert_eq!(
        column(&db, "axa_items", "id").await,
        Some(("bigint".to_string(), "NO".to_string()))
    );
    assert_eq!(column(&db, "axa_items", "data").await.unwrap().0, "jsonb");
    assert_eq!(
        column(&db, "axa_items", "deleted_at").await.unwrap().1,
        "YES"
    );

    // References get a bigint column and a predictable name. A `?` suffix
    // makes the reference optional.
    create_table(
        &m,
        "axa_notes",
        &[("id", ColType::PkAuto)],
        &[("axa_items", ""), ("axa_items?", "parent_id")],
    )
    .await
    .unwrap();
    assert_eq!(
        column(&db, "axa_notes", "axa_item_id").await,
        Some(("bigint".to_string(), "NO".to_string()))
    );
    assert_eq!(
        column(&db, "axa_notes", "parent_id").await,
        Some(("bigint".to_string(), "YES".to_string()))
    );
    let keys = foreign_keys(&db, "axa_notes").await;
    assert!(
        keys.contains(&"fk-axa_notes-axa_item_id-to-axa_items".to_string()),
        "{keys:?}"
    );
    assert!(
        keys.contains(&"fk-axa_notes-parent_id-to-axa_items".to_string()),
        "{keys:?}"
    );

    // Columns can be added and removed.
    add_column(&m, "axa_items", "note", ColType::TextNull)
        .await
        .unwrap();
    assert!(column(&db, "axa_items", "note").await.is_some());
    remove_column(&m, "axa_items", "note").await.unwrap();
    assert!(column(&db, "axa_items", "note").await.is_none());

    drop_table(&m, "axa_notes").await.unwrap();
    drop_table(&m, "axa_items").await.unwrap();
    assert!(column(&db, "axa_items", "id").await.is_none());
}

// The functions every database crate has.

mod shared {
    use axa_postgres_seaorm::{
        dangerously_recreate, is_unique_violation, migrate, pending, ping, revert,
        sea_orm::ConnectionTrait,
        sea_orm_migration::{
            MigrationName, MigrationTrait, MigratorTrait, SchemaManager, async_trait,
            sea_orm::DbErr,
        },
        truncate,
    };

    struct People;
    struct Notes;

    impl MigrationName for People {
        fn name(&self) -> &'static str {
            "m0001_people"
        }
    }
    impl MigrationName for Notes {
        fn name(&self) -> &'static str {
            "m0002_notes"
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for People {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .get_connection()
                .execute_unprepared(
                    "CREATE TABLE seaorm_people (id BIGSERIAL PRIMARY KEY, email TEXT UNIQUE)",
                )
                .await
                .map(|_| ())
        }
        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .get_connection()
                .execute_unprepared("DROP TABLE seaorm_people")
                .await
                .map(|_| ())
        }
    }

    #[async_trait::async_trait]
    impl MigrationTrait for Notes {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .get_connection()
                .execute_unprepared("CREATE TABLE seaorm_notes (id BIGSERIAL PRIMARY KEY)")
                .await
                .map(|_| ())
        }
        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .get_connection()
                .execute_unprepared("DROP TABLE seaorm_notes")
                .await
                .map(|_| ())
        }
    }

    struct Migrator;

    impl MigratorTrait for Migrator {
        fn migrations() -> Vec<Box<dyn MigrationTrait>> {
            vec![Box::new(People), Box::new(Notes)]
        }
    }

    async fn fresh() -> axa_postgres_seaorm::Pool {
        let db = super::database().await;
        for table in ["seaorm_notes", "seaorm_people", "seaql_migrations"] {
            db.execute_unprepared(&format!("DROP TABLE IF EXISTS {table}"))
                .await
                .unwrap();
        }
        db
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DB_URL"]
    async fn migrations_apply_revert_and_list_what_is_pending() {
        let db = fresh().await;
        ping(&db).await.unwrap();
        assert_eq!(
            pending::<Migrator>(&db).await.unwrap(),
            ["m0001_people", "m0002_notes"]
        );
        migrate::<Migrator>(&db).await.unwrap();
        assert_eq!(
            pending::<Migrator>(&db).await.unwrap(),
            Vec::<String>::new()
        );
        revert::<Migrator>(&db, 1).await.unwrap();
        assert_eq!(pending::<Migrator>(&db).await.unwrap(), ["m0002_notes"]);
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DB_URL"]
    async fn truncate_empties_tables_and_a_duplicate_is_a_unique_violation() {
        let db = fresh().await;
        migrate::<Migrator>(&db).await.unwrap();
        let insert = "INSERT INTO seaorm_people (email) VALUES ('a@example.org')";
        db.execute_unprepared(insert).await.unwrap();
        let error = db.execute_unprepared(insert).await.unwrap_err();
        assert!(is_unique_violation(&error), "{error}");
        assert!(
            truncate(&db, &["seaorm_people; DROP TABLE x"])
                .await
                .is_err()
        );
        truncate(&db, &["seaorm_people"]).await.unwrap();
        db.execute_unprepared(insert).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DB_URL and drops every table in it"]
    async fn dangerously_recreate_rebuilds_from_the_first_migration() {
        let db = fresh().await;
        migrate::<Migrator>(&db).await.unwrap();
        let insert = "INSERT INTO seaorm_people (email) VALUES ('a@example.org')";
        db.execute_unprepared(insert).await.unwrap();
        dangerously_recreate::<Migrator>(&db).await.unwrap();
        db.execute_unprepared(insert).await.unwrap();
        assert_eq!(
            pending::<Migrator>(&db).await.unwrap(),
            Vec::<String>::new()
        );
    }
}

/// The one setting, or an empty string when unset.
async fn tenant_on(db: &impl ConnectionTrait) -> String {
    db.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT coalesce(current_setting('app.tenant', true), '') AS tenant",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "tenant")
    .unwrap()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL as a superuser, and changes its schema"]
async fn settings_last_for_their_transaction_and_row_security_is_checked() {
    use axa_postgres_seaorm::{begin_with, row_security, sea_orm::TransactionTrait};
    let uri = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    // One connection, so a leak, or SET ROLE, reaches the next query.
    let mut config = PoolConfig::new(uri);
    config.min_connections = 1;
    config.max_connections = 1;
    let db = connect(&config).await.unwrap();

    let tx = begin_with(&db, &[("app.tenant", "acme")]).await.unwrap();
    assert_eq!(tenant_on(&tx).await, "acme");
    tx.commit().await.unwrap();
    assert_eq!(tenant_on(&db).await, "", "a committed setting leaked");
    let tx = begin_with(&db, &[("app.tenant", "globex")]).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(tenant_on(&db).await, "", "a rolled-back setting leaked");
    let error = begin_with(&db, &[("role", "postgres")]).await.unwrap_err();
    assert!(
        error.to_string().contains("not a custom setting"),
        "{error}"
    );
    // Nothing was begun for the refused name.
    db.begin().await.unwrap().rollback().await.unwrap();

    for statement in [
        "DROP TABLE IF EXISTS sea_rls_plain, sea_rls_forced",
        "CREATE TABLE sea_rls_plain (tenant_id TEXT)",
        "CREATE TABLE sea_rls_forced (tenant_id TEXT)",
        "ALTER TABLE sea_rls_forced ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE sea_rls_forced FORCE ROW LEVEL SECURITY",
        "DROP ROLE IF EXISTS axa_sea_rls_app",
        "CREATE ROLE axa_sea_rls_app NOLOGIN",
    ] {
        db.execute_unprepared(statement).await.unwrap();
    }
    let found = row_security(&db, &["sea_rls_plain", "sea_rls_forced", "sea_rls_missing"])
        .await
        .unwrap();
    assert!(found.bypasses);
    assert_eq!(found.unprotected, ["sea_rls_plain", "sea_rls_missing"]);
    db.execute_unprepared("SET ROLE axa_sea_rls_app")
        .await
        .unwrap();
    let found = row_security(&db, &["sea_rls_forced"]).await.unwrap();
    assert_eq!(found.role, "axa_sea_rls_app");
    assert!(found.holds(), "{found:?}");
    db.execute_unprepared("RESET ROLE").await.unwrap();
    db.execute_unprepared("DROP TABLE sea_rls_plain, sea_rls_forced")
        .await
        .unwrap();
    db.execute_unprepared("DROP ROLE axa_sea_rls_app")
        .await
        .unwrap();
}

#[test]
fn only_custom_settings_can_be_set() {
    use axa_postgres_seaorm::is_custom_setting;
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
