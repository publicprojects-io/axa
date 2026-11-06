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

use std::{cell::RefCell, fmt};

use crate::{
    Environment, Error, Result,
    cli::{Cli, Command},
    client::ProxyNet,
    logger,
    worker::WorkerMode,
};

/// The default port: a=1, x=24, a=1.
pub const DEFAULT_PORT: u16 = 1241;

/// How long browsers keep to HTTPS unless `HSTS_MAX_AGE_SECONDS` says otherwise: one year.
pub const DEFAULT_HSTS_MAX_AGE: u64 = 31_536_000;

/// Production's policy unless `HTTP_CSP` says otherwise. It suits a JSON API: a service that
/// serves pages must name what they load.
pub const DEFAULT_CSP: &str = "default-src 'none'; frame-ancestors 'none'";

/// How long to wait for a database connection, unless a service or
/// `DB_CONNECT_TIMEOUT` says otherwise: five seconds, in milliseconds.
pub const DEFAULT_CONNECT_TIMEOUT: u64 = 5_000;

/// How long an idle database connection stays open, unless a service or
/// `DB_IDLE_TIMEOUT` says otherwise: ten minutes, in milliseconds.
pub const DEFAULT_IDLE_TIMEOUT: u64 = 600_000;

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
    /// Database connection timeout, in milliseconds.
    pub connect_timeout: u64,
    /// Database idle timeout, in milliseconds.
    pub idle_timeout: u64,
    /// Whether to migrate at boot outside production. Turn this off for a
    /// service that shares a database it does not own. Production never does
    /// unless `DB_AUTO_MIGRATE` says so.
    pub auto_migrate: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            database: "app",
            port: DEFAULT_PORT,
            production_port: None,
            body_limit: 2 * MB,
            max_connections: 10,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            auto_migrate: true,
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
    /// How long, once told to stop, to wait for requests and jobs to finish. `0`
    /// waits for as long as they take.
    pub shutdown_timeout_seconds: u64,
    /// How long a client has to send a request's headers. `0` allows for ever.
    pub header_read_timeout_seconds: u64,
    /// The most connections open at once. More wait to be accepted.
    pub max_connections: usize,
}

/// How long a client has to send a request's headers unless
/// `HEADER_READ_TIMEOUT_SECONDS` says otherwise.
pub const DEFAULT_HEADER_READ_TIMEOUT_SECONDS: u64 = 30;

/// How long shutdown waits unless `SHUTDOWN_TIMEOUT_SECONDS` says otherwise. It
/// sits under an orchestrator's usual 30 seconds before it kills a process.
pub const DEFAULT_SHUTDOWN_TIMEOUT_SECONDS: u64 = 25;

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

/// Database settings. `Debug` hides any password in the connection string.
#[derive(Clone)]
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
    /// How long browsers keep to HTTPS, in seconds.
    pub hsts_max_age: u64,
    /// The `content-security-policy`, if any.
    pub csp: Option<String>,
    /// Proxies whose `X-Forwarded-For` entries are believed. Empty ignores the header.
    pub trusted_proxies: Vec<ProxyNet>,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Named one by one, so that a new field cannot print without a decision.
        let Self {
            uri,
            min_connections,
            max_connections,
            connect_timeout,
            idle_timeout,
            enable_logging,
            auto_migrate,
            dangerously_truncate,
        } = self;
        f.debug_struct("Database")
            .field("uri", &redact("DATABASE_URL", uri))
            .field("min_connections", min_connections)
            .field("max_connections", max_connections)
            .field("connect_timeout", connect_timeout)
            .field("idle_timeout", idle_timeout)
            .field("enable_logging", enable_logging)
            .field("auto_migrate", auto_migrate)
            .field("dangerously_truncate", dangerously_truncate)
            .finish()
    }
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

/// Whether a connection string keeps its traffic private: it demands TLS, or it goes
/// no further than this machine. An address that cannot be read counts as neither.
/// `sslmode=require` counts, though it encrypts without checking who answers. Only
/// `verify-full` checks that.
fn stays_private(uri: &str) -> bool {
    let Some((_, rest)) = uri.split_once("://") else {
        return false;
    };
    let (location, query) = rest.split_once('?').unwrap_or((rest, ""));
    // The last of a repeated parameter wins, as it does in the driver.
    let parameter = |names: &[&str]| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .filter(|(name, _)| names.contains(name))
            .map(|(_, value)| value)
            .next_back()
    };
    if parameter(&["sslmode", "ssl-mode"]).is_some_and(|mode| {
        ["require", "verify-ca", "verify-full"]
            .iter()
            .any(|safe| mode.eq_ignore_ascii_case(safe))
    }) {
        return true;
    }
    let authority = location.split('/').next().unwrap_or_default();
    let address = authority.rsplit('@').next().unwrap_or_default();
    let host = match address.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => address.split(':').next().unwrap_or_default(),
    };
    let host = parameter(&["host", "hostaddr"]).unwrap_or(host);
    host.starts_with('/')
        || host.starts_with("%2F")
        || matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// The connection string. Fail closed: a production service must never fall
/// back to a guessed local database.
fn database_uri<F: Fn(&str) -> Option<String>>(
    vars: &Vars<F>,
    environment: &Environment,
    defaults: &Defaults,
) -> Result<String> {
    match vars.set("DATABASE_URL") {
        Some(uri)
            if environment.is_production_like()
                && !stays_private(&uri)
                && !vars.flag("DB_ALLOW_INSECURE", false) =>
        {
            Err(Error::string(
                "DATABASE_URL must set sslmode to require, verify-ca, or verify-full in \
                 production, unless DB_ALLOW_INSECURE is set",
            ))
        }
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
/// `MAX_CONNECTIONS`, which must leave room for at least one.
fn connection_limit<F: Fn(&str) -> Option<String>>(vars: &Vars<F>) -> usize {
    let max = vars.parsed("MAX_CONNECTIONS", crate::server::MAX_CONNECTIONS);
    if max == 0 {
        vars.invalid("MAX_CONNECTIONS", "0", "at least 1");
        return crate::server::MAX_CONNECTIONS;
    }
    max
}

/// `TRUSTED_PROXIES`, as a comma-separated list. A list with any bad entry is noted,
/// and none are trusted, which is the side to fail on.
fn trusted_proxies<F: Fn(&str) -> Option<String>>(vars: &Vars<F>) -> Vec<ProxyNet> {
    let Some(list) = vars.set("TRUSTED_PROXIES") else {
        return Vec::new();
    };
    list.split(',')
        .map(|net| net.trim().parse())
        .collect::<std::result::Result<_, _>>()
        .unwrap_or_else(|_| {
            vars.invalid("TRUSTED_PROXIES", &list, "addresses or CIDR ranges");
            Vec::new()
        })
}

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

/// The smallest and largest pool sizes. An unset smallest size follows a small
/// largest one, so `DB_MAX_CONNECTIONS=1` works alone. A pool that cannot exist is
/// noted, and the default applies.
fn pool_size<F: Fn(&str) -> Option<String>>(
    vars: &Vars<F>,
    defaults: &Defaults,
    production: bool,
) -> (u32, u32) {
    let default_max = if production {
        20
    } else {
        defaults.max_connections
    };
    let mut max = vars.parsed("DB_MAX_CONNECTIONS", default_max);
    if max == 0 {
        vars.invalid("DB_MAX_CONNECTIONS", "0", "at least 1");
        max = default_max;
    }
    let default_min = u32::from(production).saturating_add(1).min(max);
    let min = vars.parsed("DB_MIN_CONNECTIONS", default_min);
    if min > max {
        let shown = min.to_string();
        vars.invalid("DB_MIN_CONNECTIONS", &shown, "at most DB_MAX_CONNECTIONS");
        return (default_min, max);
    }
    (min, max)
}

/// The policy from `HTTP_CSP`, where blank means none, or production's default when unset.
fn content_security_policy(setting: Option<String>, production: bool) -> Option<String> {
    match setting {
        Some(policy) if policy.trim().is_empty() => None,
        Some(policy) => Some(policy),
        None if production => Some(DEFAULT_CSP.to_string()),
        None => None,
    }
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
        let (min_connections, max_connections) = pool_size(&vars, defaults, production);

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
                shutdown_timeout_seconds: vars
                    .parsed("SHUTDOWN_TIMEOUT_SECONDS", DEFAULT_SHUTDOWN_TIMEOUT_SECONDS),
                header_read_timeout_seconds: vars.parsed(
                    "HEADER_READ_TIMEOUT_SECONDS",
                    DEFAULT_HEADER_READ_TIMEOUT_SECONDS,
                ),
                max_connections: connection_limit(&vars),
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
                min_connections,
                max_connections,
                connect_timeout: vars.parsed("DB_CONNECT_TIMEOUT", defaults.connect_timeout),
                idle_timeout: vars.parsed("DB_IDLE_TIMEOUT", defaults.idle_timeout),
                enable_logging: vars.flag("DB_LOGGING", false),
                auto_migrate: vars.flag("DB_AUTO_MIGRATE", defaults.auto_migrate && !production),
                dangerously_truncate: test,
            },
            http: Http {
                body_limit: vars.parsed("BODY_LIMIT_BYTES", defaults.body_limit),
                compression: vars.flag("HTTP_COMPRESSION", production),
                timeout_seconds: vars
                    .parsed("REQUEST_TIMEOUT_SECONDS", if production { 60 } else { 0 }),
                timeout_overrides: Vec::new(),
                cors_origins,
                trusted_proxies: trusted_proxies(&vars),
                hsts: vars.flag("HTTP_HSTS", production),
                hsts_max_age: vars.parsed("HSTS_MAX_AGE_SECONDS", DEFAULT_HSTS_MAX_AGE),
                csp: content_security_policy((vars.lookup)("HTTP_CSP"), production),
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
            (
                "HEADER_READ_TIMEOUT_SECONDS",
                self.server.header_read_timeout_seconds.to_string(),
            ),
            ("MAX_CONNECTIONS", self.server.max_connections.to_string()),
            (
                "SHUTDOWN_TIMEOUT_SECONDS",
                server.shutdown_timeout_seconds.to_string(),
            ),
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
            (
                "TRUSTED_PROXIES",
                http.trusted_proxies
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            ("HTTP_HSTS", http.hsts.to_string()),
            ("HSTS_MAX_AGE_SECONDS", http.hsts_max_age.to_string()),
            ("HTTP_CSP", http.csp.clone().unwrap_or_default()),
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
    fn production_refuses_a_database_url_that_could_go_in_the_clear() {
        for uri in [
            "postgres://me:pw@db.internal/app",
            "postgres://me:pw@db.internal/app?sslmode=prefer",
            "postgres://me:pw@db.internal/app?sslmode=disable",
            "postgres://me:pw@db.internal/app?sslmode=require&sslmode=disable",
            "postgres://localhost:pw@db.internal/app",
            "postgres://localhost/app?hostaddr=10.0.0.5",
            "postgres://db.internal/app?hostaddr=127.0.0.1&host=db.internal",
            "postgres:///app",
            "not a url",
        ] {
            let error = with(&[("DATABASE_URL", uri)], &Environment::Production).unwrap_err();
            assert!(error.to_string().contains("sslmode"), "{uri}");
            assert!(!error.to_string().contains("pw"), "{uri}");
            let allowed = [("DATABASE_URL", uri), ("DB_ALLOW_INSECURE", "true")];
            assert!(with(&allowed, &Environment::Production).is_ok(), "{uri}");
            assert!(with(&[("DATABASE_URL", uri)], &Environment::Development).is_ok());
        }
    }

    #[test]
    fn trusted_proxies_are_read_and_a_bad_list_trusts_none() {
        let url = ("DATABASE_URL", "postgres://localhost/x");
        let config = with(
            &[url, ("TRUSTED_PROXIES", "10.0.0.0/8, 192.168.1.1")],
            &Environment::Production,
        )
        .unwrap();
        assert_eq!(config.http.trusted_proxies.len(), 2);

        let unset = with(&[url], &Environment::Production).unwrap();
        assert_eq!(unset.http.trusted_proxies.len(), 0);

        let bad = [url, ("TRUSTED_PROXIES", "10.0.0.0/8, nonsense")];
        assert!(with(&bad, &Environment::Production).is_err());
        let config = with(&bad, &Environment::Development).unwrap();
        assert_eq!(config.http.trusted_proxies.len(), 0);
    }

    #[test]
    fn the_connection_limit_is_a_setting_and_cannot_be_zero() {
        let url = ("DATABASE_URL", "postgres://localhost/x");
        let config = with(&[url, ("MAX_CONNECTIONS", "500")], &Environment::Production).unwrap();
        assert_eq!(config.server.max_connections, 500);
        let config = with(&[url], &Environment::Production).unwrap();
        assert_eq!(
            config.server.max_connections,
            crate::server::MAX_CONNECTIONS
        );
        assert!(with(&[url, ("MAX_CONNECTIONS", "0")], &Environment::Production).is_err());
    }

    #[test]
    fn debug_output_never_prints_a_password() {
        for uri in [
            "postgres://me:hunter2@db.internal/app",
            "postgres://db.internal/app?password=hunter2",
        ] {
            let config = with(&[("DATABASE_URL", uri)], &Environment::Development).unwrap();
            let printed = format!("{config:?}");
            assert!(!printed.contains("hunter2"), "{printed}");
            assert!(printed.contains("db.internal"), "{printed}");
        }
    }

    #[test]
    fn production_accepts_a_database_url_that_stays_private() {
        for uri in [
            "postgres://me:pw@db.internal/app?sslmode=require",
            "postgres://me:pw@db.internal/app?sslmode=VERIFY-FULL",
            "postgresql://db.internal/app?application_name=x&ssl-mode=verify-ca",
            "postgres://me:pw@localhost:5432/app",
            "postgres://127.0.0.1/app",
            "postgres://[::1]:5432/app",
            "postgres:///app?host=/var/run/postgresql",
            "postgres://db.internal/app?hostaddr=127.0.0.1",
        ] {
            assert!(
                with(&[("DATABASE_URL", uri)], &Environment::Production).is_ok(),
                "{uri}"
            );
        }
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
        let url = [("DATABASE_URL", "postgres://localhost/x")];
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
            &[("DATABASE_URL", "postgres://localhost/x")],
            &Environment::Production,
        )
        .unwrap();
        assert!(!config.database.auto_migrate);
    }

    #[test]
    fn the_database_timeouts_suit_a_real_database() {
        let config = with(
            &[("DATABASE_URL", "postgres://localhost/x")],
            &Environment::Production,
        )
        .unwrap();
        assert_eq!(config.database.connect_timeout, 5_000);
        assert_eq!(config.database.idle_timeout, 600_000);
    }

    #[test]
    fn a_service_can_state_its_own_database_defaults() {
        let defaults = Defaults {
            database: "svc",
            connect_timeout: 10_000,
            idle_timeout: 30_000,
            auto_migrate: false,
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Development, &defaults, |_| None).unwrap();
        assert_eq!(config.database.connect_timeout, 10_000);
        assert_eq!(config.database.idle_timeout, 30_000);
        assert!(!config.database.auto_migrate);
        let lookup = |name: &str| (name == "DB_AUTO_MIGRATE").then(|| "on".to_string());
        let config = Config::from_lookup(&Environment::Development, &defaults, lookup).unwrap();
        assert!(config.database.auto_migrate);
    }

    #[test]
    fn development_migrates_at_boot_by_default() {
        assert!(
            with(&[], &Environment::Development)
                .unwrap()
                .database
                .auto_migrate
        );
    }

    #[test]
    fn a_small_largest_pool_size_carries_the_smallest_with_it() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
            ("DB_MAX_CONNECTIONS", "1"),
        ];
        let config = with(&pairs, &Environment::Production).unwrap();
        assert_eq!(config.database.min_connections, 1);
        assert_eq!(config.database.max_connections, 1);
    }

    #[test]
    fn a_pool_that_cannot_exist_is_refused() {
        let url = ("DATABASE_URL", "postgres://localhost/x");
        let pairs = [
            url,
            ("DB_MIN_CONNECTIONS", "9"),
            ("DB_MAX_CONNECTIONS", "3"),
        ];
        let text = with(&pairs, &Environment::Production)
            .unwrap_err()
            .to_string();
        assert!(text.contains("DB_MIN_CONNECTIONS"), "{text}");
        let pairs = [url, ("DB_MAX_CONNECTIONS", "0")];
        let text = with(&pairs, &Environment::Production)
            .unwrap_err()
            .to_string();
        assert!(text.contains("DB_MAX_CONNECTIONS"), "{text}");
        let config = with(&[("DB_MAX_CONNECTIONS", "0")], &Environment::Development).unwrap();
        assert_eq!(config.database.max_connections, 10);
        assert!(config.logger.warnings[0].contains("DB_MAX_CONNECTIONS"));
    }

    #[test]
    fn production_sends_a_strict_policy_by_default() {
        let config = with(
            &[("DATABASE_URL", "postgres://localhost/x")],
            &Environment::Production,
        )
        .unwrap();
        assert_eq!(config.http.csp.as_deref(), Some(DEFAULT_CSP));
        assert_eq!(config.http.hsts_max_age, DEFAULT_HSTS_MAX_AGE);
    }

    #[test]
    fn development_sends_no_policy_by_default() {
        let config = with(&[], &Environment::Development).unwrap();
        assert_eq!(config.http.csp, None);
    }

    #[test]
    fn a_service_can_name_its_own_policy_or_none() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
            ("HTTP_CSP", "default-src 'self'"),
        ];
        let config = with(&pairs, &Environment::Production).unwrap();
        assert_eq!(config.http.csp.as_deref(), Some("default-src 'self'"));
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
            ("HTTP_CSP", " "),
        ];
        assert_eq!(
            with(&pairs, &Environment::Production).unwrap().http.csp,
            None
        );
    }

    #[test]
    fn the_hsts_max_age_can_be_set() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
            ("HSTS_MAX_AGE_SECONDS", "63072000"),
        ];
        let config = with(&pairs, &Environment::Production).unwrap();
        assert_eq!(config.http.hsts_max_age, 63_072_000);
    }

    #[test]
    fn production_refuses_a_wildcard_cors_origin() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
            ("CORS_ALLOWED_ORIGINS", "*"),
        ];
        let error = with(&pairs, &Environment::Production).unwrap_err();
        assert!(error.to_string().contains("CORS_ALLOWED_ORIGINS"));
    }

    #[test]
    fn production_sends_no_cors_headers_until_origins_are_listed() {
        let url = ("DATABASE_URL", "postgres://localhost/x");
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
        let lookup =
            |name: &str| (name == "DATABASE_URL").then(|| "postgres://localhost/x".to_string());
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
            let pairs = [("DATABASE_URL", "postgres://localhost/x"), (name, "12x")];
            let error = with(&pairs, &Environment::Production).unwrap_err();
            let text = error.to_string();
            assert!(text.contains(name) && text.contains("12x"), "{text}");
        }
    }

    #[test]
    fn a_number_out_of_range_is_refused() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
            ("PORT", "70000"),
        ];
        assert!(with(&pairs, &Environment::Production).is_err());
    }

    #[test]
    fn every_problem_is_named_at_once() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
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
            ("DATABASE_URL", "postgres://localhost/x"),
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
        let url = ("DATABASE_URL", "postgres://localhost/x");
        for value in ["0", "false", "NO", " off "] {
            let pairs = [url, ("HTTP_HSTS", value)];
            let config = with(&pairs, &Environment::Production).unwrap();
            assert!(!config.http.hsts, "{value}");
            assert_eq!(config.logger.warnings, Vec::<String>::new());
        }
    }

    #[test]
    fn production_refuses_a_flag_it_does_not_know() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
            ("HTTP_HSTS", "tru"),
        ];
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
            let pairs = [
                ("DATABASE_URL", "postgres://localhost/x"),
                (name, "nonsense"),
            ];
            let error = with(&pairs, &Environment::Production).unwrap_err();
            assert!(error.to_string().contains(name), "{name}");
        }
    }

    #[test]
    fn a_bad_log_filter_is_refused_and_never_reaches_the_logger() {
        let pairs = [
            ("DATABASE_URL", "postgres://localhost/x"),
            ("LOG_FILTER", "a=b=c"),
        ];
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
