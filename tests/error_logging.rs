//! What errors write to the log. This has its own test binary for the reason given
//! in `request_logging.rs`: `tracing` caches per process whether a log site is on.

use std::sync::{Arc, Mutex};

use axa::{Error, error::ErrorDetail};
use axum::{http::StatusCode, response::IntoResponse};

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

#[test]
fn only_a_server_fault_is_an_error_and_a_clients_text_is_not_logged() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();
    let logs = || String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();

    let _ = Error::NotFound.into_response();
    let _ = Error::BadRequest("alice@example.org is taken".to_string()).into_response();
    let detail = ErrorDetail::new("conflict", "bob@example.org");
    let _ = Error::CustomError(StatusCode::CONFLICT, detail).into_response();
    assert_eq!(logs(), "");

    let _ = Error::Unauthorized("expired token".to_string()).into_response();
    assert!(logs().contains("WARN"), "{}", logs());
    assert!(logs().contains("expired token"), "{}", logs());

    let _ = Error::string("the database went away").into_response();
    assert!(logs().contains("ERROR"), "{}", logs());
    assert!(logs().contains("the database went away"), "{}", logs());

    let detail = ErrorDetail::new("broken", "a custom server fault");
    let _ = Error::CustomError(StatusCode::BAD_GATEWAY, detail).into_response();
    assert!(logs().contains("a custom server fault"), "{}", logs());

    let fault = RowFault {
        detail: "Key (email)=(carol@example.org) already exists.",
    };
    let _ = Error::wrap(fault).into_response();
    assert!(logs().contains("the insert failed"), "{}", logs());
    assert!(!logs().contains("carol@example.org"), "{}", logs());
}

/// Shaped like a database error: its `Debug` text quotes the row, and its message doesn't.
#[derive(Debug, thiserror::Error)]
#[error("the insert failed")]
struct RowFault {
    #[allow(dead_code)]
    detail: &'static str,
}
