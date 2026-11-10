//! Against a real Postgres. The tests share one database, so run them with
//! `--test-threads=1`, on a database this suite may wipe.
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test -p axa-postgres-toasty -- --ignored --test-threads=1
//! ```

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
