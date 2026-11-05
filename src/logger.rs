//! Logging setup over `tracing-subscriber`.

use std::fmt;

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
        match name.trim().to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "warn" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Info,
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
        match name.trim().to_ascii_lowercase().as_str() {
            "json" => Self::Json,
            "pretty" => Self::Pretty,
            _ => Self::Compact,
        }
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
    EnvFilter::try_from_default_env()
        .or_else(|_| {
            override_filter.map_or_else(
                || {
                    let directives = DEFAULT_MODULES
                        .iter()
                        .copied()
                        .chain(std::iter::once(app_name))
                        .map(|module| format!("{module}={level}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    EnvFilter::try_new(directives)
                },
                EnvFilter::try_new,
            )
        })
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

/// Install plain stdout logging. A second call does nothing.
pub fn init(app_name: &str, config: &Logger) {
    if !config.enable {
        return;
    }
    let filter = init_env_filter(app_name, config.override_filter.as_ref(), &config.level);
    let _ = tracing_subscriber::registry()
        .with(init_layer(std::io::stdout, &config.format, true))
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
    }

    #[test]
    fn formats_parse() {
        assert_eq!(Format::parse("JSON"), Format::Json);
        assert_eq!(Format::parse("pretty"), Format::Pretty);
        assert_eq!(Format::parse("nonsense"), Format::Compact);
    }

    #[test]
    fn default_filter_names_the_service_and_shared_modules() {
        let filter = init_env_filter("my_service", None, &LogLevel::Debug).to_string();
        assert!(filter.contains("my_service=debug"), "{filter}");
        assert!(filter.contains("tower_http=debug"), "{filter}");
    }

    #[test]
    fn an_override_replaces_the_default_filter() {
        let custom = "my_service=trace".to_string();
        let filter = init_env_filter("my_service", Some(&custom), &LogLevel::Info).to_string();
        assert_eq!(filter, custom);
    }
}
