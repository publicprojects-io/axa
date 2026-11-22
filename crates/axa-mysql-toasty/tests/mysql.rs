#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "RCA-SCOPE-4: tests may unwrap"
)]
//! Against a real MySQL. The tests share one database, so run them with
//! `--test-threads=1`, on a database this suite may wipe.
//!
//! ```sh
//! AXA_TEST_MYSQL_URL=mysql://... cargo test -p axa-mysql-toasty -- --ignored --test-threads=1
//! ```

#![allow(
    clippy::used_underscore_items,
    reason = "the toasty::Model derive generates underscore items"
)]

use axa_mysql_toasty::{
    PoolConfig, connect, dangerously_recreate, error_number, is_unique_violation, migrate, pending,
    ping, sql_state, truncate,
};
use toasty::migration::{MigrationFile, MigrationSet};
use toasty_core::stmt::{Type, Value};

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
        "CREATE TABLE toasty_people (
            id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
            email VARCHAR(255) NOT NULL UNIQUE
        )",
    ),
    MigrationFile::new(
        2,
        "0002_notes",
        "CREATE TABLE toasty_notes (
            id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
            person_id BIGINT UNSIGNED NOT NULL,
            FOREIGN KEY (person_id) REFERENCES toasty_people (id)
        )",
    ),
]);

fn uri() -> String {
    std::env::var("AXA_TEST_MYSQL_URL").expect("AXA_TEST_MYSQL_URL")
}

/// One connection, so that a session setting left behind would be seen.
async fn fresh() -> axa_mysql_toasty::Pool {
    let config = PoolConfig {
        max_connections: 1,
        ..PoolConfig::new(uri())
    };
    let db = connect(&config, toasty::models!(Person))
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

async fn scalar(db: &axa_mysql_toasty::Pool, sql: &str) -> i64 {
    let mut handle = db.clone();
    let rows = toasty::sql::query(sql)
        .column_types([Type::I64])
        .exec(&mut handle)
        .await
        .unwrap();
    match rows.first() {
        Some(Value::Record(record)) => match record.fields.first() {
            Some(Value::I64(value)) => *value,
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn connect_checks_the_database_answers() {
    let db = fresh().await;
    ping(&db).await.unwrap();
    let unreachable = PoolConfig {
        connect_timeout_ms: 500,
        ..PoolConfig::new("mysql://axa:axa@127.0.0.1:1/none")
    };
    assert!(
        connect(&unreachable, toasty::models!(Person))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
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
#[ignore = "needs AXA_TEST_MYSQL_URL"]
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
    assert_eq!(Person::all().exec(&mut db).await.unwrap().len(), 1);
    // toasty_people is referenced by toasty_notes, which MySQL refuses to
    // truncate with foreign key checks on.
    truncate(&db, &["toasty_people", "toasty_notes"])
        .await
        .unwrap();
    assert!(Person::all().exec(&mut db).await.unwrap().is_empty());
    assert_eq!(
        scalar(&db, "SELECT CAST(@@FOREIGN_KEY_CHECKS AS SIGNED)").await,
        1,
        "foreign key checks are back on"
    );

    // A table that is not there fails the truncate part way, and the one connection
    // goes back to the pool with foreign key checks on all the same.
    toasty::create!(Person {
        email: "b@example.org"
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert!(
        truncate(&db, &["toasty_people", "toasty_missing", "toasty_notes"])
            .await
            .is_err()
    );
    assert_eq!(
        scalar(&db, "SELECT CAST(@@FOREIGN_KEY_CHECKS AS SIGNED)").await,
        1,
        "back on after a failure"
    );
    assert!(
        Person::all().exec(&mut db).await.unwrap().is_empty(),
        "what came before the failure was emptied"
    );
    truncate::<&str>(&db, &[]).await.unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
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
    assert_eq!(error_number(&error), Some(1062), "{error}");
    assert_eq!(sql_state(&error), Some("23000"), "{error}");
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL and drops every table in it"]
async fn dangerously_recreate_rebuilds_from_the_first_migration() {
    let mut db = fresh().await;
    migrate(&db, &MIGRATIONS).await.unwrap();
    toasty::create!(Person {
        email: "a@example.org"
    })
    .exec(&mut db)
    .await
    .unwrap();
    toasty::sql::statement("CREATE TABLE toasty_stray (id INT PRIMARY KEY)")
        .exec(&mut db)
        .await
        .unwrap();
    dangerously_recreate(&db, &MIGRATIONS).await.unwrap();
    assert!(Person::all().exec(&mut db).await.unwrap().is_empty());
    assert_eq!(
        pending(&db, &MIGRATIONS).await.unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT CAST(COUNT(*) AS SIGNED) FROM information_schema.tables
             WHERE table_schema = DATABASE() AND table_name = 'toasty_stray'"
        )
        .await,
        0,
        "a table no migration makes is gone"
    );
    assert_eq!(
        scalar(&db, "SELECT CAST(@@FOREIGN_KEY_CHECKS AS SIGNED)").await,
        1,
        "foreign key checks are back on"
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL, on a server with TLS"]
async fn ssl_mode_in_the_url_asks_for_tls() {
    let with = |mode: &str| {
        let base = uri();
        let joiner = if base.contains('?') { '&' } else { '?' };
        PoolConfig::new(format!("{base}{joiner}ssl-mode={mode}"))
    };
    let db = connect(&with("REQUIRED"), toasty::models!(Person))
        .await
        .expect("connect over TLS");
    let mut handle = db.clone();
    let rows = toasty::sql::query("SHOW SESSION STATUS LIKE 'Ssl_cipher'")
        .column_types([Type::String, Type::String])
        .exec(&mut handle)
        .await
        .unwrap();
    match rows.first() {
        Some(Value::Record(record)) => match record.fields.get(1) {
            Some(Value::String(cipher)) => assert!(!cipher.is_empty(), "no TLS cipher"),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
    // A test server's certificate is not one a public root signs.
    assert!(
        connect(&with("VERIFY_CA"), toasty::models!(Person))
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn every_connection_starts_with_the_session_settings() {
    let mut writer = fresh().await;
    for statement in [
        "DROP TABLE IF EXISTS axa_read_only",
        "CREATE TABLE axa_read_only (x INT)",
    ] {
        toasty::sql::statement(statement)
            .exec(&mut writer)
            .await
            .unwrap();
    }
    let config = PoolConfig {
        statement_timeout_ms: 1234,
        read_only: true,
        ..PoolConfig::new(uri())
    };
    let db = connect(&config, toasty::models!(Person)).await.unwrap();
    // Every connection, not only the first: a transaction holds one, so hold two.
    let (mut one, mut two) = (db.clone(), db.clone());
    let mut first = one.transaction().await.unwrap();
    let mut second = two.transaction().await.unwrap();
    for transaction in [&mut first, &mut second] {
        let rows = toasty::sql::query(
            "SELECT CAST(@@SESSION.max_execution_time AS SIGNED), \
             CAST(@@SESSION.transaction_read_only AS SIGNED)",
        )
        .column_types([Type::I64, Type::I64])
        .exec(transaction)
        .await
        .unwrap();
        match rows.first() {
            Some(Value::Record(record)) => {
                assert_eq!(record.fields, [Value::I64(1234), Value::I64(1)]);
            }
            other => panic!("{other:?}"),
        }
        let refused = toasty::sql::statement("INSERT INTO axa_read_only VALUES (1)")
            .exec(transaction)
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("READ ONLY"), "{refused}");
    }
    drop((first, second));
    toasty::sql::statement("DROP TABLE axa_read_only")
        .exec(&mut writer)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs AXA_TEST_MYSQL_URL"]
async fn the_pool_hands_its_drivers_schema_and_url_on() {
    let mut db = fresh().await;
    assert_eq!(db.driver().url(), uri(), "the URL it was given");
    db.push_schema().await.unwrap();
    toasty::create!(Person {
        email: "a@example.org"
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(Person::all().exec(&mut db).await.unwrap().len(), 1);
}
