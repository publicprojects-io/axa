//! Service configuration, from the process environment.
//!
//! There are no config files. A service states its defaults in code with
//! [`Defaults`], and the environment overrides them. Production has no
//! default database, so a missing `DATABASE_URL` fails at boot.

use crate::{Environment, Error, Result, logger, worker::WorkerMode};

/// The default port: a=1, x=24, a=1.
pub const DEFAULT_PORT: u16 = 1241;

/// One megabyte, as a body limit.
pub const MB: usize = 1_000_000;

/// A service's own defaults.
#[derive(Debug, Clone)]
pub struct Defaults {
    /// Database name stem: `person_service` gives `person_service_test`.
    pub database: &'static str,
    /// Port when `PORT` is unset.
    pub port: u16,
    /// Port when `PORT` is unset in production, if different.
    pub production_port: Option<u16>,
    /// Largest accepted request body, in bytes.
    pub body_limit: usize,
    /// Pool size outside production.
    pub max_connections: u32,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            database: "app",
            port: DEFAULT_PORT,
            production_port: None,
            body_limit: 2 * MB,
            max_connections: 10,
        }
    }
}

/// HTTP server settings.
#[derive(Debug, Clone)]
pub struct Server {
    /// Listening port.
    pub port: u16,
    /// Listening address.
    pub binding: String,
    /// Public base URL.
    pub host: String,
}

/// Logging settings.
#[derive(Debug, Clone)]
pub struct Logger {
    /// Whether to log to stdout.
    pub enable: bool,
    /// Minimum level.
    pub level: logger::LogLevel,
    /// Output format.
    pub format: logger::Format,
    /// A full `EnvFilter` string that replaces the default filter.
    pub override_filter: Option<String>,
}

/// Database settings.
#[derive(Debug, Clone)]
pub struct Database {
    /// Connection string. Empty for a service with no database.
    pub uri: String,
    /// Smallest pool size.
    pub min_connections: u32,
    /// Largest pool size.
    pub max_connections: u32,
    /// Connection timeout, in milliseconds.
    pub connect_timeout: u64,
    /// Idle timeout, in milliseconds.
    pub idle_timeout: u64,
    /// Log every statement.
    pub enable_logging: bool,
    /// Run migrations at boot. Off in production.
    pub auto_migrate: bool,
    /// Empty the tables at boot. Only ever on under `test`.
    pub dangerously_truncate: bool,
}

/// HTTP middleware settings.
#[derive(Debug, Clone)]
pub struct Http {
    /// Largest accepted request body, in bytes.
    pub body_limit: usize,
    /// Gzip, Brotli, and Zstd response compression.
    pub compression: bool,
    /// Per-request timeout, in seconds. `0` turns it off.
    pub timeout_seconds: u64,
    /// Origins allowed by CORS. `["*"]` allows any and is refused in
    /// production-like environments. Empty sends no CORS headers.
    pub cors_origins: Vec<String>,
    /// Send `Strict-Transport-Security`.
    pub hsts: bool,
}

/// All service configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// HTTP server.
    pub server: Server,
    /// Logging.
    pub logger: Logger,
    /// Database.
    pub database: Database,
    /// HTTP middleware.
    pub http: Http,
    /// How background jobs run.
    pub workers: WorkerMode,
}

/// Typed reads over a lookup function, so parsing can be tested without
/// touching the process environment.
struct Vars<F> {
    lookup: F,
}

impl<F: Fn(&str) -> Option<String>> Vars<F> {
    /// The value of `name`, or `default` when unset.
    fn text(&self, name: &str, default: &str) -> String {
        (self.lookup)(name).unwrap_or_else(|| default.to_string())
    }

    /// The parsed value of `name`. An unset or unparsable value gives
    /// `default`, because a typo in a tuning knob should not stop a boot.
    fn parsed<T: std::str::FromStr>(&self, name: &str, default: T) -> T {
        (self.lookup)(name)
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    }

    /// The boolean value of `name`: `1`, `true`, `yes`, and `on` mean true,
    /// and any other set value means false.
    fn flag(&self, name: &str, default: bool) -> bool {
        (self.lookup)(name).map_or(default, |value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    }
}

impl Config {
    /// Build configuration for `environment`, reading overrides from the
    /// process environment. See [`Config::from_lookup`].
    ///
    /// # Errors
    ///
    /// Fails when production has no `DATABASE_URL` and the service has a
    /// database, or when `*` is given as a CORS origin in production.
    pub fn from_env(environment: &Environment, defaults: &Defaults) -> Result<Self> {
        Self::from_lookup(environment, defaults, |name| std::env::var(name).ok())
    }

    /// Build configuration for `environment`, reading overrides through
    /// `lookup`.
    ///
    /// # Examples
    ///
    /// ```
    /// use axa::config::{Config, Defaults};
    /// use axa::Environment;
    ///
    /// let config = Config::from_lookup(
    ///     &Environment::Development,
    ///     &Defaults::default(),
    ///     |name| (name == "PORT").then(|| "8000".to_string()),
    /// )
    /// .unwrap();
    /// assert_eq!(config.server.port, 8000);
    /// ```
    ///
    /// # Errors
    ///
    /// Fails when production has no `DATABASE_URL` and the service has a
    /// database, or when `*` is given as a CORS origin in production.
    pub fn from_lookup(
        environment: &Environment,
        defaults: &Defaults,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let vars = Vars { lookup };
        let production = environment.is_production_like();
        let test = *environment == Environment::Test;

        // Fail closed: a production service must never fall back to a guessed
        // local database.
        let uri = match (vars.lookup)("DATABASE_URL") {
            Some(uri) => uri,
            None if defaults.database.is_empty() => String::new(),
            None if production => {
                return Err(Error::string("DATABASE_URL is required in production"));
            }
            None => format!(
                "postgres://loco:loco@localhost:5432/{}_{environment}",
                defaults.database
            ),
        };

        // Development serves browser dev servers on other ports, so it allows
        // any origin. Production sends no CORS headers until origins are listed.
        let cors_origins: Vec<String> = match (vars.lookup)("CORS_ALLOWED_ORIGINS") {
            Some(list) => list
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(str::to_string)
                .collect(),
            None if production => Vec::new(),
            None => vec!["*".to_string()],
        };
        if production && cors_origins.iter().any(|origin| origin == "*") {
            return Err(Error::string(
                "CORS_ALLOWED_ORIGINS must list origins in production, not *",
            ));
        }

        let level = if test {
            "warn"
        } else if production {
            "info"
        } else {
            "debug"
        };
        let workers = if test {
            WorkerMode::ForegroundBlocking
        } else {
            WorkerMode::BackgroundQueue
        };

        Ok(Self {
            server: Server {
                port: vars.parsed(
                    "PORT",
                    defaults
                        .production_port
                        .filter(|_| production)
                        .unwrap_or(defaults.port),
                ),
                binding: vars.text("BINDING", if production { "0.0.0.0" } else { "localhost" }),
                host: vars.text("HOST", "http://localhost"),
            },
            logger: Logger {
                enable: vars.flag("LOG_ENABLE", !test),
                level: logger::LogLevel::parse(&vars.text("LOG_LEVEL", level)),
                format: logger::Format::parse(
                    &vars.text("LOG_FORMAT", if production { "json" } else { "compact" }),
                ),
                override_filter: (vars.lookup)("LOG_FILTER"),
            },
            database: Database {
                uri,
                min_connections: vars.parsed("DB_MIN_CONNECTIONS", if production { 2 } else { 1 }),
                max_connections: vars.parsed(
                    "DB_MAX_CONNECTIONS",
                    if production {
                        20
                    } else {
                        defaults.max_connections
                    },
                ),
                connect_timeout: vars.parsed("DB_CONNECT_TIMEOUT", 500),
                idle_timeout: vars.parsed("DB_IDLE_TIMEOUT", 500),
                enable_logging: vars.flag("DB_LOGGING", false),
                auto_migrate: vars.flag("DB_AUTO_MIGRATE", !production),
                dangerously_truncate: test,
            },
            http: Http {
                body_limit: vars.parsed("BODY_LIMIT_BYTES", defaults.body_limit),
                compression: vars.flag("HTTP_COMPRESSION", production),
                timeout_seconds: vars
                    .parsed("REQUEST_TIMEOUT_SECONDS", if production { 60 } else { 0 }),
                cors_origins,
                hsts: vars.flag("HTTP_HSTS", production),
            },
            workers: match (vars.lookup)("WORKER_MODE").as_deref() {
                Some("ForegroundBlocking") => WorkerMode::ForegroundBlocking,
                Some("BackgroundAsync") => WorkerMode::BackgroundAsync,
                Some("BackgroundQueue") => WorkerMode::BackgroundQueue,
                _ => workers,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn with(pairs: &[(&str, &str)], environment: &Environment) -> Result<Config> {
        let vars: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let defaults = Defaults {
            database: "svc",
            ..Defaults::default()
        };
        Config::from_lookup(environment, &defaults, |name| vars.get(name).cloned())
    }

    #[test]
    fn default_port_is_1241() {
        assert_eq!(Defaults::default().port, 1241);
        assert_eq!(DEFAULT_PORT, 1241);
    }

    #[test]
    fn development_and_test_get_a_local_database_named_for_the_environment() {
        let dev = with(&[], &Environment::Development).unwrap();
        assert!(dev.database.uri.ends_with("/svc_development"));
        let test = with(&[], &Environment::Test).unwrap();
        assert!(test.database.uri.ends_with("/svc_test"));
    }

    #[test]
    fn production_refuses_to_boot_without_a_database_url() {
        let error = with(&[], &Environment::Production).unwrap_err();
        assert!(error.to_string().contains("DATABASE_URL"));
    }

    #[test]
    fn a_service_with_no_database_boots_in_production_without_one() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Production, &defaults, |_| None).unwrap();
        assert_eq!(config.database.uri, "");
    }

    #[test]
    fn only_tests_ever_truncate() {
        let url = [("DATABASE_URL", "postgres://x")];
        assert!(
            with(&[], &Environment::Test)
                .unwrap()
                .database
                .dangerously_truncate
        );
        assert!(
            !with(&[], &Environment::Development)
                .unwrap()
                .database
                .dangerously_truncate
        );
        assert!(
            !with(&url, &Environment::Production)
                .unwrap()
                .database
                .dangerously_truncate
        );
    }

    #[test]
    fn production_does_not_migrate_at_boot_by_default() {
        let config = with(
            &[("DATABASE_URL", "postgres://x")],
            &Environment::Production,
        )
        .unwrap();
        assert!(!config.database.auto_migrate);
    }

    #[test]
    fn production_refuses_a_wildcard_cors_origin() {
        let pairs = [
            ("DATABASE_URL", "postgres://x"),
            ("CORS_ALLOWED_ORIGINS", "*"),
        ];
        let error = with(&pairs, &Environment::Production).unwrap_err();
        assert!(error.to_string().contains("CORS_ALLOWED_ORIGINS"));
    }

    #[test]
    fn production_sends_no_cors_headers_until_origins_are_listed() {
        let url = ("DATABASE_URL", "postgres://x");
        let none = with(&[url], &Environment::Production).unwrap();
        assert_eq!(none.http.cors_origins, Vec::<String>::new());
        let listed = with(
            &[
                url,
                (
                    "CORS_ALLOWED_ORIGINS",
                    " https://a.example , https://b.example ,",
                ),
            ],
            &Environment::Production,
        )
        .unwrap();
        assert_eq!(
            listed.http.cors_origins,
            ["https://a.example", "https://b.example"]
        );
    }

    #[test]
    fn production_port_applies_only_in_production() {
        let defaults = Defaults {
            port: 5150,
            production_port: Some(8080),
            database: "svc",
            ..Defaults::default()
        };
        let lookup = |name: &str| (name == "DATABASE_URL").then(|| "postgres://x".to_string());
        let dev = Config::from_lookup(&Environment::Development, &defaults, lookup).unwrap();
        assert_eq!(dev.server.port, 5150);
        let prod = Config::from_lookup(&Environment::Production, &defaults, lookup).unwrap();
        assert_eq!(prod.server.port, 8080);
    }

    #[test]
    fn an_unparsable_number_falls_back_to_the_default() {
        let config = with(&[("PORT", "not-a-port")], &Environment::Development).unwrap();
        assert_eq!(config.server.port, DEFAULT_PORT);
    }

    #[test]
    fn flags_accept_the_usual_spellings() {
        for value in ["1", "true", "YES", " on "] {
            let config = with(&[("HTTP_COMPRESSION", value)], &Environment::Development).unwrap();
            assert!(config.http.compression, "{value}");
        }
        let config = with(&[("HTTP_COMPRESSION", "nope")], &Environment::Development).unwrap();
        assert!(!config.http.compression);
    }

    #[test]
    fn tests_run_jobs_in_the_foreground() {
        assert_eq!(
            with(&[], &Environment::Test).unwrap().workers,
            WorkerMode::ForegroundBlocking
        );
        let config = with(&[("WORKER_MODE", "BackgroundAsync")], &Environment::Test).unwrap();
        assert_eq!(config.workers, WorkerMode::BackgroundAsync);
    }
}
