//! Service configuration, from the process environment.
//!
//! There are no config files. A service states its defaults in code with
//! [`Defaults`], and the environment overrides them. Production has no
//! default database, so a missing `DATABASE_URL` fails at boot.

use crate::{Environment, Error, Result, logger, worker::WorkerMode};

/// One megabyte, as a body limit.
pub const MB: usize = 1_000_000;

/// A service's own defaults.
#[derive(Debug, Clone)]
pub struct Defaults {
    /// Database name stem: `person_service` gives `person_service_test`.
    pub database: &'static str,
    /// Port when `PORT` is unset.
    pub port: u16,
    /// Largest accepted request body, in bytes.
    pub body_limit: usize,
    /// Pool size outside production.
    pub max_connections: u32,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            database: "app",
            port: 5150,
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

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name).map_or(default, |value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

impl Config {
    /// Build configuration for `environment`, reading overrides from the
    /// process environment.
    ///
    /// # Errors
    ///
    /// Fails when production has no `DATABASE_URL` and the service has a
    /// database, or when `*` is given as a CORS origin in production.
    pub fn from_env(environment: &Environment, defaults: &Defaults) -> Result<Self> {
        let production = environment.is_production_like();
        let test = *environment == Environment::Test;

        let uri = match std::env::var("DATABASE_URL") {
            Ok(uri) => uri,
            Err(_) if defaults.database.is_empty() => String::new(),
            Err(_) if production => {
                return Err(Error::string("DATABASE_URL is required in production"));
            }
            Err(_) => format!(
                "postgres://loco:loco@localhost:5432/{}_{environment}",
                defaults.database
            ),
        };

        let cors_origins: Vec<String> = match std::env::var("CORS_ALLOWED_ORIGINS") {
            Ok(list) => list
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(str::to_string)
                .collect(),
            Err(_) if production => Vec::new(),
            Err(_) => vec!["*".to_string()],
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
                port: env_parse("PORT", defaults.port),
                binding: env_or("BINDING", if production { "0.0.0.0" } else { "localhost" }),
                host: env_or("HOST", "http://localhost"),
            },
            logger: Logger {
                enable: env_bool("LOG_ENABLE", !test),
                level: logger::LogLevel::parse(&env_or("LOG_LEVEL", level)),
                format: logger::Format::parse(&env_or(
                    "LOG_FORMAT",
                    if production { "json" } else { "compact" },
                )),
                override_filter: std::env::var("LOG_FILTER").ok(),
            },
            database: Database {
                uri,
                min_connections: env_parse("DB_MIN_CONNECTIONS", if production { 2 } else { 1 }),
                max_connections: env_parse(
                    "DB_MAX_CONNECTIONS",
                    if production {
                        20
                    } else {
                        defaults.max_connections
                    },
                ),
                connect_timeout: env_parse("DB_CONNECT_TIMEOUT", 500),
                idle_timeout: env_parse("DB_IDLE_TIMEOUT", 500),
                enable_logging: env_bool("DB_LOGGING", false),
                auto_migrate: env_bool("DB_AUTO_MIGRATE", !production),
                dangerously_truncate: test,
            },
            http: Http {
                body_limit: env_parse("BODY_LIMIT_BYTES", defaults.body_limit),
                compression: env_bool("HTTP_COMPRESSION", production),
                timeout_seconds: env_parse(
                    "REQUEST_TIMEOUT_SECONDS",
                    if production { 60 } else { 0 },
                ),
                cors_origins,
                hsts: env_bool("HTTP_HSTS", production),
            },
            workers: match std::env::var("WORKER_MODE").as_deref() {
                Ok("ForegroundBlocking") => WorkerMode::ForegroundBlocking,
                Ok("BackgroundAsync") => WorkerMode::BackgroundAsync,
                Ok("BackgroundQueue") => WorkerMode::BackgroundQueue,
                _ => workers,
            },
        })
    }
}
