//! Logs captured for one test, so it can check what a request wrote, and that it
//! wrote no secret.
//!
//! ```
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let logs = axa_test::logs::capture();
//! tracing::warn!(user = "ada", "sign-in refused");
//! assert!(logs.contains("sign-in refused"));
//! assert_eq!(logs.lines()[0]["fields"]["user"], "ada");
//! logs.assert_absent("hunter2");
//! # }
//! ```
//!
//! Capture holds on the thread that called [`capture`], until the [`Logs`] is dropped,
//! and covers `DEBUG` and above. A `#[tokio::test]` runs on one thread unless told
//! otherwise, so a request sent through [`crate::TestServer`] is captured. A task a
//! multi-threaded runtime moves to another thread is not.

use std::{
    io::Write,
    sync::{Arc, Mutex, PoisonError},
};

use tracing::subscriber::DefaultGuard;
use tracing_subscriber::fmt::MakeWriter;

/// What has been logged since [`capture`], one JSON object per line.
pub struct Logs {
    buffer: Arc<Mutex<Vec<u8>>>,
    _guard: DefaultGuard,
}

impl std::fmt::Debug for Logs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Logs")
            .field("lines", &self.lines().len())
            .finish()
    }
}

#[derive(Clone)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Buffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Start capturing this thread's logs, as JSON, until the answer is dropped.
#[must_use]
pub fn capture() -> Logs {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::DEBUG)
        .with_current_span(true)
        .with_span_list(false)
        .with_writer(Buffer(buffer.clone()))
        .finish();
    Logs {
        buffer,
        _guard: tracing::subscriber::set_default(subscriber),
    }
}

impl Logs {
    /// Everything logged so far, as written.
    #[must_use]
    pub fn text(&self) -> String {
        let buffer = self.buffer.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// Each line logged so far, parsed: `level`, `fields` with `message`, `target`,
    /// and `span` when there was one.
    ///
    /// # Panics
    ///
    /// If a line is not JSON, which the capture never writes.
    #[must_use]
    pub fn lines(&self) -> Vec<serde_json::Value> {
        self.text()
            .lines()
            .map(|line| serde_json::from_str(line).expect("each captured line is JSON"))
            .collect()
    }

    /// Whether any line holds `text`.
    #[must_use]
    pub fn contains(&self, text: &str) -> bool {
        self.text().contains(text)
    }

    /// Fail the test if any line holds `secret`, naming the line but not repeating
    /// the secret.
    ///
    /// # Panics
    ///
    /// If a line holds it.
    pub fn assert_absent(&self, secret: &str) {
        let text = self.text();
        if let Some(line) = text.lines().position(|line| line.contains(secret)) {
            panic!(
                "captured log line {} holds what must not be logged",
                line + 1
            );
        }
    }

    /// Forget what has been logged so far.
    pub fn clear(&self) {
        self.buffer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}
