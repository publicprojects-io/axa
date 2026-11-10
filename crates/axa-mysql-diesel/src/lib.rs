//! Diesel pooling, migrations, and test resets for MySQL, in pure Rust.
//!
//! This has the same shape as `axa-postgres-diesel` and the other axa database
//! crates, so a service can move between them by changing names. It uses
//! `diesel-async` over `mysql_async`, so there is no `libmysqlclient` to link,
//! no OpenSSL, and no C in the database path. It holds the operations every
//! service repeats, and a service writes its own queries with Diesel as usual.
//!
//! ```ignore
//! use axa_mysql_diesel::{PoolConfig, connect, migrate};
//! use diesel_migrations::{EmbeddedMigrations, embed_migrations};
//!
//! const MIGRATIONS: EmbeddedMigrations = embed_migrations!();
//!
//! let pool = connect(&PoolConfig::new("mysql://app:secret@localhost:3306/app")).await?;
//! migrate(&pool, MIGRATIONS).await?;
//! ```
//!
//! # What differs from Postgres
//!
//! - Names are quoted with backticks rather than double quotes.
//! - [`truncate`] empties only the tables it is given, with foreign key checks
//!   off on one connection, and turns them back on. MySQL has no `CASCADE` for
//!   `TRUNCATE`, so rows in other tables that point at the emptied ones stay:
//!   name every table that should be emptied.
//! - [`dangerously_recreate`] drops every base table in `DATABASE()`, with
//!   foreign key checks off. Views, routines, and events are left alone. MySQL
//!   has no enum types apart from columns, so there are none to drop.
//! - MySQL commits each DDL statement as it runs, so a migration that fails part
//!   way leaves the statements before the failure applied, even though Diesel
//!   runs each migration in a transaction. Keep one DDL statement per migration
//!   where that matters.
//!
//! # TLS
//!
//! Connections use TLS through `rustls` with `ring`, trusting the standard web
//! roots. The connection URL takes the same `ssl-mode` query parameter as
//! `sqlx`, so one URL works with every axa MySQL crate. This crate reads it
//! from the URL itself and sets `mysql_async`'s TLS to match:
//!
//! | `ssl-mode` | Here |
//! |---|---|
//! | `DISABLED` | Plain text. |
//! | `PREFERRED`, the default | TLS when the server offers it, without checking its certificate; plain text when the server offers no TLS. |
//! | `REQUIRED` | TLS, without checking the certificate. |
//! | `VERIFY_CA` | TLS, with a certificate that chains to a trusted root, for any host name. |
//! | `VERIFY_IDENTITY` | TLS, with a trusted certificate for the host name in the URL. |
//!
//! The value is read without regard to case. In production the URL should say
//! `ssl-mode=VERIFY_IDENTITY`, because the `PREFERRED` default neither checks the
//! server nor refuses plain text. A database behind a private certificate
//! authority names its root certificate file with `ssl-ca=/path/to/ca.pem`, as
//! for `sqlx`, which is trusted beside the web roots.
//!
//! `mysql_async`'s own TLS parameters (`require_ssl`, `verify_ca`,
//! `verify_identity`, `built_in_roots`) are refused, so that a URL cannot say
//! two things about TLS. Other `sqlx` parameters, such as `charset` or
//! `ssl-cert`, are not understood by `mysql_async`, which refuses them. A
//! connection goes over TCP to the host in the URL, rather than switching to a
//! local socket, unless the URL says `prefer_socket=true`; a socket is never
//! encrypted.
//!
//! `mysql_async` builds its TLS settings from the process's default `rustls`
//! provider, which is `ring` when that is the only one built in. A service that
//! also builds in `aws-lc-rs` must install a default provider itself.

use std::{borrow::Cow, path::PathBuf, time::Duration};

use deadpool::Runtime;
pub use diesel;
use diesel::{
    ConnectionError, ConnectionResult, QueryableByName,
    migration::MigrationSource,
    mysql::Mysql,
    result::{DatabaseErrorKind, Error as QueryError},
    sql_types::Text,
};
pub use diesel_async;
use diesel_async::{
    AsyncMigrationHarness, AsyncMysqlConnection, RunQueryDsl, SimpleAsyncConnection,
    pooled_connection::{
        AsyncDieselConnectionManager, ManagerConfig,
        deadpool::{BuildError, Object, Pool as DeadPool, PoolError},
    },
};
pub use diesel_migrations;
use diesel_migrations::MigrationHarness;
pub use mysql_async;
use mysql_async::{DriverError, Opts, OptsBuilder, SslOpts};

/// A connection pool.
pub type Pool = DeadPool<AsyncMysqlConnection>;

/// A pooled connection.
pub type Connection = Object<AsyncMysqlConnection>;

/// A failure in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The connection URL could not be read.
    #[error("invalid connection URL: {0}")]
    Url(String),
    /// The pool could not be built.
    #[error(transparent)]
    Build(#[from] BuildError),
    /// No connection could be had.
    #[error(transparent)]
    Pool(#[from] PoolError),
    /// A query failed.
    #[error(transparent)]
    Query(#[from] QueryError),
    /// A migration failed.
    #[error("migration failed: {0}")]
    Migration(String),
    /// A table name was refused by [`quote_identifier`].
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
}

/// Pool settings. The connection string is left out of the `Debug` text, as it may
/// hold a password.
#[derive(Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// Connection URL, `mysql://user:password@host:port/database`.
    pub uri: String,
    /// Largest number of connections.
    pub max_connections: usize,
    /// How long a caller waits for a connection, whether a free one or a new one,
    /// before it gets an error.
    pub wait_timeout: Duration,
}

impl std::fmt::Debug for PoolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("max_connections", &self.max_connections)
            .field("wait_timeout", &self.wait_timeout)
            .finish_non_exhaustive()
    }
}

impl PoolConfig {
    /// Settings for `uri` with a small pool.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            max_connections: 10,
            wait_timeout: Duration::from_secs(5),
        }
    }
}

/// How strictly a connection uses TLS, as `sqlx`'s `ssl-mode` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SslMode {
    Disabled,
    Preferred,
    Required,
    VerifyCa,
    VerifyIdentity,
}

impl SslMode {
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_uppercase().as_str() {
            "DISABLED" => Some(Self::Disabled),
            "PREFERRED" => Some(Self::Preferred),
            "REQUIRED" => Some(Self::Required),
            "VERIFY_CA" => Some(Self::VerifyCa),
            "VERIFY_IDENTITY" => Some(Self::VerifyIdentity),
            _ => None,
        }
    }
}

/// Where and how to connect, read once from the URL.
#[derive(Clone, Debug)]
struct Target {
    opts: Opts,
    mode: SslMode,
    ca: Option<PathBuf>,
}

/// `mysql_async`'s own TLS parameters, refused in favour of `ssl-mode`.
const DRIVER_TLS_PARAMETERS: &[&str] = &[
    "require_ssl",
    "verify_ca",
    "verify_identity",
    "built_in_roots",
];

/// Decode `%XX` escapes in a query parameter's value.
fn percent_decode(value: &str) -> Result<String, Error> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| Error::Url("bad percent escape in `ssl-ca'".to_string()))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| Error::Url("`ssl-ca' is not UTF-8".to_string()))
}

/// Read `uri`, taking out `ssl-mode` and `ssl-ca` and handing the rest to
/// `mysql_async`.
fn parse_uri(uri: &str) -> Result<Target, Error> {
    let (base, query) = uri.split_once('?').unwrap_or((uri, ""));
    let mut mode = None;
    let mut ca = None;
    let mut prefer_socket_given = false;
    let mut kept = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "ssl-mode" => {
                let parsed = SslMode::parse(value).ok_or_else(|| {
                    Error::Url(format!(
                        "`ssl-mode' must be DISABLED, PREFERRED, REQUIRED, VERIFY_CA, or VERIFY_IDENTITY, not `{value}'"
                    ))
                })?;
                if mode.replace(parsed).is_some() {
                    return Err(Error::Url("`ssl-mode' is given twice".to_string()));
                }
            }
            "ssl-ca" => {
                if ca.replace(PathBuf::from(percent_decode(value)?)).is_some() {
                    return Err(Error::Url("`ssl-ca' is given twice".to_string()));
                }
            }
            key if DRIVER_TLS_PARAMETERS.contains(&key) => {
                return Err(Error::Url(format!(
                    "`{key}' is not taken here; use `ssl-mode'"
                )));
            }
            key => {
                prefer_socket_given |= key == "prefer_socket";
                kept.push(pair);
            }
        }
    }
    let rest: Cow<'_, str> = if kept.is_empty() {
        Cow::Borrowed(base)
    } else {
        Cow::Owned(format!("{base}?{}", kept.join("&")))
    };
    let mut opts = Opts::from_url(&rest).map_err(|error| Error::Url(error.to_string()))?;
    if !prefer_socket_given {
        opts = OptsBuilder::from_opts(opts).prefer_socket(false).into();
    }
    Ok(Target {
        opts,
        mode: mode.unwrap_or(SslMode::Preferred),
        ca,
    })
}

/// `mysql_async`'s TLS settings for `mode`, trusting `ca` beside the web roots.
fn ssl_opts(mode: SslMode, ca: Option<&PathBuf>) -> Option<SslOpts> {
    let base = SslOpts::default()
        .with_root_certs(ca.map(|path| vec![path.clone().into()]).unwrap_or_default());
    match mode {
        SslMode::Disabled => None,
        SslMode::Preferred | SslMode::Required => Some(base.with_danger_accept_invalid_certs(true)),
        SslMode::VerifyCa => Some(base.with_danger_skip_domain_validation(true)),
        SslMode::VerifyIdentity => Some(base),
    }
}

/// Open one `mysql_async` connection with `ssl`.
async fn open(target: &Target, ssl: Option<SslOpts>) -> mysql_async::Result<mysql_async::Conn> {
    // Diesel keeps its own statement cache, and counts found rather than changed
    // rows, as `AsyncMysqlConnection::establish` sets.
    let builder = OptsBuilder::from_opts(target.opts.clone())
        .stmt_cache_size(0)
        .client_found_rows(true)
        .ssl_opts(ssl);
    mysql_async::Conn::new(builder).await
}

/// Open one connection to `target`, with TLS as its `ssl-mode` says.
async fn establish(target: Target) -> ConnectionResult<AsyncMysqlConnection> {
    let ssl = ssl_opts(target.mode, target.ca.as_ref());
    let conn = match open(&target, ssl).await {
        Err(mysql_async::Error::Driver(DriverError::NoClientSslFlagFromServer))
            if target.mode == SslMode::Preferred =>
        {
            open(&target, None).await
        }
        other => other,
    }
    .map_err(|error| match error {
        mysql_async::Error::Url(error) => ConnectionError::InvalidConnectionUrl(error.to_string()),
        error => ConnectionError::BadConnection(error.to_string()),
    })?;
    AsyncMysqlConnection::try_from(conn).await
}

/// Build a pool without connecting yet. TLS is as the URL's `ssl-mode` says.
///
/// # Errors
///
/// Fails when the URL cannot be read or the pool's settings are invalid.
pub fn pool(config: &PoolConfig) -> Result<Pool, Error> {
    let target = parse_uri(&config.uri)?;
    let mut setup = ManagerConfig::default();
    setup.custom_setup = Box::new(move |_uri| Box::pin(establish(target.clone())));
    let manager = AsyncDieselConnectionManager::new_with_config(&config.uri, setup);
    // With no limit, a caller waits for ever behind a pool that is all in use.
    Ok(Pool::builder(manager)
        .max_size(config.max_connections)
        .wait_timeout(Some(config.wait_timeout))
        .create_timeout(Some(config.wait_timeout))
        .runtime(Runtime::Tokio1)
        .build()?)
}

/// Open a pool and prove it works with one connection.
///
/// # Errors
///
/// Fails when the URL cannot be read or the database cannot be reached.
pub async fn connect(config: &PoolConfig) -> Result<Pool, Error> {
    let pool = pool(config)?;
    ping(&pool).await?;
    Ok(pool)
}

/// Check that the database answers.
///
/// # Errors
///
/// Fails when it does not.
pub async fn ping(pool: &Pool) -> Result<(), Error> {
    let mut connection = pool.get().await?;
    diesel::sql_query("SELECT 1")
        .execute(&mut connection)
        .await?;
    Ok(())
}

/// Whether an error is a violated unique constraint, which a service usually
/// reports as `409` rather than `500`.
#[must_use]
pub fn is_unique_violation(error: &QueryError) -> bool {
    matches!(
        error,
        QueryError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)
    )
}

/// Quote a table name for use in SQL, with backticks.
///
/// Identifiers cannot be bound as parameters, so this is the one place names
/// reach SQL as text. Only lowercase ASCII letters, digits, and underscores
/// are accepted, starting with a letter or underscore.
///
/// ```
/// assert_eq!(axa_mysql_diesel::quote_identifier("users").as_deref(), Some("`users`"));
/// assert_eq!(axa_mysql_diesel::quote_identifier("users; DROP TABLE x"), None);
/// ```
#[must_use]
pub fn quote_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("`{name}`"))
}

/// Quote every name, failing on the first one [`quote_identifier`] refuses.
fn quote_all<S: AsRef<str>>(names: &[S]) -> Result<Vec<String>, Error> {
    names
        .iter()
        .map(|name| {
            quote_identifier(name.as_ref())
                .ok_or_else(|| Error::InvalidIdentifier(name.as_ref().to_string()))
        })
        .collect()
}

/// Run `work` on a migration harness. The harness blocks, so it gets a blocking
/// thread rather than stalling the runtime.
async fn with_harness<T, F>(pool: &Pool, work: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: FnOnce(&mut AsyncMigrationHarness<Connection>) -> Result<T, String> + Send + 'static,
{
    let connection = pool.get().await?;
    tokio::task::spawn_blocking(move || work(&mut AsyncMigrationHarness::new(connection)))
        .await
        .map_err(|error| Error::Migration(error.to_string()))?
        .map_err(Error::Migration)
}

/// Apply pending migrations, returning how many ran.
///
/// # Errors
///
/// Fails when a migration fails. MySQL commits each DDL statement as it runs, so
/// the statements before the failure stay applied.
pub async fn migrate<M>(pool: &Pool, migrations: M) -> Result<usize, Error>
where
    M: MigrationSource<Mysql> + Send + 'static,
{
    with_harness(pool, move |harness| {
        harness
            .run_pending_migrations(migrations)
            .map(|applied| applied.len())
            .map_err(|error| error.to_string())
    })
    .await
}

/// Revert the newest `steps` migrations, returning how many were reverted.
///
/// # Errors
///
/// Fails when a down migration fails.
pub async fn revert<M>(pool: &Pool, migrations: M, steps: usize) -> Result<usize, Error>
where
    M: MigrationSource<Mysql> + Clone + Send + 'static,
{
    with_harness(pool, move |harness| {
        let mut reverted = 0;
        while reverted < steps {
            let applied = harness
                .applied_migrations()
                .map_err(|error| error.to_string())?;
            if applied.is_empty() {
                break;
            }
            harness
                .revert_last_migration(migrations.clone())
                .map_err(|error| error.to_string())?;
            reverted += 1;
        }
        Ok(reverted)
    })
    .await
}

/// The names of migrations that have not run, in the order they would run.
///
/// # Errors
///
/// Fails on a database error.
pub async fn pending<M>(pool: &Pool, migrations: M) -> Result<Vec<String>, Error>
where
    M: MigrationSource<Mysql> + Send + 'static,
{
    with_harness(pool, move |harness| {
        let pending = harness
            .pending_migrations(migrations)
            .map_err(|error| error.to_string())?;
        Ok(pending
            .iter()
            .map(|migration| migration.name().to_string())
            .collect())
    })
    .await
}

/// Run `statements` on one connection with foreign key checks off, then turn
/// them back on. A connection whose checks could not be turned back on is taken
/// out of the pool rather than handed to the next caller.
async fn without_foreign_key_checks(pool: &Pool, statements: &[String]) -> Result<(), Error> {
    let mut connection = pool.get().await?;
    connection
        .batch_execute("SET FOREIGN_KEY_CHECKS = 0")
        .await?;
    let mut outcome = Ok(());
    for statement in statements {
        if let Err(error) = connection.batch_execute(statement).await {
            outcome = Err(error);
            break;
        }
    }
    let restored = connection.batch_execute("SET FOREIGN_KEY_CHECKS = 1").await;
    if restored.is_err() {
        drop(Object::take(connection));
    }
    outcome?;
    restored?;
    Ok(())
}

/// Empty `tables`, restarting their `AUTO_INCREMENT` counters. Each name goes
/// through [`quote_identifier`], and every name is checked before any table is
/// emptied.
///
/// Foreign key checks are off while the tables are emptied, on one connection,
/// so a table can be emptied while another references it. Rows in tables that
/// are not named stay, even when they point at emptied rows.
///
/// # Errors
///
/// Fails on an invalid name or a database error. MySQL commits each `TRUNCATE`
/// as it runs, so the tables before a failure stay emptied.
pub async fn truncate<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<(), Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let statements: Vec<String> = quote_all(tables)?
        .into_iter()
        .map(|table| format!("TRUNCATE TABLE {table}"))
        .collect();
    without_foreign_key_checks(pool, &statements).await
}

/// One name from a catalog query.
#[derive(QueryableByName)]
struct Name {
    #[diesel(sql_type = Text)]
    name: String,
}

/// Drop every base table in `DATABASE()`, then migrate from nothing. This gives
/// each test an empty schema, whatever tables it touched.
///
/// **This destroys all data.** It exists for tests, and a service must never
/// reach it outside the `test` environment.
///
/// Tables are dropped with foreign key checks off, so their order does not
/// matter. Views, routines, and events are left alone.
///
/// # Errors
///
/// Fails on a database error, a failed migration, or a name
/// [`quote_identifier`] refuses, which is checked before anything is dropped.
pub async fn dangerously_recreate<M>(pool: &Pool, migrations: M) -> Result<(), Error>
where
    M: MigrationSource<Mysql> + Send + 'static,
{
    let tables: Vec<Name> = {
        let mut connection = pool.get().await?;
        diesel::sql_query(
            "SELECT CAST(table_name AS CHAR) AS name FROM information_schema.tables
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE'",
        )
        .load(&mut connection)
        .await?
    };
    let names: Vec<&str> = tables.iter().map(|table| table.name.as_str()).collect();
    if !names.is_empty() {
        let names = quote_all(&names)?.join(", ");
        without_foreign_key_checks(pool, &[format!("DROP TABLE {names}")]).await?;
    }
    migrate(pool, migrations).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let text = format!("{:?}", PoolConfig::new("mysql://me:hunter2@db:3306/app"));
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
    }

    #[test]
    fn identifiers_are_quoted_with_backticks() {
        assert_eq!(quote_identifier("users").as_deref(), Some("`users`"));
    }

    #[test]
    fn unsafe_identifiers_are_refused() {
        for name in [
            "", "1users", "Users", "a b", "a`b", "a\"b", "a;b", "a.b", "users--",
        ] {
            assert_eq!(quote_identifier(name), None, "{name:?}");
        }
    }

    #[test]
    fn quote_all_fails_on_any_bad_name() {
        assert!(quote_all(&["good"]).is_ok());
        assert!(matches!(
            quote_all(&["good", "bad name"]),
            Err(Error::InvalidIdentifier(name)) if name == "bad name"
        ));
    }

    #[test]
    fn errors_that_are_not_from_the_database_are_not_unique_violations() {
        assert!(!is_unique_violation(&QueryError::NotFound));
    }

    #[test]
    fn ssl_mode_defaults_to_preferred() {
        let target = parse_uri("mysql://me:pw@db:3306/app").unwrap();
        assert_eq!(target.mode, SslMode::Preferred);
        assert_eq!(target.opts.db_name(), Some("app"));
    }

    #[test]
    fn ssl_mode_is_read_from_the_url_in_any_case() {
        for (value, mode) in [
            ("DISABLED", SslMode::Disabled),
            ("preferred", SslMode::Preferred),
            ("Required", SslMode::Required),
            ("VERIFY_CA", SslMode::VerifyCa),
            ("verify_identity", SslMode::VerifyIdentity),
        ] {
            let target = parse_uri(&format!("mysql://db/app?ssl-mode={value}")).unwrap();
            assert_eq!(target.mode, mode, "{value}");
        }
    }

    #[test]
    fn an_unknown_ssl_mode_is_refused() {
        assert!(matches!(
            parse_uri("mysql://db/app?ssl-mode=SOMETIMES"),
            Err(Error::Url(_))
        ));
        assert!(matches!(
            parse_uri("mysql://db/app?ssl-mode=REQUIRED&ssl-mode=DISABLED"),
            Err(Error::Url(_))
        ));
    }

    #[test]
    fn the_drivers_own_tls_parameters_are_refused() {
        for parameter in DRIVER_TLS_PARAMETERS {
            assert!(
                matches!(
                    parse_uri(&format!("mysql://db/app?{parameter}=true")),
                    Err(Error::Url(_))
                ),
                "{parameter}"
            );
        }
    }

    #[test]
    fn other_parameters_reach_the_driver() {
        let target = parse_uri("mysql://db/app?ssl-mode=REQUIRED&stmt_cache_size=7").unwrap();
        assert_eq!(target.mode, SslMode::Required);
        assert_eq!(target.opts.stmt_cache_size(), 7);
        assert!(matches!(
            parse_uri("mysql://db/app?charset=utf8mb4"),
            Err(Error::Url(_))
        ));
    }

    #[test]
    fn connections_use_tcp_unless_the_url_says_otherwise() {
        assert!(!parse_uri("mysql://db/app").unwrap().opts.prefer_socket());
        assert!(
            parse_uri("mysql://db/app?prefer_socket=true")
                .unwrap()
                .opts
                .prefer_socket()
        );
    }

    #[test]
    fn ssl_ca_is_read_from_the_url() {
        let target =
            parse_uri("mysql://db/app?ssl-ca=%2Fetc%2Fca%20root.pem&ssl-mode=VERIFY_CA").unwrap();
        assert_eq!(target.ca, Some(PathBuf::from("/etc/ca root.pem")));
        let ssl = ssl_opts(target.mode, target.ca.as_ref()).unwrap();
        assert_eq!(ssl.root_certs().len(), 1);
        assert!(matches!(
            parse_uri("mysql://db/app?ssl-ca=%zz"),
            Err(Error::Url(_))
        ));
    }

    #[test]
    fn each_ssl_mode_sets_the_drivers_tls_to_match() {
        assert!(ssl_opts(SslMode::Disabled, None).is_none());
        for mode in [SslMode::Preferred, SslMode::Required] {
            let ssl = ssl_opts(mode, None).unwrap();
            assert!(ssl.accept_invalid_certs(), "{mode:?}");
        }
        let ca = ssl_opts(SslMode::VerifyCa, None).unwrap();
        assert!(!ca.accept_invalid_certs());
        assert!(ca.skip_domain_validation());
        let identity = ssl_opts(SslMode::VerifyIdentity, None).unwrap();
        assert!(!identity.accept_invalid_certs());
        assert!(!identity.skip_domain_validation());
        assert!(!identity.disable_built_in_roots());
    }

    #[test]
    fn url_errors_leave_out_the_password() {
        let error = parse_uri("mysql://me:hunter2@db/app?nonsense=1").unwrap_err();
        assert!(!error.to_string().contains("hunter2"), "{error}");
    }
}
