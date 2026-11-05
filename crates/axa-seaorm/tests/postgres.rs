//! Behaviour against a real Postgres. Ignored by default, because these create
//! and drop tables in the database they are given. Run them with:
//!
//! ```sh
//! AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-seaorm -- --ignored --test-threads=1
//! ```

use axa_seaorm::{
    PoolConfig, connect,
    schema::{ColType, add_column, create_table, drop_table, remove_column},
    sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement},
    sea_orm_migration::SchemaManager,
};

async fn database() -> DatabaseConnection {
    let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
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
#[ignore = "needs AXA_TEST_DATABASE_URL and changes its schema"]
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
