//! Logging setup over `tracing-subscriber`.

use std::{fmt, io::IsTerminal};

use tracing_subscriber::{
    EnvFilter, Layer, Registry, fmt as tracing_fmt, fmt::MakeWriter, layer::SubscriberExt,
    util::SubscriberInitExt,
};

use crate::config::Logger;

/// Crates whose events are shown by default, besides the service itself.
const DEFAULT_MODULES: &[&str] = &["axa", "sea_orm_migration", "tower_http", "sqlx::query"];

/// A logging layer, as [`init_layer`] returns and [`init_with`] takes.
pub type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

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
    choose_filter(rust_log.as_deref(), app_name, override_filter, *level, &[])
}

/// The choice behind [`init_env_filter`], given the value of `RUST_LOG`. A blank
/// or invalid `RUST_LOG` is passed over. `extra_directives` join the default filter
/// only, so that an operator's own filter stays exactly as they wrote it.
fn choose_filter(
    rust_log: Option<&str>,
    app_name: &str,
    override_filter: Option<&String>,
    level: LogLevel,
    extra_directives: &[&str],
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
                    .chain(extra_directives.iter().map(ToString::to_string))
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
    init_with(app_name, config, Vec::new(), &[]);
}

/// [`init`], with more layers beside the stdout logs, such as an exporter.
///
/// `layers` see what the filter lets through, as the logs do, and they keep running
/// when `LOG_ENABLE=false` turns off stdout. `extra_directives` widen the default
/// filter, for example to show an exporter's own errors, and are left out when
/// `RUST_LOG` or an override filter is set.
///
/// # Panics
///
/// If an override filter or an extra directive is not valid.
pub fn init_with(
    app_name: &str,
    config: &Logger,
    layers: Vec<BoxedLayer>,
    extra_directives: &[&str],
) {
    for warning in &config.warnings {
        eprintln!("warning: {warning}");
    }
    let layers = with_stdout(layers, config);
    if layers.is_empty() {
        return;
    }
    let rust_log = std::env::var(EnvFilter::DEFAULT_ENV).ok();
    let filter = choose_filter(
        rust_log.as_deref(),
        app_name,
        config.override_filter.as_ref(),
        config.level,
        extra_directives,
    );
    let _ = subscriber(layers, filter).try_init();
}

/// The stdout layer, when logging is on, ahead of the others.
fn with_stdout(mut layers: Vec<BoxedLayer>, config: &Logger) -> Vec<BoxedLayer> {
    if config.enable {
        let stdout = init_layer(
            std::io::stdout,
            &config.format,
            std::io::stdout().is_terminal(),
        );
        layers.insert(0, stdout);
    }
    layers
}

/// The layers under one filter, in the order [`init_with`] installs them.
fn subscriber(
    layers: Vec<BoxedLayer>,
    filter: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::registry().with(layers).with(filter)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

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
            &["extra=warn"],
        )
        .to_string();
        assert_eq!(filter, "from_rust_log=trace");
    }

    #[test]
    fn a_blank_or_invalid_rust_log_is_passed_over() {
        for value in ["", "  ", "a=b=c=d"] {
            let filter =
                choose_filter(Some(value), "my_service", None, LogLevel::Debug, &[]).to_string();
            assert!(filter.contains("my_service=debug"), "{value:?}: {filter}");
        }
    }

    #[test]
    fn extra_directives_widen_the_default_filter_only() {
        let default = choose_filter(None, "my_service", None, LogLevel::Info, &["extra=warn"]);
        assert!(default.to_string().contains("extra=warn"), "{default}");

        let custom = "my_service=trace".to_string();
        let filter = choose_filter(
            None,
            "my_service",
            Some(&custom),
            LogLevel::Info,
            &["extra=warn"],
        );
        assert_eq!(filter.to_string(), custom);
    }

    /// Counts the events it is shown.
    #[derive(Clone, Default)]
    struct Events(Arc<AtomicUsize>);

    impl<S: tracing::Subscriber> Layer<S> for Events {
        fn on_event(
            &self,
            _event: &tracing::Event<'_>,
            _context: tracing_subscriber::layer::Context<'_, S>,
        ) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn an_extra_layer_sees_what_the_filter_lets_through() {
        let events = Events::default();
        let filter = EnvFilter::new("shown=info");
        let subscriber = subscriber(vec![events.clone().boxed()], filter);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "shown", "yes");
            tracing::info!(target: "hidden", "no");
        });
        assert_eq!(events.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn extra_layers_keep_running_with_logging_off() {
        let config = |enable| Logger {
            enable,
            level: LogLevel::Info,
            format: Format::Compact,
            override_filter: None,
            warnings: Vec::new(),
        };
        let extra = || vec![Events::default().boxed()];
        assert_eq!(with_stdout(extra(), &config(true)).len(), 2);
        assert_eq!(with_stdout(extra(), &config(false)).len(), 1);
        assert!(with_stdout(Vec::new(), &config(false)).is_empty());
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
