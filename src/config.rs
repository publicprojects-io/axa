//! Service configuration, from the process environment.
//!
//! There are no config files. A service states its defaults in code with
//! [`Defaults`], and the environment overrides them. Production has no
//! default database, so a missing `DATABASE_URL` fails at boot.
//!
//! A setting that is set but makes no sense is a mistake, and a silent default
//! would hide it: `HTTP_HSTS=tru` must not turn HSTS off. So in a
//! production-like environment it stops the boot, with an error naming the
//! variable. Elsewhere the default applies, and the problem is kept in
//! [`Logger::warnings`] for [`logger::init`] to write out. A blank value counts
//! as unset, because that is what a compose file sends for an unset variable.

use std::cell::RefCell;

use crate::{
    Environment, Error, Result,
    cli::{Cli, Command},
    logger,
    worker::WorkerMode,
};

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
    /// Problems found while reading the settings. Always empty in a
    /// production-like environment, where a problem stops the boot.
    pub warnings: Vec<String>,
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

/// A timeout for the routes ending in one path suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeoutOverride {
    /// The path suffix, such as `/import`.
    pub path_suffix: String,
    /// The timeout, in seconds. `0` turns it off for these routes.
    pub seconds: u64,
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
    /// Routes with their own timeout, checked in order. Set these in code,
    /// for a route such as a large upload that legitimately takes longer.
    pub timeout_overrides: Vec<TimeoutOverride>,
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

/// What stands in for a secret in anything printed.
pub const REDACTED: &str = "[redacted]";

/// Words that make a name secret-shaped.
const SECRET_WORDS: [&str; 11] = [
    "SECRET",
    "PASS",
    "PWD",
    "TOKEN",
    "KEY",
    "CREDENTIAL",
    "PRIVATE",
    "AUTH",
    "COOKIE",
    "SALT",
    "SIGNATURE",
];

/// Whether `name` looks like it holds a secret. It errs towards yes, because
/// the cost of a wrong yes is one hidden value.
fn secret_shaped(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|word| name.contains(word))
}

/// Hide the password in `scheme://user:password@host`, or the whole of
/// `scheme://token@host`. It takes the last `@`, because a password may hold
/// `/` or `@` unescaped.
fn redact_userinfo(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let Some(at) = rest.rfind('@') else {
        return value.to_string();
    };
    let (userinfo, host) = rest.split_at(at);
    match userinfo.split_once(':') {
        Some((user, _)) => format!("{scheme}://{user}:{REDACTED}{host}"),
        None => format!("{scheme}://{REDACTED}{host}"),
    }
}

/// Hide the value of any secret-shaped `key=value` pair, as in a URL's query
/// or a keyword connection string.
fn redact_pairs(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    loop {
        let end = rest.find(['?', '&', ';', ',', ' ']).unwrap_or(rest.len());
        let (piece, tail) = rest.split_at(end);
        match piece.split_once('=') {
            Some((key, _)) if secret_shaped(key) => {
                out.push_str(key);
                out.push('=');
                out.push_str(REDACTED);
            }
            _ => out.push_str(piece),
        }
        let Some(separator) = tail.chars().next() else {
            return out;
        };
        out.push(separator);
        rest = &tail[separator.len_utf8()..];
    }
}

/// `value` as it is safe to print for the setting `name`. A secret-shaped name
/// hides the whole value. Otherwise a password in a URL, and any secret-shaped
/// `key=value` pair, are hidden. A quoted value could hold a separator, so one
/// that mentions a secret at all is hidden whole.
///
/// # Examples
///
/// ```
/// use axa::config::redact;
///
/// assert_eq!(redact("API_TOKEN", "abc"), "[redacted]");
/// assert_eq!(
///     redact("DATABASE_URL", "postgres://me:hunter2@db/app"),
///     "postgres://me:[redacted]@db/app"
/// );
/// assert_eq!(redact("PORT", "8000"), "8000");
/// ```
#[must_use]
pub fn redact(name: &str, value: &str) -> String {
    if secret_shaped(name) || (value.contains(['\'', '"']) && secret_shaped(value)) {
        return REDACTED.to_string();
    }
    redact_pairs(&redact_userinfo(value))
}

/// Typed reads over a lookup function, so parsing can be tested without
/// touching the process environment. A value that is set but invalid is noted
/// in `problems`, and what happens next is decided in one place, at the end of
/// [`Config::from_lookup`].
struct Vars<F> {
    lookup: F,
    problems: RefCell<Vec<String>>,
}

impl<F: Fn(&str) -> Option<String>> Vars<F> {
    /// The value of `name`, when it is set to something other than blanks.
    fn set(&self, name: &str) -> Option<String> {
        (self.lookup)(name).filter(|value| !value.trim().is_empty())
    }

    /// Note that `name` holds an invalid `value`.
    fn invalid(&self, name: &str, value: &str, expected: &str) {
        let shown = redact(name, value);
        self.problems
            .borrow_mut()
            .push(format!("{name}={shown:?} is not {expected}"));
    }

    /// The value of `name`, or `default` when unset.
    fn text(&self, name: &str, default: &str) -> String {
        (self.lookup)(name).unwrap_or_else(|| default.to_string())
    }

    /// The parsed value of `name`, or `default` when unset. An unparsable
    /// value also gives `default`, and is noted.
    fn parsed<T: std::str::FromStr>(&self, name: &str, default: T) -> T {
        let Some(value) = self.set(name) else {
            return default;
        };
        value.trim().parse().unwrap_or_else(|_| {
            self.invalid(name, &value, "a valid number");
            default
        })
    }

    /// The boolean value of `name`, or `default` when unset. `1`, `true`,
    /// `yes`, and `on` mean true, and `0`, `false`, `no`, and `off` mean false.
    /// Anything else also gives `default`, and is noted: a typo for "on" must
    /// not read as "off".
    fn flag(&self, name: &str, default: bool) -> bool {
        let Some(value) = self.set(name) else {
            return default;
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => {
                self.invalid(name, &value, "one of 1, true, yes, on, 0, false, no, off");
                default
            }
        }
    }

    /// The value of `name` as read by `from_name`, or `default` when unset. A
    /// name it does not know also gives `default`, and is noted.
    fn named<T>(&self, name: &str, default: T, from_name: impl Fn(&str) -> Option<T>) -> T {
        let Some(value) = self.set(name) else {
            return default;
        };
        from_name(&value).unwrap_or_else(|| {
            self.invalid(name, &value, "a name this setting knows");
            default
        })
    }

    /// The log filter from `LOG_FILTER`. One that does not parse is noted and
    /// dropped, because [`logger::init_env_filter`] would panic on it.
    fn log_filter(&self) -> Option<String> {
        let filter = self.set("LOG_FILTER")?;
        if tracing_subscriber::EnvFilter::try_new(&filter).is_err() {
            self.invalid("LOG_FILTER", &filter, "a valid log filter");
            return None;
        }
        Some(filter)
    }
}

/// The worker mode with this name, if there is one.
fn worker_mode(name: &str) -> Option<WorkerMode> {
    match name.trim() {
        "ForegroundBlocking" => Some(WorkerMode::ForegroundBlocking),
        "BackgroundAsync" => Some(WorkerMode::BackgroundAsync),
        "BackgroundQueue" => Some(WorkerMode::BackgroundQueue),
        _ => None,
    }
}

/// The connection string. Fail closed: a production service must never fall
/// back to a guessed local database.
fn database_uri<F: Fn(&str) -> Option<String>>(
    vars: &Vars<F>,
    environment: &Environment,
    defaults: &Defaults,
) -> Result<String> {
    match vars.set("DATABASE_URL") {
        Some(uri) => Ok(uri),
        None if defaults.database.is_empty() => Ok(String::new()),
        None if environment.is_production_like() => {
            Err(Error::string("DATABASE_URL is required in production"))
        }
        None => Ok(format!(
            "postgres://postgres:postgres@localhost:5432/{}_{environment}",
            defaults.database
        )),
    }
}

/// The allowed origins. Development serves browser dev servers on other ports,
/// so it allows any. Production sends no CORS headers until origins are listed.
fn cors_origins<F: Fn(&str) -> Option<String>>(
    vars: &Vars<F>,
    production: bool,
) -> Result<Vec<String>> {
    let origins: Vec<String> = match (vars.lookup)("CORS_ALLOWED_ORIGINS") {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_string)
            .collect(),
        None if production => Vec::new(),
        None => vec!["*".to_string()],
    };
    if production && origins.iter().any(|origin| origin == "*") {
        return Err(Error::string(
            "CORS_ALLOWED_ORIGINS must list origins in production, not *",
        ));
    }
    Ok(origins)
}

impl Config {
    /// Build configuration for `environment`, reading overrides from the
    /// process environment. See [`Config::from_lookup`].
    ///
    /// # Errors
    ///
    /// Fails in production as [`Config::from_lookup`] does.
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
    /// Fails in a production-like environment when there is no `DATABASE_URL`
    /// and the service has a database, when `*` is given as a CORS origin, or
    /// when any setting is set but invalid. The error names the variable and
    /// its value, unless the name is secret-shaped.
    pub fn from_lookup(
        environment: &Environment,
        defaults: &Defaults,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let vars = Vars {
            lookup,
            problems: RefCell::new(Vec::new()),
        };
        let production = environment.is_production_like();
        let test = *environment == Environment::Test;

        let uri = database_uri(&vars, environment, defaults)?;
        let cors_origins = cors_origins(&vars, production)?;

        let level = if test {
            logger::LogLevel::Warn
        } else if production {
            logger::LogLevel::Info
        } else {
            logger::LogLevel::Debug
        };
        let workers = if test {
            WorkerMode::ForegroundBlocking
        } else {
            WorkerMode::BackgroundQueue
        };

        let mut config = Self {
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
                level: vars.named("LOG_LEVEL", level, logger::LogLevel::from_name),
                format: vars.named(
                    "LOG_FORMAT",
                    if production {
                        logger::Format::Json
                    } else {
                        logger::Format::Compact
                    },
                    logger::Format::from_name,
                ),
                override_filter: vars.log_filter(),
                warnings: Vec::new(),
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
                timeout_overrides: Vec::new(),
                cors_origins,
                hsts: vars.flag("HTTP_HSTS", production),
            },
            workers: vars.named("WORKER_MODE", workers, worker_mode),
        };

        // Fail closed: a production service that misread a setting would run
        // with something nobody chose, and a warning there is read too late.
        let problems = vars.problems.into_inner();
        if production && !problems.is_empty() {
            return Err(Error::string(&format!(
                "invalid settings: {}",
                problems.join("; ")
            )));
        }
        config.logger.warnings = problems;
        Ok(config)
    }

    /// Apply the command line's overrides. Only `start` has any: `--binding`
    /// and `--port`.
    pub fn apply_cli(&mut self, cli: &Cli) {
        if let Command::Start { binding, port, .. } = &cli.command {
            if let Some(binding) = binding {
                self.server.binding.clone_from(binding);
            }
            if let Some(port) = port {
                self.server.port = *port;
            }
        }
    }

    /// The effective settings as `(variable, value)`, safe to print: every
    /// value has been through [`redact`]. The two lower-case names,
    /// `dangerously_truncate` and `timeout_overrides`, are not variables,
    /// because no variable sets them.
    #[must_use]
    pub fn settings(&self) -> Vec<(&'static str, String)> {
        let Self {
            server,
            logger,
            database,
            http,
            workers,
        } = self;
        [
            ("PORT", server.port.to_string()),
            ("BINDING", server.binding.clone()),
            ("HOST", server.host.clone()),
            ("LOG_ENABLE", logger.enable.to_string()),
            ("LOG_LEVEL", logger.level.to_string()),
            ("LOG_FORMAT", logger.format.to_string()),
            (
                "LOG_FILTER",
                logger.override_filter.clone().unwrap_or_default(),
            ),
            ("DATABASE_URL", database.uri.clone()),
            ("DB_MIN_CONNECTIONS", database.min_connections.to_string()),
            ("DB_MAX_CONNECTIONS", database.max_connections.to_string()),
            ("DB_CONNECT_TIMEOUT", database.connect_timeout.to_string()),
            ("DB_IDLE_TIMEOUT", database.idle_timeout.to_string()),
            ("DB_LOGGING", database.enable_logging.to_string()),
            ("DB_AUTO_MIGRATE", database.auto_migrate.to_string()),
            (
                "dangerously_truncate",
                database.dangerously_truncate.to_string(),
            ),
            ("BODY_LIMIT_BYTES", http.body_limit.to_string()),
            ("HTTP_COMPRESSION", http.compression.to_string()),
            ("REQUEST_TIMEOUT_SECONDS", http.timeout_seconds.to_string()),
            (
                "timeout_overrides",
                http.timeout_overrides
                    .iter()
                    .map(|limit| format!("{}:{}", limit.path_suffix, limit.seconds))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            ("CORS_ALLOWED_ORIGINS", http.cors_origins.join(",")),
            ("HTTP_HSTS", http.hsts.to_string()),
            ("WORKER_MODE", format!("{workers:?}")),
        ]
        .map(|(name, value)| (name, redact(name, &value)))
        .to_vec()
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
    fn axa_reads_the_environment_through_the_lighter_crate() {
        let environment: axa_environment::Environment = Environment::Test;
        assert_eq!(environment, crate::environment::Environment::Test);
        assert_eq!(crate::environment::VARIABLES, ["APP_ENV", "LOCO_ENV"]);
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
    fn the_local_database_user_names_no_framework() {
        let dev = with(&[], &Environment::Development).unwrap();
        assert_eq!(
            dev.database.uri,
            "postgres://postgres:postgres@localhost:5432/svc_development"
        );
    }

    #[test]
    fn production_refuses_to_boot_without_a_database_url() {
        let error = with(&[], &Environment::Production).unwrap_err();
        assert!(error.to_string().contains("DATABASE_URL"));
    }

    #[test]
    fn production_refuses_a_blank_database_url() {
        let error = with(&[("DATABASE_URL", " ")], &Environment::Production).unwrap_err();
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
    fn an_unparsable_number_falls_back_to_the_default_with_a_warning() {
        let config = with(&[("PORT", "not-a-port")], &Environment::Development).unwrap();
        assert_eq!(config.server.port, DEFAULT_PORT);
        assert!(config.logger.warnings[0].contains("PORT"));
        assert!(config.logger.warnings[0].contains("not-a-port"));
    }

    #[test]
    fn production_refuses_an_unparsable_number() {
        for name in [
            "PORT",
            "BODY_LIMIT_BYTES",
            "REQUEST_TIMEOUT_SECONDS",
            "DB_MAX_CONNECTIONS",
        ] {
            let pairs = [("DATABASE_URL", "postgres://x"), (name, "12x")];
            let error = with(&pairs, &Environment::Production).unwrap_err();
            let text = error.to_string();
            assert!(text.contains(name) && text.contains("12x"), "{text}");
        }
    }

    #[test]
    fn a_number_out_of_range_is_refused() {
        let pairs = [("DATABASE_URL", "postgres://x"), ("PORT", "70000")];
        assert!(with(&pairs, &Environment::Production).is_err());
    }

    #[test]
    fn every_problem_is_named_at_once() {
        let pairs = [
            ("DATABASE_URL", "postgres://x"),
            ("PORT", "x"),
            ("HTTP_HSTS", "tru"),
        ];
        let text = with(&pairs, &Environment::Production)
            .unwrap_err()
            .to_string();
        assert!(
            text.contains("PORT") && text.contains("HTTP_HSTS"),
            "{text}"
        );
    }

    #[test]
    fn valid_settings_raise_no_warnings() {
        let pairs = [
            ("PORT", " 8000 "),
            ("HTTP_HSTS", "off"),
            ("LOG_LEVEL", "warn"),
            ("LOG_FORMAT", "pretty"),
            ("LOG_FILTER", "my_service=trace"),
            ("WORKER_MODE", "BackgroundAsync"),
        ];
        let config = with(&pairs, &Environment::Development).unwrap();
        assert_eq!(config.logger.warnings, Vec::<String>::new());
        assert_eq!(config.server.port, 8000);
    }

    #[test]
    fn a_blank_value_counts_as_unset() {
        let pairs = [
            ("DATABASE_URL", "postgres://x"),
            ("PORT", ""),
            ("HTTP_HSTS", " "),
        ];
        let config = with(&pairs, &Environment::Production).unwrap();
        assert_eq!(config.server.port, DEFAULT_PORT);
        assert!(config.http.hsts);
    }

    #[test]
    fn flags_accept_the_usual_spellings() {
        for value in ["1", "true", "YES", " on "] {
            let config = with(&[("HTTP_COMPRESSION", value)], &Environment::Development).unwrap();
            assert!(config.http.compression, "{value}");
        }
        let url = ("DATABASE_URL", "postgres://x");
        for value in ["0", "false", "NO", " off "] {
            let pairs = [url, ("HTTP_HSTS", value)];
            let config = with(&pairs, &Environment::Production).unwrap();
            assert!(!config.http.hsts, "{value}");
            assert_eq!(config.logger.warnings, Vec::<String>::new());
        }
    }

    #[test]
    fn production_refuses_a_flag_it_does_not_know() {
        let pairs = [("DATABASE_URL", "postgres://x"), ("HTTP_HSTS", "tru")];
        let text = with(&pairs, &Environment::Production)
            .unwrap_err()
            .to_string();
        assert!(text.contains("HTTP_HSTS") && text.contains("tru"), "{text}");
    }

    #[test]
    fn a_flag_it_does_not_know_keeps_its_default_with_a_warning() {
        let on = Environment::Other("staging".to_string());
        assert!(with(&[("DB_LOGGING", "nope")], &on).is_err());
        let config = with(&[("HTTP_COMPRESSION", "nope")], &Environment::Development).unwrap();
        assert!(!config.http.compression);
        assert!(config.logger.warnings[0].contains("HTTP_COMPRESSION"));
        let config = with(&[("LOG_ENABLE", "nope")], &Environment::Development).unwrap();
        assert!(config.logger.enable);
    }

    #[test]
    fn production_refuses_a_name_it_does_not_know() {
        for name in ["LOG_LEVEL", "LOG_FORMAT", "WORKER_MODE"] {
            let pairs = [("DATABASE_URL", "postgres://x"), (name, "nonsense")];
            let error = with(&pairs, &Environment::Production).unwrap_err();
            assert!(error.to_string().contains(name), "{name}");
        }
    }

    #[test]
    fn a_bad_log_filter_is_refused_and_never_reaches_the_logger() {
        let pairs = [("DATABASE_URL", "postgres://x"), ("LOG_FILTER", "a=b=c")];
        let error = with(&pairs, &Environment::Production).unwrap_err();
        assert!(error.to_string().contains("LOG_FILTER"));
        let config = with(&[("LOG_FILTER", "a=b=c")], &Environment::Development).unwrap();
        assert_eq!(config.logger.override_filter, None);
    }

    #[test]
    fn an_invalid_value_under_a_secret_name_is_not_printed() {
        let vars = Vars {
            lookup: |_: &str| Some("hunter2".to_string()),
            problems: RefCell::new(Vec::new()),
        };
        let _: u16 = vars.parsed("REDIS_PASSWORD_LENGTH", 1);
        let problems = vars.problems.into_inner();
        assert!(problems[0].contains("REDIS_PASSWORD_LENGTH"));
        assert!(!problems[0].contains("hunter2"), "{}", problems[0]);
    }

    #[test]
    fn redact_hides_secret_shaped_names() {
        for name in [
            "API_KEY",
            "jwt_secret",
            "DB_PASSWORD",
            "AUTH_TOKEN",
            "SIGNING_KEY",
        ] {
            assert_eq!(redact(name, "abc"), REDACTED, "{name}");
        }
        assert_eq!(redact("PORT", "abc"), "abc");
    }

    #[test]
    fn redact_hides_the_password_in_a_url() {
        let cases = [
            (
                "postgres://me:pw@h:5432/db",
                "postgres://me:[redacted]@h:5432/db",
            ),
            ("postgres://me:p/w@h/db", "postgres://me:[redacted]@h/db"),
            ("postgres://me:p@w@h/db", "postgres://me:[redacted]@h/db"),
            ("redis://token@h", "redis://[redacted]@h"),
            ("postgres://h:5432/db", "postgres://h:5432/db"),
            ("http://localhost", "http://localhost"),
        ];
        for (value, want) in cases {
            assert_eq!(redact("DATABASE_URL", value), want);
        }
    }

    #[test]
    fn redact_hides_secret_pairs() {
        let cases = [
            (
                "postgres://h/db?password=pw&sslmode=require",
                "postgres://h/db?password=[redacted]&sslmode=require",
            ),
            (
                "host=h password=pw dbname=db",
                "host=h password=[redacted] dbname=db",
            ),
            ("host=h password='a b' dbname=db", REDACTED),
            ("sslmode=require", "sslmode=require"),
        ];
        for (value, want) in cases {
            assert_eq!(redact("DATABASE_URL", value), want);
        }
    }

    #[test]
    fn every_setting_passes_through_redaction() {
        let config = with(
            &[
                ("DATABASE_URL", "postgres://me:hunter2@h/db"),
                ("HOST", "https://me:hunter3@example.org"),
                ("LOG_FILTER", "svc=info"),
            ],
            &Environment::Development,
        )
        .unwrap();
        let text = format!("{:?}", config.settings());
        assert!(!text.contains("hunter"), "{text}");
    }

    #[test]
    fn the_command_line_overrides_only_what_start_gives() {
        use clap::Parser;
        let parse = |args: &[&str]| Cli::try_parse_from(args).unwrap();
        let mut config = with(&[], &Environment::Development).unwrap();
        config.apply_cli(&parse(&["svc", "check"]));
        assert_eq!(config.server.port, DEFAULT_PORT);
        config.apply_cli(&parse(&["svc", "start", "--port", "9000"]));
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.server.binding, "localhost");
        config.apply_cli(&parse(&["svc", "start", "--binding", "0.0.0.0"]));
        assert_eq!(config.server.binding, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
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
