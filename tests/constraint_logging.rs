//! That a violated constraint is logged as a client's mistake. This has its own test
//! binary for the reason given in `request_logging.rs`.

#![cfg(any(feature = "queue", feature = "sqlx"))]

use std::sync::{Arc, Mutex};

use axa::Error;
use axum::response::IntoResponse;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn a_violated_constraint_logs_nothing_at_error() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();
    let url = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let mut setup = pool.begin().await.unwrap();
    sqlx::query("CREATE TEMPORARY TABLE axa_logged (email TEXT UNIQUE)")
        .execute(&mut *setup)
        .await
        .unwrap();
    let insert = "INSERT INTO axa_logged VALUES ('carol@example.org')";
    sqlx::query(insert).execute(&mut *setup).await.unwrap();
    let error = sqlx::query(insert).execute(&mut *setup).await.unwrap_err();
    let _ = Error::from(error).into_response();
    let mut setup = pool.begin().await.unwrap();
    sqlx::query("CREATE TEMPORARY TABLE axa_checked (sex TEXT CHECK (sex IN ('female')))")
        .execute(&mut *setup)
        .await
        .unwrap();
    let insert = "INSERT INTO axa_checked VALUES ('banana')";
    let error = sqlx::query(insert).execute(&mut *setup).await.unwrap_err();
    let _ = Error::from(error).into_response();
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs, "");
}
