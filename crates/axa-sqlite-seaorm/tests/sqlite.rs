//! Behaviour against a real SQLite. Each test makes a database file of its own
//! under Cargo's target directory, so these run in `cargo test` with no server.

use std::path::PathBuf;

use axa_sqlite_seaorm::{
    Pool, PoolConfig, connect,
    schema::{ColType, add_column, create_table, drop_table, remove_column},
    sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait},
    sea_orm_migration::SchemaManager,
};

/// The URL of a fresh database file named after `test`.
fn url(test: &str) -> String {
    let path =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("axa-sqlite-seaorm-{test}.db"));
    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{}{suffix}", path.display())).ok();
    }
    format!("sqlite://{}?mode=rwc", path.display())
}

async fn database(test: &str) -> Pool {
    connect(&PoolConfig::new(url(test))).await.expect("connect")
}

async fn scalar<T>(db: &impl ConnectionTrait, sql: &str) -> T
where
    T: axa_sqlite_seaorm::sea_orm::TryGetable,
{
    db.query_one_raw(Statement::from_string(DbBackend::Sqlite, sql))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index(0)
        .unwrap()
}

/// One column's declared type and whether it is `NOT NULL`, or `None` if it does
/// not exist.
async fn column(db: &Pool, table: &str, name: &str) -> Option<(String, bool)> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r#"SELECT type, "notnull" FROM pragma_table_info(?) WHERE name = ?"#,
            [table.into(), name.into()],
        ))
        .await
        .unwrap()?;
    Some((
        row.try_get("", "type").unwrap(),
        row.try_get::<i32>("", "notnull").unwrap() == 1,
    ))
}

/// `(column, parent, on_delete)` for each foreign key on `table`.
async fn foreign_keys(db: &Pool, table: &str) -> Vec<(String, String, String)> {
    db.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        r#"SELECT "from", "table", on_delete FROM pragma_foreign_key_list(?) ORDER BY "from""#,
        [table.into()],
    ))
    .await
    .unwrap()
    .iter()
    .map(|row| {
        (
            row.try_get("", "from").unwrap(),
            row.try_get("", "table").unwrap(),
            row.try_get("", "on_delete").unwrap(),
        )
    })
    .collect()
}

#[tokio::test]
async fn every_connection_has_foreign_keys_wal_and_a_busy_timeout() {
    let db = database("pragmas").await;
    // Hold three transactions open at once, so each is on a connection of its own.
    let txns = [
        db.begin().await.unwrap(),
        db.begin().await.unwrap(),
        db.begin().await.unwrap(),
    ];
    for txn in &txns {
        assert_eq!(scalar::<i32>(txn, "PRAGMA foreign_keys").await, 1);
        assert_eq!(scalar::<i32>(txn, "PRAGMA busy_timeout").await, 5_000);
        assert_eq!(scalar::<String>(txn, "PRAGMA journal_mode").await, "wal");
    }
}

#[tokio::test]
async fn an_in_memory_database_is_shared_by_the_pools_connections() {
    let db = connect(&PoolConfig::new("sqlite::memory:")).await.unwrap();
    db.execute_unprepared("CREATE TABLE things (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    let txns = [db.begin().await.unwrap(), db.begin().await.unwrap()];
    for txn in &txns {
        assert_eq!(scalar::<i32>(txn, "SELECT count(*) FROM things").await, 0);
        assert_eq!(scalar::<i32>(txn, "PRAGMA foreign_keys").await, 1);
        assert_eq!(scalar::<String>(txn, "PRAGMA journal_mode").await, "memory");
    }
}

#[tokio::test]
async fn a_missing_file_is_not_made_unless_the_url_asks() {
    let made = url("missing");
    let missing = made.trim_end_matches("?mode=rwc");
    assert!(connect(&PoolConfig::new(missing)).await.is_err());
    assert!(connect(&PoolConfig::new(made)).await.is_ok());
}

#[tokio::test]
async fn schema_helpers_against_sqlite() {
    let db = database("schema").await;
    let m = SchemaManager::new(&db);

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
    let created = column(&db, "axa_items", "created_at").await.unwrap();
    assert!(created.1, "{created:?}");
    assert_eq!(column(&db, "axa_items", "updated_at").await, Some(created));

    // A row that names neither timestamp gets the time of the insert.
    db.execute_unprepared(
        "INSERT INTO axa_items (pid, name, data)
         VALUES ('6f1c1d1e-0000-4000-8000-000000000001', 'stamped', '{}')",
    )
    .await
    .unwrap();
    assert_eq!(
        scalar::<i32>(
            &db,
            "SELECT created_at > datetime('now', '-1 minute')
                AND updated_at > datetime('now', '-1 minute')
             FROM axa_items WHERE name = 'stamped'",
        )
        .await,
        1
    );
    assert_eq!(
        scalar::<i32>(&db, "SELECT active FROM axa_items WHERE name = 'stamped'").await,
        1
    );
    db.execute_unprepared("DELETE FROM axa_items")
        .await
        .unwrap();
    assert_eq!(
        column(&db, "axa_items", "id").await,
        Some(("INTEGER".to_string(), true))
    );
    assert!(!column(&db, "axa_items", "deleted_at").await.unwrap().1);

    // References get a column with a predictable name. A `?` suffix makes the
    // reference optional, and its rows are kept, unlinked, when the parent goes.
    // SQLite keeps no name for a foreign key that sea-query creates.
    create_table(
        &m,
        "axa_notes",
        &[("id", ColType::PkAuto)],
        &[("axa_items", ""), ("axa_items?", "parent_id")],
    )
    .await
    .unwrap();
    assert!(column(&db, "axa_notes", "axa_item_id").await.unwrap().1);
    assert!(!column(&db, "axa_notes", "parent_id").await.unwrap().1);
    assert_eq!(
        foreign_keys(&db, "axa_notes").await,
        [
            ("axa_item_id".into(), "axa_items".into(), "CASCADE".into()),
            ("parent_id".into(), "axa_items".into(), "SET NULL".into()),
        ]
    );

    // A nullable column can be added and removed.
    add_column(&m, "axa_items", "note", ColType::TextNull)
        .await
        .unwrap();
    assert!(column(&db, "axa_items", "note").await.is_some());
    remove_column(&m, "axa_items", "note").await.unwrap();
    assert!(column(&db, "axa_items", "note").await.is_none());

    // SQLite refuses, with an error rather than a panic, a unique column, and a
    // required column with no default once the table has rows.
    assert!(
        add_column(&m, "axa_items", "code", ColType::StringUniq)
            .await
            .is_err()
    );
    assert!(column(&db, "axa_items", "code").await.is_none());
    db.execute_unprepared(
        "INSERT INTO axa_items (pid, name, data)
         VALUES ('6f1c1d1e-0000-4000-8000-000000000002', 'present', '{}')",
    )
    .await
    .unwrap();
    assert!(
        add_column(&m, "axa_items", "title", ColType::String)
            .await
            .is_err()
    );
    assert!(column(&db, "axa_items", "title").await.is_none());

    drop_table(&m, "axa_notes").await.unwrap();
    drop_table(&m, "axa_items").await.unwrap();
    assert!(column(&db, "axa_items", "id").await.is_none());
}

// The functions every database crate has.

mod shared {
    use axa_sqlite_seaorm::{
        Pool, dangerously_recreate, is_unique_violation, migrate, pending, ping, revert,
        sea_orm::ConnectionTrait,
        sea_orm_migration::{
            MigrationName, MigrationTrait, MigratorTrait, SchemaManager, async_trait,
            sea_orm::DbErr,
        },
        truncate,
    };

    use super::{database, scalar};

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
                    "CREATE TABLE seaorm_people
                     (id INTEGER PRIMARY KEY AUTOINCREMENT, email TEXT UNIQUE)",
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
                .execute_unprepared(
                    "CREATE TABLE seaorm_notes (id INTEGER PRIMARY KEY,
                     person_id INTEGER NOT NULL REFERENCES seaorm_people (id))",
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

    const INSERT_PERSON: &str = "INSERT INTO seaorm_people (email) VALUES ('a@example.org')";
    const INSERT_NOTE: &str = "INSERT INTO seaorm_notes (person_id) VALUES (1)";

    async fn count(db: &Pool, table: &str) -> i32 {
        scalar(db, &format!("SELECT count(*) FROM {table}")).await
    }

    #[tokio::test]
    async fn migrations_apply_revert_and_list_what_is_pending() {
        let db = database("migrations").await;
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
    async fn a_duplicate_is_a_unique_violation() {
        let db = database("unique").await;
        migrate::<Migrator>(&db).await.unwrap();
        db.execute_unprepared(INSERT_PERSON).await.unwrap();
        let error = db.execute_unprepared(INSERT_PERSON).await.unwrap_err();
        assert!(is_unique_violation(&error), "{error}");
        let error = db
            .execute_unprepared("INSERT INTO seaorm_people (id) VALUES (1)")
            .await
            .unwrap_err();
        assert!(is_unique_violation(&error), "{error}");
        let error = db
            .execute_unprepared("INSERT INTO seaorm_notes (person_id) VALUES (99)")
            .await
            .unwrap_err();
        assert!(!is_unique_violation(&error), "{error}");
    }

    #[tokio::test]
    async fn truncate_refuses_an_unsafe_name_before_emptying_anything() {
        let db = database("truncate-unsafe").await;
        migrate::<Migrator>(&db).await.unwrap();
        db.execute_unprepared(INSERT_PERSON).await.unwrap();
        assert!(
            truncate(&db, &["seaorm_people", "seaorm_people; DROP TABLE x"])
                .await
                .is_err()
        );
        assert_eq!(count(&db, "seaorm_people").await, 1);
    }

    #[tokio::test]
    async fn truncate_empties_tables_in_any_order_and_restarts_their_ids() {
        let db = database("truncate").await;
        migrate::<Migrator>(&db).await.unwrap();
        db.execute_unprepared(INSERT_PERSON).await.unwrap();
        db.execute_unprepared(INSERT_NOTE).await.unwrap();

        // The parent alone is refused, because a note still refers to it.
        assert!(truncate(&db, &["seaorm_people"]).await.is_err());
        assert_eq!(count(&db, "seaorm_people").await, 1);

        // Parent first is fine when the child is emptied too.
        truncate(&db, &["seaorm_people", "seaorm_notes"])
            .await
            .unwrap();
        assert_eq!(count(&db, "seaorm_people").await, 0);
        assert_eq!(count(&db, "seaorm_notes").await, 0);
        db.execute_unprepared(INSERT_PERSON).await.unwrap();
        assert_eq!(scalar::<i32>(&db, "SELECT id FROM seaorm_people").await, 1);
    }

    #[tokio::test]
    async fn dangerously_recreate_rebuilds_from_the_first_migration() {
        let db = database("recreate").await;
        migrate::<Migrator>(&db).await.unwrap();
        db.execute_unprepared(INSERT_PERSON).await.unwrap();
        db.execute_unprepared(INSERT_NOTE).await.unwrap();
        db.execute_unprepared("CREATE TABLE stray (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();
        dangerously_recreate::<Migrator>(&db).await.unwrap();
        assert_eq!(count(&db, "seaorm_people").await, 0);
        assert_eq!(
            scalar::<i32>(
                &db,
                "SELECT count(*) FROM sqlite_schema WHERE name = 'stray'"
            )
            .await,
            0
        );
        db.execute_unprepared(INSERT_PERSON).await.unwrap();
        assert_eq!(
            pending::<Migrator>(&db).await.unwrap(),
            Vec::<String>::new()
        );
        // Foreign keys are on again.
        let error = db
            .execute_unprepared("INSERT INTO seaorm_notes (person_id) VALUES (99)")
            .await;
        assert!(error.is_err());
    }
}
