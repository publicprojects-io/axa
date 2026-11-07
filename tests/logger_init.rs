//! `logger::init` as a process sees it: LOG-4, colour goes to a terminal only, and
//! CFG-16, settings warnings go to stderr.
//!
//! `logger::init` installs a global subscriber once per process, so each case runs in
//! a child: run by `cargo test` a test is the parent, which starts this same binary
//! again with [`ROLE`] set and its stdout and stderr pipes, as a log file or a
//! pipeline would be.

use std::{
    io::Write,
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex},
};

use axa::{
    config::Logger,
    logger::{self, Format, LogLevel},
};
use tracing_subscriber::{Registry, fmt::MakeWriter, layer::SubscriberExt};

/// Being set makes a process the child that logs.
const ROLE: &str = "AXA_LOG_COLOUR_TEST_ROLE";
/// What the child logs, so the parent can tell its line from the harness's.
const MESSAGE: &str = "a line for a pipe";
/// The start of every ANSI escape sequence.
const ESCAPE: char = '\u{1b}';

/// Collects what a layer writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for Captured {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

/// Run `test` again as the child, with its output piped.
fn child(test: &str) -> Output {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture"])
        .env(ROLE, "child")
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(output.status.success(), "the child failed: {output:?}");
    output
}

/// Settings as `Logger::from_env` would give them.
fn settings(enable: bool, level: LogLevel, warnings: &[&str]) -> Logger {
    Logger {
        enable,
        level,
        format: Format::Compact,
        override_filter: None,
        warnings: warnings.iter().map(ToString::to_string).collect(),
    }
}

/// What one event looks like through `init_layer`.
fn formatted(format: Format, ansi: bool) -> String {
    let captured = Captured::default();
    let subscriber = Registry::default().with(logger::init_layer(captured.clone(), &format, ansi));
    tracing::subscriber::with_default(subscriber, || tracing::warn!(id = 7, "{MESSAGE}"));
    captured.text()
}

#[test]
fn logs_to_a_pipe_carry_no_colour() {
    if std::env::var_os(ROLE).is_some() {
        logger::init("log-colour", &settings(true, LogLevel::Info, &[]));
        tracing::warn!(target: "log_colour", "{MESSAGE}");
        return;
    }
    let output = child("logs_to_a_pipe_carry_no_colour");
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.contains(MESSAGE))
        .unwrap_or_else(|| panic!("the child logged nothing: {text}"));
    assert!(!line.contains(ESCAPE), "colour reached a pipe: {line:?}");
}

/// A warning is about the settings, so neither a level nor turning logging off can
/// hide it, and it stays off stdout, which may be a log pipeline.
#[test]
fn settings_warnings_go_to_stderr_whatever_the_logging() {
    const WARNING: &str = "HTTP_HSTS is not a boolean";
    if std::env::var_os(ROLE).is_some() {
        logger::init("log-warnings", &settings(false, LogLevel::Off, &[WARNING]));
        return;
    }
    let output = child("settings_warnings_go_to_stderr_whatever_the_logging");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&format!("warning: {WARNING}")), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(WARNING), "{stdout}");
}

/// The piped test passes only because colour was turned off, not because the
/// formatter never colours: given a terminal, it does.
#[test]
fn colour_once_turned_on_shows() {
    for format in [Format::Compact, Format::Pretty] {
        assert!(formatted(format, true).contains(ESCAPE), "{format:?}");
        let plain = formatted(format, false);
        assert!(plain.contains(MESSAGE), "{plain}");
        assert!(!plain.contains(ESCAPE), "{format:?}: {plain:?}");
    }
}
