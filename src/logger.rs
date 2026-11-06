//! Logging setup over `tracing-subscriber`.

use std::{fmt, io::IsTerminal};

use tracing_subscriber::{
    EnvFilter, Layer, Registry, fmt as tracing_fmt, fmt::MakeWriter, layer::SubscriberExt,
    util::SubscriberInitExt,
};

use crate::config::Logger;

/// Crates whose events are shown by default, besides the service itself.
const DEFAULT_MODULES: &[&str] = &["axa", "sea_orm_migration", "tower_http", "sqlx::query"];

/// Minimum log level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Nothing.
    Off,
    /// Everything.
    Trace,
    /// Debug and above.
    Debug,
    /// Info and above.
    Info,
    /// Warnings and errors.
    Warn,
    /// Errors only.
    Error,
}

impl LogLevel {
    /// Parse a level name, defaulting to `Info`.
    #[must_use]
    pub fn parse(name: &str) -> Self {
        Self::from_name(name).unwrap_or(Self::Info)
    }

    /// The level with this name, if there is one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        })
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// One line per event.
    #[default]
    Compact,
    /// Multi-line and readable.
    Pretty,
    /// JSON, for log pipelines.
    Json,
}

impl Format {
    /// Parse a format name, defaulting to `Compact`.
    #[must_use]
    pub fn parse(name: &str) -> Self {
        Self::from_name(name).unwrap_or(Self::Compact)
    }

    /// The format with this name, if there is one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "compact" => Some(Self::Compact),
            "json" => Some(Self::Json),
            "pretty" => Some(Self::Pretty),
            _ => None,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Compact => "compact",
            Self::Pretty => "pretty",
            Self::Json => "json",
        })
    }
}

/// Build the filter: `RUST_LOG` wins, then `override_filter`, then the
/// default modules plus the service at `level`.
///
/// # Panics
///
/// If `override_filter` is not a valid filter string.
#[must_use]
pub fn init_env_filter(
    app_name: &str,
    override_filter: Option<&String>,
    level: &LogLevel,
) -> EnvFilter {
    let rust_log = std::env::var(EnvFilter::DEFAULT_ENV).ok();
    choose_filter(rust_log.as_deref(), app_name, override_filter, *level)
}

/// The choice behind [`init_env_filter`], given the value of `RUST_LOG`. A blank
/// or invalid `RUST_LOG` is passed over.
fn choose_filter(
    rust_log: Option<&str>,
    app_name: &str,
    override_filter: Option<&String>,
    level: LogLevel,
) -> EnvFilter {
    let from_rust_log = rust_log
        .filter(|filter| !filter.trim().is_empty())
        .and_then(|filter| EnvFilter::try_new(filter).ok());
    if let Some(filter) = from_rust_log {
        return filter;
    }
    override_filter
        .map_or_else(
            || {
                // A log target is a module path, so `case-folder` logs as `case_folder`.
                let app_name = app_name.replace('-', "_");
                let directives = DEFAULT_MODULES
                    .iter()
                    .copied()
                    .chain(std::iter::once(app_name.as_str()))
                    .map(|module| format!("{module}={level}"))
                    .collect::<Vec<_>>()
                    .join(",");
                EnvFilter::try_new(directives)
            },
            EnvFilter::try_new,
        )
        .expect("logger filter is invalid")
}

/// Build a formatting layer writing to `writer`.
pub fn init_layer<W>(
    writer: W,
    format: &Format,
    ansi: bool,
) -> Box<dyn Layer<Registry> + Send + Sync>
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    let layer = tracing_fmt::Layer::default()
        .with_ansi(ansi)
        .with_writer(writer);
    match format {
        Format::Compact => layer.compact().boxed(),
        Format::Pretty => layer.pretty().boxed(),
        Format::Json => layer.json().boxed(),
    }
}

/// Install plain stdout logging. A second call does nothing. Colour is for a
/// terminal only, so a log file or pipeline gets no escape codes.
///
/// Settings are read before logging can start, so this also writes out
/// [`Logger::warnings`]. They go to stderr, so that neither a log level nor
/// `LOG_ENABLE=false` can hide them.
pub fn init(app_name: &str, config: &Logger) {
    for warning in &config.warnings {
        eprintln!("warning: {warning}");
    }
    if !config.enable {
        return;
    }
    let filter = init_env_filter(app_name, config.override_filter.as_ref(), &config.level);
    let _ = tracing_subscriber::registry()
        .with(init_layer(
            std::io::stdout,
            &config.format,
            std::io::stdout().is_terminal(),
        ))
        .with(filter)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_and_print() {
        for name in ["off", "trace", "debug", "info", "warn", "error"] {
            assert_eq!(LogLevel::parse(name).to_string(), name);
        }
        assert_eq!(LogLevel::parse("nonsense"), LogLevel::Info);
        assert_eq!(LogLevel::from_name("nonsense"), None);
    }

    #[test]
    fn formats_parse() {
        assert_eq!(Format::parse("JSON"), Format::Json);
        assert_eq!(Format::parse("pretty"), Format::Pretty);
        assert_eq!(Format::parse("nonsense"), Format::Compact);
        assert_eq!(Format::from_name("nonsense"), None);
        for name in ["compact", "pretty", "json"] {
            assert_eq!(Format::parse(name).to_string(), name);
        }
    }

    #[test]
    fn rust_log_wins_over_the_override_and_the_level() {
        let filter = choose_filter(
            Some("from_rust_log=trace"),
            "my_service",
            Some(&"from_override=warn".to_string()),
            LogLevel::Debug,
        )
        .to_string();
        assert_eq!(filter, "from_rust_log=trace");
    }

    #[test]
    fn a_blank_or_invalid_rust_log_is_passed_over() {
        for value in ["", "  ", "a=b=c=d"] {
            let filter =
                choose_filter(Some(value), "my_service", None, LogLevel::Debug).to_string();
            assert!(filter.contains("my_service=debug"), "{value:?}: {filter}");
        }
    }

    #[test]
    fn default_filter_names_the_service_and_shared_modules() {
        let filter = init_env_filter("my_service", None, &LogLevel::Debug).to_string();
        assert!(filter.contains("my_service=debug"), "{filter}");
        assert!(filter.contains("tower_http=debug"), "{filter}");
    }

    #[test]
    fn a_package_name_with_a_hyphen_still_shows_its_logs() {
        let filter = init_env_filter("case-folder", None, &LogLevel::Info).to_string();
        assert!(filter.contains("case_folder=info"), "{filter}");
        assert!(!filter.contains("case-folder"), "{filter}");
    }

    #[test]
    fn an_override_replaces_the_default_filter() {
        let custom = "my_service=trace".to_string();
        let filter = init_env_filter("my_service", Some(&custom), &LogLevel::Info).to_string();
        assert_eq!(filter, custom);
    }
}
