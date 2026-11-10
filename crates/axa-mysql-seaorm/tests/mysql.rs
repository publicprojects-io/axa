//! Behaviour against a real MySQL. Ignored by default, because these create
//! and drop tables in the database they are given. Run them with:
//!
//! ```sh
//! AXA_TEST_MYSQL_URL=mysql://... cargo test -p axa-mysql-seaorm -- --ignored --test-threads=1
//! ```

use axa_mysql_seaorm::{
    PoolConfig, connect,
    schema::{ColType, add_column, create_table, drop_table, remove_column},
    sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement},
    sea_orm_migration::SchemaManager,
};

fn url() -> String {
    std::env::var("AXA_TEST_MYSQL_URL").expect("AXA_TEST_MYSQL_URL")
}

async fn database() -> DatabaseConnection {
    connect(&PoolConfig::new(url())).await.expect("connect")
}

/// One column's type and nullability, or `None` if it does not exist.
async fn column(db: &DatabaseConnection, table: &str, name: &str) -> Option<(String, String)> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT CAST(DATA_TYPE AS CHAR) AS data_type, CAST(IS_NULLABLE AS CHAR) AS is_nullable
             FROM information_schema.COLUMNS
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND COLUMN_NAME = ?",
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
        DbBackend::MySql,
        "SELECT CAST(CONSTRAINT_NAME AS CHAR) AS constraint_name
         FROM information_schema.TABLE_CONSTRAINTS
         WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND CONSTRAINT_TYPE = 'FOREIGN KEY'",
        [table.into()],
    ))
    .await
    .unwrap()
    .iter()
    .map(|row| row.try_get("", "constraint_name").unwrap())
    .collect()
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn connect_and_ping_and_ssl_mode_asks_for_tls() {
    let db = database().await;
    axa_mysql_seaorm::ping(&db).await.unwrap();

    // MySQL 8.4 has TLS on by default, so `REQUIRED` gets a cipher and
    // `DISABLED` gets none.
    let separator = if url().contains('?') { '&' } else { '?' };
    for (mode, encrypted) in [("REQUIRED", true), ("DISABLED", false)] {
        let db = connect(&PoolConfig::new(format!(
            "{}{separator}ssl-mode={mode}",
            url()
        )))
        .await
        .unwrap();
        let row = db
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                "SELECT CAST(VARIABLE_VALUE AS CHAR) AS cipher FROM performance_schema.session_status
                 WHERE VARIABLE_NAME = 'Ssl_cipher'",
            ))
            .await
            .unwrap()
            .unwrap();
        let cipher: String = row.try_get("", "cipher").unwrap();
        assert_eq!(!cipher.is_empty(), encrypted, "{mode}: {cipher:?}");
    }
}

/// A timestamp after 2038, where a MySQL `TIMESTAMP` ends, is kept.
async fn assert_keeps_a_timestamp_after_2038(db: &DatabaseConnection) {
    db.execute_unprepared(
        "UPDATE axa_items SET deleted_at = '2100-01-01 00:00:00' WHERE name = 'stamped'",
    )
    .await
    .unwrap();
    let later = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT CAST(YEAR(deleted_at) AS SIGNED) AS year FROM axa_items WHERE name = 'stamped'",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(later.try_get::<i64>("", "year").unwrap(), 2100);
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn schema_helpers_against_mysql() {
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
    let timestamp = ("datetime".to_string(), "NO".to_string());
    assert_eq!(
        column(&db, "axa_items", "created_at").await,
        Some(timestamp.clone())
    );
    assert_eq!(
        column(&db, "axa_items", "updated_at").await,
        Some(timestamp)
    );

    // A row that names neither timestamp gets the time of the insert, in UTC.
    db.execute_unprepared(
        "INSERT INTO axa_items (pid, name, data) VALUES (UUID_TO_BIN(UUID()), 'stamped', '{}')",
    )
    .await
    .unwrap();
    let stamped = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT CAST(ABS(TIMESTAMPDIFF(SECOND, created_at, UTC_TIMESTAMP())) < 60
                AND ABS(TIMESTAMPDIFF(SECOND, updated_at, UTC_TIMESTAMP())) < 60 AS SIGNED) AS fresh
             FROM axa_items WHERE name = 'stamped'",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stamped.try_get::<i64>("", "fresh").unwrap(), 1);

    assert_keeps_a_timestamp_after_2038(&db).await;
    db.execute_unprepared("DELETE FROM axa_items")
        .await
        .unwrap();
    assert_eq!(
        column(&db, "axa_items", "id").await,
        Some(("bigint".to_string(), "NO".to_string()))
    );
    assert_eq!(column(&db, "axa_items", "data").await.unwrap().0, "json");
    assert_eq!(column(&db, "axa_items", "pid").await.unwrap().0, "binary");
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
    use axa_mysql_seaorm::{
        PoolConfig, connect, dangerously_recreate, is_unique_violation, migrate, pending, revert,
        sea_orm::{ConnectionTrait, DbBackend, Statement},
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
                    "CREATE TABLE seaorm_people (
                        id BIGINT AUTO_INCREMENT PRIMARY KEY, email VARCHAR(255) UNIQUE)",
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

    // Notes refer to people, so emptying or dropping people needs foreign key
    // checks off.
    #[async_trait::async_trait]
    impl MigrationTrait for Notes {
        async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            manager
                .get_connection()
                .execute_unprepared(
                    "CREATE TABLE seaorm_notes (
                        id BIGINT AUTO_INCREMENT PRIMARY KEY,
                        person_id BIGINT NOT NULL,
                        FOREIGN KEY (person_id) REFERENCES seaorm_people (id))",
                )
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

    /// A pool of one connection, so a check after a reset runs on the very
    /// connection any session setting would have been left on.
    async fn fresh() -> axa_mysql_seaorm::Pool {
        let db = connect(&PoolConfig {
            min_connections: 1,
            max_connections: 1,
            ..PoolConfig::new(super::url())
        })
        .await
        .unwrap();
        for table in [
            "seaorm_notes",
            "seaorm_people",
            "seaorm_stray",
            "seaql_migrations",
        ] {
            db.execute_unprepared(&format!("DROP TABLE IF EXISTS {table}"))
                .await
                .unwrap();
        }
        db
    }

    async fn count(db: &axa_mysql_seaorm::Pool, table: &str) -> i64 {
        db.query_one_raw(Statement::from_string(
            DbBackend::MySql,
            format!("SELECT COUNT(*) AS n FROM {table}"),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
    }

    async fn foreign_key_checks(db: &axa_mysql_seaorm::Pool) -> i64 {
        db.query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT CAST(@@FOREIGN_KEY_CHECKS AS SIGNED) AS on_",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "on_")
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_MYSQL_URL"]
    async fn migrations_apply_revert_and_list_what_is_pending() {
        let db = fresh().await;
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
        assert!(super::column(&db, "seaorm_notes", "id").await.is_none());
        assert!(super::column(&db, "seaorm_people", "id").await.is_some());
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_MYSQL_URL"]
    async fn a_duplicate_is_a_unique_violation() {
        let db = fresh().await;
        migrate::<Migrator>(&db).await.unwrap();
        let insert = "INSERT INTO seaorm_people (email) VALUES ('a@example.org')";
        db.execute_unprepared(insert).await.unwrap();
        let error = db.execute_unprepared(insert).await.unwrap_err();
        assert!(is_unique_violation(&error), "{error}");
        let error = db
            .execute_unprepared("INSERT INTO seaorm_notes (person_id) VALUES (999)")
            .await
            .unwrap_err();
        assert!(!is_unique_violation(&error), "{error}");
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_MYSQL_URL"]
    async fn truncate_refuses_an_unsafe_name_before_emptying_anything() {
        let db = fresh().await;
        migrate::<Migrator>(&db).await.unwrap();
        db.execute_unprepared("INSERT INTO seaorm_people (email) VALUES ('a@example.org')")
            .await
            .unwrap();
        assert!(
            truncate(&db, &["seaorm_people", "seaorm_notes; DROP TABLE x"])
                .await
                .is_err()
        );
        assert!(
            truncate(&db, &["seaorm_people", "SeaormNotes"])
                .await
                .is_err()
        );
        assert_eq!(count(&db, "seaorm_people").await, 1);
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_MYSQL_URL"]
    async fn truncate_empties_referenced_tables_resets_ids_and_restores_checks() {
        let db = fresh().await;
        migrate::<Migrator>(&db).await.unwrap();
        db.execute_unprepared("INSERT INTO seaorm_people (email) VALUES ('a@example.org')")
            .await
            .unwrap();
        db.execute_unprepared("INSERT INTO seaorm_notes (person_id) VALUES (1)")
            .await
            .unwrap();
        // MySQL refuses TRUNCATE on a table a foreign key refers to, unless the
        // checks are off.
        assert!(
            db.execute_unprepared("TRUNCATE TABLE seaorm_people")
                .await
                .is_err()
        );
        truncate(&db, &["seaorm_notes", "seaorm_people"])
            .await
            .unwrap();
        assert_eq!(count(&db, "seaorm_people").await, 0);
        assert_eq!(count(&db, "seaorm_notes").await, 0);

        // The checks are on again on the pool's only connection.
        assert_eq!(foreign_key_checks(&db).await, 1);
        assert!(
            db.execute_unprepared("INSERT INTO seaorm_notes (person_id) VALUES (999)")
                .await
                .is_err()
        );

        // `AUTO_INCREMENT` starts again.
        db.execute_unprepared("INSERT INTO seaorm_people (email) VALUES ('b@example.org')")
            .await
            .unwrap();
        let row = db
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                "SELECT id FROM seaorm_people",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "id").unwrap(), 1);
        truncate::<&str>(&db, &[]).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_MYSQL_URL and drops every table in it"]
    async fn dangerously_recreate_rebuilds_from_the_first_migration() {
        let db = fresh().await;
        migrate::<Migrator>(&db).await.unwrap();
        db.execute_unprepared("INSERT INTO seaorm_people (email) VALUES ('a@example.org')")
            .await
            .unwrap();
        db.execute_unprepared("INSERT INTO seaorm_notes (person_id) VALUES (1)")
            .await
            .unwrap();
        db.execute_unprepared("CREATE TABLE seaorm_stray (id BIGINT PRIMARY KEY)")
            .await
            .unwrap();
        revert::<Migrator>(&db, 1).await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE seaorm_notes (person_id BIGINT,
                FOREIGN KEY (person_id) REFERENCES seaorm_people (id))",
        )
        .await
        .unwrap();
        db.execute_unprepared("INSERT INTO seaorm_notes (person_id) VALUES (1)")
            .await
            .unwrap();

        dangerously_recreate::<Migrator>(&db).await.unwrap();

        // The stray table is gone, every migration is applied, and the tables
        // are empty and as the migrations made them.
        assert!(super::column(&db, "seaorm_stray", "id").await.is_none());
        assert_eq!(
            pending::<Migrator>(&db).await.unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(count(&db, "seaorm_people").await, 0);
        assert_eq!(count(&db, "seaorm_notes").await, 0);
        assert!(super::column(&db, "seaorm_notes", "id").await.is_some());
        assert_eq!(foreign_key_checks(&db).await, 1);
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_MYSQL_URL"]
    async fn dangerously_recreate_refuses_a_connection_with_no_database() {
        // The URL without its database name.
        let url = super::url();
        let rest = url.strip_prefix("mysql://").unwrap();
        let host = rest.split(['/', '?']).next().unwrap();
        let db = connect(&PoolConfig::new(format!("mysql://{host}/")))
            .await
            .unwrap();
        let error = dangerously_recreate::<Migrator>(&db).await.unwrap_err();
        assert!(error.to_string().contains("no database"), "{error}");
    }
}
