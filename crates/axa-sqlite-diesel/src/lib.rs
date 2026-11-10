//! Diesel pooling, migrations, and test resets, on SQLite.
//!
//! This has the same shape as `axa-postgres-diesel` and the other Axa database
//! crates, so a service can move between them by changing names: a [`Pool`], an
//! [`Error`], [`PoolConfig`], [`connect`], [`ping`], [`migrate`], [`pending`],
//! [`revert`], [`truncate`], [`dangerously_recreate`], [`is_unique_violation`],
//! and [`quote_identifier`]. A service writes its own queries with Diesel as
//! usual.
//!
//! ```ignore
//! use axa_sqlite_diesel::{PoolConfig, connect, migrate};
//! use diesel_migrations::{EmbeddedMigrations, embed_migrations};
//!
//! const MIGRATIONS: EmbeddedMigrations = embed_migrations!();
//!
//! let pool = connect(&PoolConfig::new("sqlite://data/app.db")).await?;
//! migrate(&pool, MIGRATIONS).await?;
//! ```
//!
//! # What differs for SQLite
//!
//! - **The driver blocks.** Diesel's `SqliteConnection` is synchronous, so each
//!   pooled connection is a `diesel-async` [`SyncConnectionWrapper`] around one,
//!   which runs every query on Tokio's blocking threads.
//! - **SQLite is C.** It is compiled from source into the binary through
//!   `libsqlite3-sys` with its `bundled` feature, at the version `sqlx` pins, so a
//!   service that also uses `sqlx` links one SQLite. Nothing is needed on the host.
//! - **URLs** are `sqlite://path` (`sqlite:///absolute/path` for an absolute one,
//!   or `sqlite:path`) or `sqlite::memory:`. Anything else is refused with
//!   [`Error::InvalidUri`]. The path is handed to SQLite as a `file:` URI, so query
//!   parameters such as `?mode=ro` reach it, and a `%`, `?`, or `#` in the path
//!   must be percent-encoded. A file that does not exist is created, unless the
//!   URL says `mode=ro` or `mode=rw`; a file in a directory that does not exist is
//!   an error.
//! - **In memory**, `sqlite::memory:` gives each pool its own database, shared by
//!   all of that pool's connections, as `sqlx` does. It lasts while the pool holds
//!   at least one connection, and is gone when the last one closes.
//! - **Every connection** is opened with `PRAGMA foreign_keys = ON`, `PRAGMA
//!   busy_timeout` set to [`PoolConfig::busy_timeout`], and, for a file, `PRAGMA
//!   journal_mode = WAL`. SQLite allows one writer at a time: a second waits up to
//!   the busy timeout, then fails with `database is locked`.
//! - **No TLS.** There is no network, so there is nothing like the Postgres
//!   crate's `web_tls` or `pool_with_tls`. [`pool`] builds a pool without
//!   connecting.
//! - **[`truncate`]** deletes every row of each table and resets its
//!   `AUTOINCREMENT` counter in `sqlite_sequence`, in one transaction. SQLite has
//!   no `TRUNCATE ... CASCADE`, so where Postgres also empties the tables that
//!   reference one, here foreign keys are checked at the end of the transaction:
//!   the tables named may be in any order, but a row elsewhere that still points
//!   at a deleted one makes the whole call fail, and nothing is deleted.
//! - **[`dangerously_recreate`]** drops every table and view in `sqlite_schema`
//!   other than SQLite's own (`sqlite_%`), with foreign keys off, in one
//!   transaction. SQLite has no enum types to drop.

use std::{
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use deadpool::Runtime;
pub use diesel;
use diesel::{
    Connection as _, ConnectionError, ConnectionResult, QueryableByName, RunQueryDsl,
    SqliteConnection,
    connection::SimpleConnection,
    migration::MigrationSource,
    result::{DatabaseErrorKind, Error as QueryError},
    sql_types::{BigInt, Text},
    sqlite::Sqlite,
};
pub use diesel_async;
use diesel_async::{
    pooled_connection::{
        AsyncDieselConnectionManager, ManagerConfig,
        deadpool::{BuildError, Object, Pool as DeadPool, PoolError},
    },
    sync_connection_wrapper::SyncConnectionWrapper,
};
pub use diesel_migrations;
use diesel_migrations::MigrationHarness;

/// One SQLite connection, usable from async code.
pub type AsyncSqliteConnection = SyncConnectionWrapper<SqliteConnection>;

/// A connection pool.
pub type Pool = DeadPool<AsyncSqliteConnection>;

/// A pooled connection.
pub type Connection = Object<AsyncSqliteConnection>;

/// A failure in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
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
    /// The URL is neither `sqlite://path`, `sqlite:path`, nor `sqlite::memory:`.
    #[error("invalid SQLite URL: expected sqlite://path or sqlite::memory:")]
    InvalidUri,
}

/// Pool settings. The URL is left out of the `Debug` text, as it may name a
/// private path or, for other databases, hold a password.
#[derive(Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// URL, `sqlite://path` or `sqlite::memory:`.
    pub uri: String,
    /// Largest number of connections.
    pub max_connections: usize,
    /// How long a caller waits for a connection, whether a free one or a new one,
    /// before it gets an error.
    pub wait_timeout: Duration,
    /// How long a statement waits for another connection's lock on the database
    /// before it fails with `database is locked`.
    pub busy_timeout: Duration,
}

impl std::fmt::Debug for PoolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("max_connections", &self.max_connections)
            .field("wait_timeout", &self.wait_timeout)
            .field("busy_timeout", &self.busy_timeout)
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
            busy_timeout: Duration::from_secs(5),
        }
    }
}

/// What SQLite is asked to open.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    /// A `file:` URI.
    uri: String,
    /// Whether it is a file, which gets WAL, rather than memory.
    file: bool,
}

/// Turn an `sqlite:` URL into what SQLite opens, or `None` for anything else.
fn target(uri: &str) -> Option<Target> {
    static MEMORY: AtomicU64 = AtomicU64::new(0);
    if uri == "sqlite::memory:" {
        let n = MEMORY.fetch_add(1, Ordering::Relaxed);
        let id = process::id();
        return Some(Target {
            uri: format!("file:axa-memory-{id}-{n}?mode=memory&cache=shared"),
            file: false,
        });
    }
    let rest = uri
        .strip_prefix("sqlite://")
        .or_else(|| uri.strip_prefix("sqlite:"))?;
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    if path.is_empty() || path.starts_with(':') {
        return None;
    }
    let query = rest.split_once('?').map_or("", |(_, query)| query);
    let memory = query.split('&').any(|pair| pair == "mode=memory");
    Some(Target {
        uri: format!("file:{rest}"),
        file: !memory,
    })
}

/// The statements every new connection runs.
fn pragmas(file: bool, busy_timeout: Duration) -> String {
    let busy = i32::try_from(busy_timeout.as_millis()).unwrap_or(i32::MAX);
    let mut sql = format!("PRAGMA busy_timeout = {busy}; PRAGMA foreign_keys = ON;");
    if file {
        sql.push_str(" PRAGMA journal_mode = WAL;");
    }
    sql
}

/// Open one connection on a blocking thread and set it up.
async fn establish(uri: String, setup: String) -> ConnectionResult<AsyncSqliteConnection> {
    let connection = tokio::task::spawn_blocking(move || {
        let mut connection = SqliteConnection::establish(&uri)?;
        connection
            .batch_execute(&setup)
            .map_err(ConnectionError::CouldntSetupConfiguration)?;
        Ok::<_, ConnectionError>(connection)
    })
    .await
    .map_err(|error| ConnectionError::BadConnection(error.to_string()))??;
    Ok(SyncConnectionWrapper::new(connection))
}

/// Build a pool without connecting yet.
///
/// # Errors
///
/// Fails on a URL that is not `sqlite:`, or when the pool's settings are invalid.
pub fn pool(config: &PoolConfig) -> Result<Pool, Error> {
    let target = target(&config.uri).ok_or(Error::InvalidUri)?;
    let setup = pragmas(target.file, config.busy_timeout);
    let mut manager_config = ManagerConfig::default();
    manager_config.custom_setup =
        Box::new(move |uri| Box::pin(establish(uri.to_string(), setup.clone())));
    let manager = AsyncDieselConnectionManager::new_with_config(target.uri, manager_config);
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
/// Fails on a URL that is not `sqlite:`, or when the database cannot be opened.
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
    blocking(pool, |connection| Ok(connection.batch_execute("SELECT 1")?)).await
}

/// Whether an error is a violated unique constraint, which a service usually
/// reports as `409` rather than `500`. A duplicate primary key counts too, as
/// Diesel reports both the same way.
#[must_use]
pub fn is_unique_violation(error: &QueryError) -> bool {
    matches!(
        error,
        QueryError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)
    )
}

/// Quote a table name for use in SQL.
///
/// Identifiers cannot be bound as parameters, so this is the one place names
/// reach SQL as text. Only lowercase ASCII letters, digits, and underscores
/// are accepted, starting with a letter or underscore.
///
/// ```
/// assert_eq!(axa_sqlite_diesel::quote_identifier("users").as_deref(), Some("\"users\""));
/// assert_eq!(axa_sqlite_diesel::quote_identifier("users; DROP TABLE x"), None);
/// ```
#[must_use]
pub fn quote_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("\"{name}\""))
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

/// Run `work` on the synchronous connection inside a pooled one, on a blocking
/// thread.
async fn blocking<T, F>(pool: &Pool, work: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: FnOnce(&mut SqliteConnection) -> Result<T, Error> + Send + 'static,
{
    let mut connection = pool.get().await?;
    connection
        .spawn_blocking(move |inner| Ok(work(inner)))
        .await?
}

/// Turn a migration error into this crate's.
#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err hands it over by value"
)]
fn migration_error(error: Box<dyn std::error::Error + Send + Sync>) -> Error {
    Error::Migration(error.to_string())
}

/// Apply pending migrations, returning how many ran.
///
/// # Errors
///
/// Fails when a migration fails.
pub async fn migrate<M>(pool: &Pool, migrations: M) -> Result<usize, Error>
where
    M: MigrationSource<Sqlite> + Send + 'static,
{
    blocking(pool, move |connection| {
        connection
            .run_pending_migrations(migrations)
            .map(|applied| applied.len())
            .map_err(migration_error)
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
    M: MigrationSource<Sqlite> + Clone + Send + 'static,
{
    blocking(pool, move |connection| {
        let mut reverted = 0;
        while reverted < steps {
            let applied = connection.applied_migrations().map_err(migration_error)?;
            if applied.is_empty() {
                break;
            }
            connection
                .revert_last_migration(migrations.clone())
                .map_err(migration_error)?;
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
    M: MigrationSource<Sqlite> + Send + 'static,
{
    blocking(pool, move |connection| {
        let pending = connection
            .pending_migrations(migrations)
            .map_err(migration_error)?;
        Ok(pending
            .iter()
            .map(|migration| migration.name().to_string())
            .collect())
    })
    .await
}

/// One count from a catalog query.
#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

/// One name from a catalog query.
#[derive(QueryableByName)]
struct Name {
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    name: String,
}

/// Empty `tables`, resetting their `AUTOINCREMENT` counters, in one
/// transaction. Each name goes through [`quote_identifier`] before anything is
/// deleted.
///
/// Foreign keys are checked when the transaction ends, so the tables may be
/// named in any order, but a row in a table left out that still references a
/// deleted row fails the call, and nothing is deleted.
///
/// # Errors
///
/// Fails on an invalid name, a foreign key left dangling, or a database error.
pub async fn truncate<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<(), Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let quoted = quote_all(tables)?;
    let names: Vec<String> = tables
        .iter()
        .map(|name| name.as_ref().to_string())
        .collect();
    blocking(pool, move |connection| {
        connection
            .immediate_transaction(|connection| {
                connection.batch_execute("PRAGMA defer_foreign_keys = ON")?;
                for table in &quoted {
                    diesel::sql_query(format!("DELETE FROM {table}")).execute(connection)?;
                }
                let sequence: Count = diesel::sql_query(
                    "SELECT count(*) AS n FROM sqlite_schema WHERE name = 'sqlite_sequence'",
                )
                .get_result(connection)?;
                if sequence.n > 0 {
                    for name in &names {
                        diesel::sql_query("DELETE FROM sqlite_sequence WHERE name = ?")
                            .bind::<Text, _>(name)
                            .execute(connection)?;
                    }
                }
                Ok(())
            })
            .map_err(Error::Query)
    })
    .await
}

/// Drop every table and view, then migrate from nothing. This gives each test
/// an empty schema, whatever tables it touched.
///
/// **This destroys all data.** It exists for tests, and a service must never
/// reach it outside the `test` environment.
///
/// Every table and view in `sqlite_schema` is dropped other than SQLite's own
/// (`sqlite_%`), with foreign keys off, in one transaction, so a failure drops
/// nothing. Foreign keys are turned back on afterwards, whether or not it
/// worked.
///
/// # Errors
///
/// Fails on a database error, a failed migration, or a name
/// [`quote_identifier`] refuses.
pub async fn dangerously_recreate<M>(pool: &Pool, migrations: M) -> Result<(), Error>
where
    M: MigrationSource<Sqlite> + Send + 'static,
{
    blocking(pool, |connection| {
        connection.batch_execute("PRAGMA foreign_keys = OFF")?;
        let dropped = drop_everything(connection);
        let restored = connection.batch_execute("PRAGMA foreign_keys = ON");
        dropped?;
        restored?;
        Ok(())
    })
    .await?;
    migrate(pool, migrations).await?;
    Ok(())
}

/// Drop every table and view other than SQLite's own, views first.
fn drop_everything(connection: &mut SqliteConnection) -> Result<(), Error> {
    let objects: Vec<Name> = diesel::sql_query(
        "SELECT type AS kind, name FROM sqlite_schema
         WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
         ORDER BY type = 'table', name",
    )
    .load(connection)?;
    let names: Vec<&str> = objects.iter().map(|object| object.name.as_str()).collect();
    let quoted = quote_all(&names)?;
    connection.immediate_transaction(|connection| {
        for (object, name) in objects.iter().zip(&quoted) {
            let kind = if object.kind == "view" {
                "VIEW"
            } else {
                "TABLE"
            };
            // A virtual table's shadow tables go with it, so they may be gone.
            diesel::sql_query(format!("DROP {kind} IF EXISTS {name}")).execute(connection)?;
        }
        Ok::<_, Error>(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_text_leaves_out_the_url() {
        let text = format!("{:?}", PoolConfig::new("sqlite:///srv/secret-name.db"));
        assert!(!text.contains("secret-name"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
        assert!(text.contains("busy_timeout"), "{text}");
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote_identifier("users").as_deref(), Some("\"users\""));
        assert_eq!(quote_identifier("_x1").as_deref(), Some("\"_x1\""));
    }

    #[test]
    fn unsafe_identifiers_are_refused() {
        for name in [
            "", "1users", "Users", "a b", "a\"b", "a;b", "a.b", "users--", "a`b",
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
    fn sqlite_urls_become_file_uris() {
        assert_eq!(
            target("sqlite://data/app.db"),
            Some(Target {
                uri: "file:data/app.db".into(),
                file: true
            })
        );
        assert_eq!(
            target("sqlite:///srv/app.db?mode=ro"),
            Some(Target {
                uri: "file:/srv/app.db?mode=ro".into(),
                file: true
            })
        );
        assert_eq!(
            target("sqlite:app.db"),
            Some(Target {
                uri: "file:app.db".into(),
                file: true
            })
        );
        assert_eq!(
            target("sqlite://shared?mode=memory&cache=shared").map(|t| t.file),
            Some(false)
        );
    }

    #[test]
    fn each_memory_url_names_a_database_of_its_own() {
        let one = target("sqlite::memory:").unwrap();
        let two = target("sqlite::memory:").unwrap();
        assert!(!one.file);
        assert!(one.uri.contains("mode=memory&cache=shared"), "{}", one.uri);
        assert_ne!(one.uri, two.uri);
    }

    #[test]
    fn urls_that_are_not_sqlite_are_refused() {
        for uri in [
            "",
            "app.db",
            "postgres://localhost/app",
            "mysql://localhost/app",
            "sqlite:",
            "sqlite://",
            "sqlite://?mode=memory",
            "sqlite::nonsense:",
        ] {
            assert_eq!(target(uri), None, "{uri:?}");
        }
    }

    #[test]
    fn only_files_get_wal() {
        let file = pragmas(true, Duration::from_millis(1500));
        assert!(file.contains("busy_timeout = 1500"), "{file}");
        assert!(file.contains("foreign_keys = ON"), "{file}");
        assert!(file.contains("journal_mode = WAL"), "{file}");
        let memory = pragmas(false, Duration::from_secs(1));
        assert!(memory.contains("foreign_keys = ON"), "{memory}");
        assert!(!memory.contains("WAL"), "{memory}");
    }

    #[test]
    fn a_huge_busy_timeout_is_capped() {
        let sql = pragmas(false, Duration::from_secs(u64::MAX));
        assert!(
            sql.contains(&format!("busy_timeout = {}", i32::MAX)),
            "{sql}"
        );
    }
}
