//! Capturing log output in tests.

use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use tracing_subscriber::fmt::{self, MakeWriter};

/// Collects what the formatter writes.
#[derive(Clone, Default)]
pub struct Lines(Arc<Mutex<Vec<u8>>>);

impl Lines {
    /// Everything written so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    /// A plain formatting layer writing here.
    pub fn layer<S>(&self) -> fmt::Layer<S, fmt::format::DefaultFields, fmt::format::Format, Self> {
        fmt::layer().with_ansi(false).with_writer(self.clone())
    }
}

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for Lines {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self {
        self.clone()
    }
}
