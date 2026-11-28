//! SQL Server pooling, migrations, and test resets, in pure Rust.
//!
//! This has the same shape as `axa-postgres-diesel` and the other Axa database
//! crates, so a service can move between them by changing names. It uses the
//! [`mssql`] driver, a maintained fork of Tiberius, over Tokio, with TLS through
//! `rustls` and `ring`, so there is no ODBC, no OpenSSL, and no C in the database
//! path. A service writes its own queries with [`mssql`] as usual, on a
//! [`Connection`] from the [`Pool`].
//!
//! ```no_run
//! use axa_mssql::{Migration, PoolConfig, connect, migrate};
//!
//! const MIGRATIONS: &[Migration] = &[
//!     Migration::new(1, "create items", "CREATE TABLE items (id BIGINT IDENTITY PRIMARY KEY)"),
//! ];
//!
//! # async fn run() -> Result<(), axa_mssql::Error> {
//! let pool = connect(&PoolConfig::new(
//!     "server=tcp:db,1433;user=app;password=secret;database=app",
//! ))
//! .await?;
//! migrate(&pool, MIGRATIONS).await?;
//! let mut connection = pool.get().await?;
//! let rows = connection
//!     .query("SELECT id FROM items WHERE id > @P1", &[&0_i64])
//!     .await?
//!     .into_first_result()
//!     .await?;
//! # let _ = rows;
//! # Ok(())
//! # }
//! ```
//!
//! # Connection strings
//!
//! [`PoolConfig::new`] takes an ADO.NET connection string, as SQL Server and Azure
//! hand out, such as `server=tcp:db,1433;user=app;password=secret;database=app`, or a
//! JDBC one starting `jdbc:sqlserver://`. Keys are read without regard to case. A
//! value holding `;` is quoted, as in `password='a;b'`, and nothing is
//! percent-decoded. A named instance (`server=tcp:db\\instance`) needs the SQL Browser
//! service, which this crate does not reach: give the instance's port instead.
//!
//! # TLS
//!
//! Connections are encrypted unless the connection string says `encrypt=false`, and
//! the server's certificate must chain to a root in the operating system's store
//! and name the host. `TrustServerCertificateCA=/path/to/ca.pem` trusts a private
//! certificate authority beside the store, `HostNameInCertificate=name` checks
//! another name, and `encrypt=strict` uses TDS 8.0, where TLS starts before any SQL
//! Server bytes, on SQL Server 2022 and Azure. `TrustServerCertificate=true` checks
//! nothing, and is for a development server only.
//!
//! # What differs from Postgres
//!
//! - Names are quoted with brackets, `[items]`, rather than double quotes.
//! - Migrations are this crate's own: a list of [`Migration`]s, each with its
//!   T-SQL, usually from `include_str!`. Each runs in a transaction, as SQL Server
//!   rolls DDL back. A line holding only `GO` splits a migration into batches, as
//!   for `sqlcmd`, for a statement such as `CREATE VIEW` that must start its own.
//!   Applied migrations are kept in `_axa_migrations`, and a migration whose text
//!   changed after it was applied is refused.
//! - [`truncate`] deletes rather than truncates, as SQL Server refuses
//!   `TRUNCATE` on any table a foreign key refers to, and then restarts each
//!   identity column. A row in a table not named that refers to an emptied row
//!   stops it, and nothing is emptied.
//! - [`dangerously_recreate`] drops what migrations make in the default schema:
//!   tables, with the foreign keys between them, views, procedures, functions,
//!   sequences, synonyms, and types. Objects in other schemas are left.
//! - There is no read-only session as on Postgres: a reporting service connects as
//!   a login that may only read, or to a readable replica with
//!   `ApplicationIntent=ReadOnly`.

use std::{fmt, time::Duration};

use deadpool::{
    Runtime,
    managed::{self, Metrics, Object, RecycleResult},
};
pub use mssql;
use mssql::{Config, error::Error as DbError};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

/// A connection to SQL Server, as the pool holds it.
pub type Client = mssql::Client<Compat<TcpStream>>;

/// A connection pool.
pub type Pool = managed::Pool<Manager>;

/// A pooled connection, which is a [`Client`] to query with.
pub type Connection = Object<Manager>;

/// Why no connection could be had from the pool.
pub type PoolError = managed::PoolError<DbError>;

/// Why a pool could not be built.
pub use deadpool::managed::BuildError;

/// The name of the table that keeps the applied migrations.
pub const MIGRATIONS_TABLE: &str = "_axa_migrations";

/// The longest a migration's description may be, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 200;

/// How long [`migrate`] and [`revert`] wait for another process's migrations to
/// finish, in milliseconds.
const MIGRATION_LOCK_TIMEOUT_MS: u32 = 60_000;

/// A failure in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The connection string could not be read.
    #[error("invalid connection string: {0}")]
    ConnectionString(String),
    /// The pool could not be built.
    #[error(transparent)]
    Build(#[from] BuildError),
    /// No connection could be had.
    #[error(transparent)]
    Pool(#[from] PoolError),
    /// A query failed.
    #[error(transparent)]
    Query(#[from] DbError),
    /// A migration failed, or the list of migrations is not one that can run.
    #[error("migration failed: {0}")]
    Migration(String),
    /// A table name was refused by [`quote_identifier`].
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
}

impl Error {
    /// Whether this is the database unable to serve for now, rather than a refusal
    /// of what was asked: no connection to be had, one lost, or SQL Server's own
    /// code for it, as [`is_unavailable`] reads it.
    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Pool(_) => true,
            Self::Query(error) => is_unavailable(error),
            _ => false,
        }
    }
}

/// Pool settings. The connection string is left out of the `Debug` text, as it may
/// hold a password.
#[derive(Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// An ADO.NET connection string, `server=tcp:host,1433;user=...;password=...;database=...`,
    /// or a JDBC one, `jdbc:sqlserver://host:1433;...`.
    pub connection_string: String,
    /// Largest number of connections.
    pub max_connections: usize,
    /// How long a caller waits for a connection, whether a free one or a new one,
    /// before it gets an error.
    pub wait_timeout: Duration,
    /// How long a connection waits for the server to send anything before the query
    /// fails, or `None` to wait for ever. The connection is then out of step with the
    /// server, so the pool drops it. The wait starts again each time the server sends
    /// something, so it bounds a stalled server rather than a long query.
    pub command_timeout: Option<Duration>,
    /// SQL Server's `LOCK_TIMEOUT` on every connection, in milliseconds: how long a
    /// statement waits for a lock before it fails with error 1222. `0` waits for ever,
    /// which is SQL Server's own default.
    pub lock_timeout_ms: u32,
    /// `SET XACT_ABORT ON` on every connection, so that any error in a transaction
    /// rolls the whole transaction back, rather than only the statement, and a
    /// transaction is never left open behind a failed or timed-out statement.
    pub xact_abort: bool,
}

impl fmt::Debug for PoolConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolConfig")
            .field("max_connections", &self.max_connections)
            .field("wait_timeout", &self.wait_timeout)
            .field("command_timeout", &self.command_timeout)
            .field("lock_timeout_ms", &self.lock_timeout_ms)
            .field("xact_abort", &self.xact_abort)
            .finish_non_exhaustive()
    }
}

impl PoolConfig {
    /// Settings for `connection_string` with a small pool, a 30 second command
    /// timeout, as ADO.NET has, and `XACT_ABORT` on.
    #[must_use]
    pub fn new(connection_string: impl Into<String>) -> Self {
        Self {
            connection_string: connection_string.into(),
            max_connections: 10,
            wait_timeout: Duration::from_secs(5),
            command_timeout: Some(Duration::from_secs(30)),
            lock_timeout_ms: 0,
            xact_abort: true,
        }
    }

    /// The one batch that sets the session settings on a new connection, or `None`
    /// when there are none to set.
    #[must_use]
    pub fn session_statement(&self) -> Option<String> {
        session_statement(self.lock_timeout_ms, self.xact_abort)
    }
}

/// `SET` for each session setting asked for, as one batch, or `None`. The values are
/// a number and a fixed word, so nothing a caller sends reaches it.
fn session_statement(lock_timeout_ms: u32, xact_abort: bool) -> Option<String> {
    let mut settings = Vec::new();
    if xact_abort {
        settings.push("SET XACT_ABORT ON".to_string());
    }
    if lock_timeout_ms > 0 {
        settings.push(format!("SET LOCK_TIMEOUT {lock_timeout_ms}"));
    }
    (!settings.is_empty()).then(|| settings.join("; "))
}

/// Read a connection string, ADO.NET or JDBC. The driver's own error is left out,
/// as it may quote the string, password and all.
fn parse_connection_string(text: &str) -> Result<Config, Error> {
    // The driver reads the `jdbc:` scheme in lower case only.
    let parsed = match text.get(..5).zip(text.get(5..)) {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("jdbc:") => {
            Config::from_jdbc_string(&format!("jdbc:{rest}"))
        }
        _ => Config::from_ado_string(text),
    };
    parsed.map_err(|_| {
        Error::ConnectionString(
            "it is not an ADO.NET or JDBC connection string this driver reads".to_string(),
        )
    })
}

/// Opens and checks the pool's connections.
pub struct Manager {
    config: Config,
    /// The session settings, run on each new connection.
    setup: Option<String>,
}

impl fmt::Debug for Manager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Manager")
            .field("setup", &self.setup)
            .finish_non_exhaustive()
    }
}

/// Run `sql`, one batch, and read every result it sends, so that the connection is
/// ready for the next.
async fn run(client: &mut Client, sql: &str) -> Result<(), DbError> {
    client.simple_query(sql).await?.into_results().await?;
    Ok(())
}

/// Open one connection to the server `config` names.
async fn open_at(config: Config) -> Result<Client, DbError> {
    let tcp = TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    // The driver's handshake is a large future, so it waits on the heap.
    Box::pin(mssql::Client::connect(config, tcp.compat_write())).await
}

/// Open one connection, following the server once when it sends the connection on
/// elsewhere, as Azure SQL does.
async fn open(config: &Config) -> Result<Client, DbError> {
    match open_at(config.clone()).await {
        Err(DbError::Routing { host, port }) => {
            let mut routed = config.clone();
            routed.host(host);
            routed.port(port);
            open_at(routed).await
        }
        other => other,
    }
}

/// What a connection runs before it goes back into use: it rolls back a transaction
/// a caller left open, and proves the connection still answers.
const RECYCLE: &str = "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; SELECT 1";

impl managed::Manager for Manager {
    type Type = Client;
    type Error = DbError;

    async fn create(&self) -> Result<Client, DbError> {
        let mut client = open(&self.config).await?;
        if let Some(setup) = &self.setup {
            run(&mut client, setup).await?;
        }
        Ok(client)
    }

    async fn recycle(&self, client: &mut Client, _: &Metrics) -> RecycleResult<DbError> {
        run(client, RECYCLE).await?;
        Ok(())
    }
}

/// Build a pool without connecting yet.
///
/// # Errors
///
/// Fails when the connection string cannot be read or the pool's settings are
/// invalid.
pub fn pool(config: &PoolConfig) -> Result<Pool, Error> {
    let mut parsed = parse_connection_string(&config.connection_string)?;
    parsed.command_timeout(config.command_timeout);
    let manager = Manager {
        config: parsed,
        setup: config.session_statement(),
    };
    // With no limit, a caller waits for ever behind a pool that is all in use.
    Ok(Pool::builder(manager)
        .max_size(config.max_connections)
        .wait_timeout(Some(config.wait_timeout))
        .create_timeout(Some(config.wait_timeout))
        .recycle_timeout(Some(config.wait_timeout))
        .runtime(Runtime::Tokio1)
        .build()?)
}

/// Open a pool and prove it works with one connection.
///
/// # Errors
///
/// Fails when the connection string cannot be read or the database cannot be
/// reached.
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
    run(&mut connection, "SELECT 1").await?;
    Ok(())
}

/// A constraint a write broke, as SQL Server's error number names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Violation {
    /// A unique constraint or unique index: errors 2627 and 2601.
    Unique,
    /// A foreign key, from either end: error 547 naming a `FOREIGN KEY` or
    /// `REFERENCE` constraint.
    ForeignKey,
    /// A `CHECK` constraint: error 547 naming one.
    Check,
    /// A `NULL` where the column allows none: error 515.
    NotNull,
}

/// The constraint `error` says a write broke, if it is one.
#[must_use]
pub fn violation(error: &DbError) -> Option<Violation> {
    let DbError::Server(token) = error else {
        return None;
    };
    match token.code() {
        2627 | 2601 => Some(Violation::Unique),
        515 => Some(Violation::NotNull),
        // One number for both: the message names the kind of constraint, in words
        // SQL Server does not translate.
        547 if token.message().contains("CHECK constraint") => Some(Violation::Check),
        547 => Some(Violation::ForeignKey),
        _ => None,
    }
}

/// Whether an error is a violated unique constraint, which a service usually
/// reports as `409` rather than `500`.
#[must_use]
pub fn is_unique_violation(error: &DbError) -> bool {
    violation(error) == Some(Violation::Unique)
}

/// SQL Server's error numbers for a database that cannot serve for now, rather than
/// one that refused this request: a read-only database or replica, a database not
/// open or in recovery, a server shutting down or paused, or Azure SQL's busy,
/// throttled, and not-available answers.
const UNAVAILABLE: &[u32] = &[
    // A read-only database, a secondary replica not readable, or one not yet joined.
    3906, 976, 978, 983,
    // The database cannot be opened, is recovering, or the server is shutting down.
    4060, 922, 6005, 17142,
    // Azure SQL: not available, busy, throttled, or out of resources.
    40613, 40197, 40501, 40540, 10928, 10929, 49918, 49919, 49920,
];

/// Whether `error` is the database unable to serve for now: a connection lost, TLS
/// that failed, or one of SQL Server's own numbers for it. A deadlock or a lock
/// timeout is not, as the same request may meet it again.
#[must_use]
pub fn is_unavailable(error: &DbError) -> bool {
    match error {
        DbError::Io { .. } | DbError::Tls(_) | DbError::Routing { .. } => true,
        DbError::Server(token) => UNAVAILABLE.contains(&token.code()),
        _ => false,
    }
}

/// Quote a table name for use in SQL, with brackets.
///
/// Identifiers cannot be bound as parameters, so this is the one place names
/// reach SQL as text. Only lowercase ASCII letters, digits, and underscores
/// are accepted, starting with a letter or underscore, up to 128 characters.
///
/// ```
/// assert_eq!(axa_mssql::quote_identifier("users").as_deref(), Some("[users]"));
/// assert_eq!(axa_mssql::quote_identifier("users; DROP TABLE x"), None);
/// ```
#[must_use]
pub fn quote_identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let valid = name.len() <= 128
        && (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("[{name}]"))
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

/// One migration: its version, which orders it, a description, and its T-SQL, with
/// the T-SQL that undoes it if it can be undone.
///
/// ```
/// use axa_mssql::Migration;
///
/// const MIGRATIONS: &[Migration] = &[
///     Migration::new(1, "create items", "CREATE TABLE items (id BIGINT IDENTITY PRIMARY KEY)")
///         .with_down("DROP TABLE items"),
/// ];
/// assert_eq!(MIGRATIONS[0].name(), "1_create items");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Migration {
    /// Its version, above zero, and above the version of each migration before it.
    pub version: i64,
    /// What it does, in up to [`MAX_DESCRIPTION_CHARS`] characters.
    pub description: &'static str,
    /// The T-SQL that applies it. A line holding only `GO` starts another batch.
    pub up: &'static str,
    /// The T-SQL that undoes it, if it can be undone.
    pub down: Option<&'static str>,
}

impl Migration {
    /// A migration that cannot be undone.
    #[must_use]
    pub const fn new(version: i64, description: &'static str, up: &'static str) -> Self {
        Self {
            version,
            description,
            up,
            down: None,
        }
    }

    /// This migration, undone by `down`.
    #[must_use]
    pub const fn with_down(mut self, down: &'static str) -> Self {
        self.down = Some(down);
        self
    }

    /// Its name, `version_description`, as [`pending`] lists it.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{}_{}", self.version, self.description)
    }

    /// A fingerprint of its `up` text, kept when it is applied, so that a migration
    /// changed after that is found: 64-bit FNV-1a.
    #[must_use]
    pub fn checksum(&self) -> i64 {
        let hash = self
            .up
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
        i64::from_ne_bytes(hash.to_ne_bytes())
    }
}

/// Refuse a list of migrations that cannot run: a version not above zero or not
/// above the one before it, or a description that is empty, too long, or holds a
/// control character.
fn check_migrations(migrations: &[Migration]) -> Result<(), Error> {
    let mut last = 0_i64;
    for migration in migrations {
        if migration.version <= last {
            return Err(Error::Migration(format!(
                "migration {} must have a version above zero and above the one before it",
                migration.version
            )));
        }
        last = migration.version;
        let description = migration.description;
        if description.is_empty()
            || description.chars().count() > MAX_DESCRIPTION_CHARS
            || description.chars().any(char::is_control)
        {
            return Err(Error::Migration(format!(
                "migration {} needs a description of 1 to {MAX_DESCRIPTION_CHARS} characters with no control characters",
                migration.version
            )));
        }
    }
    Ok(())
}

/// The batches of a migration's T-SQL, split on lines that hold only `GO`, leaving
/// out any that are only white space.
fn batches(sql: &str) -> Vec<String> {
    let mut batches = Vec::new();
    let mut current = String::new();
    for line in sql.lines() {
        if line.trim().eq_ignore_ascii_case("go") {
            batches.push(std::mem::take(&mut current));
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    batches.push(current);
    batches.retain(|batch| !batch.trim().is_empty());
    batches
}

const CREATE_MIGRATIONS_TABLE: &str = "IF OBJECT_ID(N'_axa_migrations', N'U') IS NULL
    CREATE TABLE _axa_migrations (
        version BIGINT NOT NULL PRIMARY KEY,
        description NVARCHAR(200) NOT NULL,
        checksum BIGINT NOT NULL,
        applied_at DATETIME2 NOT NULL DEFAULT SYSUTCDATETIME()
    )";

const APPLIED: &str = "IF OBJECT_ID(N'_axa_migrations', N'U') IS NOT NULL
    SELECT version, checksum FROM _axa_migrations ORDER BY version";

/// The version and checksum of every applied migration, oldest first, or none when
/// nothing has been migrated.
async fn applied(client: &mut Client) -> Result<Vec<(i64, i64)>, Error> {
    let rows = client
        .simple_query(APPLIED)
        .await?
        .into_first_result()
        .await?;
    rows.iter()
        .map(|row| {
            let version = row.try_get::<i64, _>(0)?;
            let checksum = row.try_get::<i64, _>(1)?;
            version.zip(checksum).ok_or_else(|| {
                Error::Migration(format!("{MIGRATIONS_TABLE} holds a row with no version"))
            })
        })
        .collect()
}

/// Refuse to go on when a migration that ran has changed since.
fn check_unchanged(migrations: &[Migration], applied: &[(i64, i64)]) -> Result<(), Error> {
    for (version, checksum) in applied {
        if let Some(migration) = migrations.iter().find(|m| m.version == *version)
            && migration.checksum() != *checksum
        {
            return Err(Error::Migration(format!(
                "migration {} has changed since it was applied",
                migration.name()
            )));
        }
    }
    Ok(())
}

/// Hold SQL Server's application lock for migrations on this connection, so that
/// services booting together migrate one at a time.
async fn lock_migrations(client: &mut Client) -> Result<(), Error> {
    let sql = format!(
        "DECLARE @result INT;
         EXEC @result = sp_getapplock @Resource = N'axa_migrations', @LockMode = 'Exclusive',
             @LockOwner = 'Session', @LockTimeout = {MIGRATION_LOCK_TIMEOUT_MS};
         SELECT @result"
    );
    let row = client.simple_query(sql).await?.into_row().await?;
    let result = row.and_then(|row| row.get::<i32, _>(0));
    match result {
        Some(result) if result >= 0_i32 => Ok(()),
        _ => Err(Error::Migration(
            "another process held the migration lock for too long".to_string(),
        )),
    }
}

const UNLOCK_MIGRATIONS: &str =
    "EXEC sp_releaseapplock @Resource = N'axa_migrations', @LockOwner = 'Session'";

/// Run `work` on one connection holding the migration lock, and let the lock go
/// after. A connection whose lock could not be let go is dropped rather than
/// handed to the next caller, which lets the lock go with it.
async fn with_migration_lock<T>(
    pool: &Pool,
    work: impl AsyncFnOnce(&mut Client) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut connection = pool.get().await?;
    lock_migrations(&mut connection).await?;
    let outcome = work(&mut connection).await;
    if run(&mut connection, UNLOCK_MIGRATIONS).await.is_err() {
        drop(Object::take(connection));
    }
    outcome
}

/// Run `work` in a transaction on `client`, rolling it back on any failure.
async fn in_transaction(
    client: &mut Client,
    work: impl AsyncFnOnce(&mut Client) -> Result<(), Error>,
) -> Result<(), Error> {
    run(client, "BEGIN TRANSACTION").await?;
    match work(client).await {
        Ok(()) => {
            run(client, "COMMIT TRANSACTION").await?;
            Ok(())
        }
        Err(error) => {
            // With `XACT_ABORT` on, SQL Server has already rolled it back.
            let _ = run(client, "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION").await;
            Err(error)
        }
    }
}

/// Apply one migration and record it, in one transaction.
async fn apply(client: &mut Client, migration: &Migration) -> Result<(), Error> {
    in_transaction(client, async |client| {
        for batch in batches(migration.up) {
            run(client, &batch).await?;
        }
        client
            .execute(
                "INSERT INTO _axa_migrations (version, description, checksum) VALUES (@P1, @P2, @P3)",
                &[&migration.version, &migration.description, &migration.checksum()],
            )
            .await?;
        Ok(())
    })
    .await
    .map_err(|error| Error::Migration(format!("{}: {error}", migration.name())))
}

/// Apply pending migrations, oldest first, returning how many ran. Each runs in a
/// transaction of its own, under a lock that makes services booting together take
/// turns. A migration applied before and missing from `migrations`, as during a
/// rolling deploy, is left alone.
///
/// # Errors
///
/// Fails on a list that cannot run, a migration changed since it was applied, or a
/// failed migration, which is rolled back, and then no later one is tried.
pub async fn migrate(pool: &Pool, migrations: &[Migration]) -> Result<usize, Error> {
    check_migrations(migrations)?;
    with_migration_lock(pool, async |client| {
        run(client, CREATE_MIGRATIONS_TABLE).await?;
        let applied = applied(client).await?;
        check_unchanged(migrations, &applied)?;
        let mut ran = 0_usize;
        for migration in migrations {
            if !applied
                .iter()
                .any(|(version, _)| *version == migration.version)
            {
                apply(client, migration).await?;
                ran = ran.checked_add(1).unwrap_or(usize::MAX);
            }
        }
        Ok(ran)
    })
    .await
}

/// Revert the newest `steps` applied migrations, newest first, returning how many
/// were reverted, which is fewer when fewer were applied.
///
/// # Errors
///
/// Fails when an applied migration is missing from `migrations`, has no `down`,
/// or has changed, or when a down migration fails, which is rolled back.
pub async fn revert(pool: &Pool, migrations: &[Migration], steps: usize) -> Result<usize, Error> {
    check_migrations(migrations)?;
    with_migration_lock(pool, async |client| {
        let applied = applied(client).await?;
        check_unchanged(migrations, &applied)?;
        let mut reverted = 0_usize;
        for (version, _) in applied.iter().rev().take(steps) {
            let migration = migrations
                .iter()
                .find(|m| m.version == *version)
                .ok_or_else(|| {
                    Error::Migration(format!("applied migration {version} is not in the list"))
                })?;
            let down = migration.down.ok_or_else(|| {
                Error::Migration(format!("migration {} cannot be undone", migration.name()))
            })?;
            in_transaction(client, async |client| {
                for batch in batches(down) {
                    run(client, &batch).await?;
                }
                client
                    .execute(
                        "DELETE FROM _axa_migrations WHERE version = @P1",
                        &[version],
                    )
                    .await?;
                Ok(())
            })
            .await
            .map_err(|error| Error::Migration(format!("{}: {error}", migration.name())))?;
            reverted = reverted.checked_add(1).unwrap_or(usize::MAX);
        }
        Ok(reverted)
    })
    .await
}

/// The names of migrations that have not run, in the order they would run.
///
/// # Errors
///
/// Fails on a list that cannot run or a database error.
pub async fn pending(pool: &Pool, migrations: &[Migration]) -> Result<Vec<String>, Error> {
    check_migrations(migrations)?;
    let mut connection = pool.get().await?;
    let applied = applied(&mut connection).await?;
    Ok(migrations
        .iter()
        .filter(|migration| {
            !applied
                .iter()
                .any(|(version, _)| *version == migration.version)
        })
        .map(Migration::name)
        .collect())
}

/// SQL Server's error number for a statement a foreign key stopped.
const CONSTRAINT_CONFLICT: u32 = 547;

/// Restart `table`'s identity column, if it has one that has handed out a value,
/// so that the next row gets the column's first value again.
fn reseed(table: &str) -> String {
    format!(
        "DECLARE @next BIGINT = (SELECT CAST(seed_value AS BIGINT) - CAST(increment_value AS BIGINT)
             FROM sys.identity_columns WHERE object_id = OBJECT_ID(N'{table}') AND last_value IS NOT NULL);
         IF @next IS NOT NULL DBCC CHECKIDENT (N'{table}', RESEED, @next) WITH NO_INFOMSGS"
    )
}

/// Delete every row of `tables`, in one transaction, in whatever order the foreign
/// keys between them allow: each round deletes what it can, and what a foreign key
/// stopped waits for the next round.
async fn delete_all(client: &mut Client, tables: &[String]) -> Result<(), Error> {
    let mut left: Vec<&String> = tables.iter().collect();
    while !left.is_empty() {
        let mut stopped = Vec::new();
        let mut last = None;
        for table in left.iter().copied() {
            match run(client, &format!("DELETE FROM {table}")).await {
                Ok(()) => {}
                Err(error) if error.code() == Some(CONSTRAINT_CONFLICT) => {
                    stopped.push(table);
                    last = Some(error);
                }
                Err(error) => return Err(error.into()),
            }
        }
        if stopped.len() == left.len()
            && let Some(error) = last
        {
            // Nothing could go: a row outside these tables refers to one in them.
            return Err(error.into());
        }
        left = stopped;
    }
    for table in tables {
        run(client, &reseed(table)).await?;
    }
    Ok(())
}

/// Empty `tables` and restart their identity columns. Each name goes through
/// [`quote_identifier`], and every name is checked before any table is emptied.
///
/// The rows go in one transaction, in any order the foreign keys between the named
/// tables allow. A row in a table that is not named and refers to an emptied row
/// stops it, and then nothing is emptied.
///
/// # Errors
///
/// Fails on an invalid name, a row elsewhere that refers to one here, or a database
/// error.
pub async fn truncate<S: AsRef<str>>(pool: &Pool, tables: &[S]) -> Result<(), Error> {
    if tables.is_empty() {
        return Ok(());
    }
    let tables = quote_all(tables)?;
    let mut connection = pool.get().await?;
    // A statement a foreign key stops must leave the transaction open for the next
    // round, which `XACT_ABORT` would not.
    run(&mut connection, "SET XACT_ABORT OFF").await?;
    let outcome = in_transaction(&mut connection, async |client| {
        delete_all(client, &tables).await
    })
    .await;
    let restored = match &pool.manager().setup {
        Some(setup) => run(&mut connection, setup).await,
        None => Ok(()),
    };
    if restored.is_err() {
        drop(Object::take(connection));
    }
    outcome?;
    restored?;
    Ok(())
}

/// Drop everything in the default schema that migrations make: system-versioned
/// tables first made plain, then foreign keys, views, procedures, tables, functions,
/// sequences, synonyms, and types, in that order, as one batch the server builds
/// from its own catalog.
const DROP_ALL: &str = "DECLARE @sql NVARCHAR(MAX) = N'';
SELECT @sql += N'ALTER TABLE ' + QUOTENAME(SCHEMA_NAME(schema_id)) + N'.' + QUOTENAME(name)
    + N' SET (SYSTEM_VERSIONING = OFF);'
    FROM sys.tables WHERE schema_id = SCHEMA_ID() AND temporal_type = 2;
SELECT @sql += N'ALTER TABLE ' + QUOTENAME(SCHEMA_NAME(t.schema_id)) + N'.' + QUOTENAME(t.name)
    + N' DROP CONSTRAINT ' + QUOTENAME(f.name) + N';'
    FROM sys.foreign_keys AS f JOIN sys.tables AS t ON f.parent_object_id = t.object_id
    WHERE t.schema_id = SCHEMA_ID();
SELECT @sql += N'DROP ' + kind + N' ' + QUOTENAME(SCHEMA_NAME(schema_id)) + N'.' + QUOTENAME(name) + N';'
FROM (
    SELECT schema_id, name, is_ms_shipped,
        CASE type WHEN 'V' THEN N'VIEW' WHEN 'P' THEN N'PROCEDURE' WHEN 'U' THEN N'TABLE'
            WHEN 'FN' THEN N'FUNCTION' WHEN 'IF' THEN N'FUNCTION' WHEN 'TF' THEN N'FUNCTION'
            WHEN 'SO' THEN N'SEQUENCE' WHEN 'SN' THEN N'SYNONYM' END AS kind,
        CASE type WHEN 'V' THEN 1 WHEN 'P' THEN 2 WHEN 'U' THEN 3 WHEN 'SO' THEN 5 WHEN 'SN' THEN 6
            ELSE 4 END AS step
    FROM sys.objects WHERE type IN ('V', 'P', 'U', 'FN', 'IF', 'TF', 'SO', 'SN')
) AS found
WHERE schema_id = SCHEMA_ID() AND is_ms_shipped = 0
ORDER BY step;
SELECT @sql += N'DROP TYPE ' + QUOTENAME(SCHEMA_NAME(schema_id)) + N'.' + QUOTENAME(name) + N';'
    FROM sys.types WHERE is_user_defined = 1 AND schema_id = SCHEMA_ID();
EXEC sp_executesql @sql";

/// Drop everything migrations make in the default schema, then migrate from
/// nothing. This gives each test an empty schema, whatever it touched.
///
/// **This destroys all data.** It exists for tests, and a service must never
/// reach it outside the `test` environment.
///
/// Tables, views, procedures, functions, sequences, synonyms, and types go, with the
/// foreign keys between the tables dropped first, so their order does not matter.
/// Objects in another schema are left alone.
///
/// # Errors
///
/// Fails on a database error, or a failed migration.
pub async fn dangerously_recreate(pool: &Pool, migrations: &[Migration]) -> Result<(), Error> {
    check_migrations(migrations)?;
    {
        let mut connection = pool.get().await?;
        run(&mut connection, DROP_ALL).await?;
    }
    migrate(pool, migrations).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_settings_become_one_batch() {
        assert_eq!(session_statement(0, false), None);
        assert_eq!(
            session_statement(0, true).as_deref(),
            Some("SET XACT_ABORT ON")
        );
        assert_eq!(
            session_statement(5000, false).as_deref(),
            Some("SET LOCK_TIMEOUT 5000")
        );
        assert_eq!(
            session_statement(5000, true).as_deref(),
            Some("SET XACT_ABORT ON; SET LOCK_TIMEOUT 5000")
        );
        assert_eq!(
            PoolConfig::new("server=db").session_statement().as_deref(),
            Some("SET XACT_ABORT ON")
        );
    }

    #[test]
    fn debug_text_leaves_out_the_connection_string() {
        let config = PoolConfig::new("server=tcp:db,1433;user=me;password=hunter2");
        let text = format!("{config:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("max_connections"), "{text}");
        let manager = pool(&config).unwrap();
        let text = format!("{:?}", manager.manager());
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.starts_with("Manager"), "{text}");
        assert!(text.contains("SET XACT_ABORT ON"), "{text}");
    }

    #[test]
    fn ado_and_jdbc_strings_are_read() {
        let ado =
            parse_connection_string("server=tcp:db,1444;user=me;password=pw;database=app").unwrap();
        assert_eq!(ado.get_addr(), "db:1444");
        let jdbc =
            parse_connection_string("jdbc:sqlserver://db:1445;user=me;password=pw;database=app")
                .unwrap();
        assert_eq!(jdbc.get_addr(), "db:1445");
        let upper =
            parse_connection_string("JDBC:sqlserver://db:1446;user=me;password=pw").unwrap();
        assert_eq!(upper.get_addr(), "db:1446");
    }

    #[test]
    fn a_bad_connection_string_is_refused_without_its_password() {
        let error = parse_connection_string("server=tcp:db,notaport;password=hunter2").unwrap_err();
        assert!(matches!(error, Error::ConnectionString(_)), "{error:?}");
        assert!(!error.to_string().contains("hunter2"), "{error}");
        assert!(pool(&PoolConfig::new("server=tcp:db,notaport")).is_err());
    }

    #[test]
    fn the_command_timeout_reaches_the_driver() {
        let mut config = PoolConfig::new("server=tcp:db,1433");
        config.command_timeout = Some(Duration::from_secs(7));
        assert_eq!(
            pool(&config)
                .unwrap()
                .manager()
                .config
                .get_command_timeout(),
            Some(Duration::from_secs(7))
        );
        config.command_timeout = None;
        assert_eq!(
            pool(&config)
                .unwrap()
                .manager()
                .config
                .get_command_timeout(),
            None
        );
    }

    #[test]
    fn identifiers_are_quoted_with_brackets() {
        assert_eq!(quote_identifier("users").as_deref(), Some("[users]"));
        assert_eq!(
            quote_identifier("_axa_migrations").as_deref(),
            Some("[_axa_migrations]")
        );
        assert!(quote_identifier(&"a".repeat(128)).is_some());
        assert_eq!(quote_identifier(&"a".repeat(129)), None);
    }

    #[test]
    fn unsafe_identifiers_are_refused() {
        for name in [
            "", "1users", "Users", "a b", "a]b", "a[b", "a\"b", "a;b", "a.b", "users--", "é",
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
    fn a_migration_splits_on_go_lines_only() {
        assert_eq!(
            batches("CREATE TABLE a (x INT)"),
            ["CREATE TABLE a (x INT)\n"]
        );
        assert_eq!(
            batches("CREATE TABLE a (x INT)\n  go  \nCREATE VIEW v AS SELECT x FROM a\nGO\n\n"),
            [
                "CREATE TABLE a (x INT)\n",
                "CREATE VIEW v AS SELECT x FROM a\n"
            ]
        );
        // `GO` inside a line, or as part of a word, is T-SQL like any other.
        assert_eq!(
            batches("SELECT 'GO'\nGOTO done"),
            ["SELECT 'GO'\nGOTO done\n"]
        );
        assert_eq!(batches("\n  \nGO\n"), Vec::<String>::new());
    }

    #[test]
    fn a_checksum_follows_the_up_text_only() {
        let one = Migration::new(1, "a", "CREATE TABLE a (x INT)");
        assert_eq!(
            one.checksum(),
            Migration::new(2, "b", one.up).with_down("x").checksum()
        );
        assert_ne!(
            one.checksum(),
            Migration::new(1, "a", "CREATE TABLE a (y INT)").checksum()
        );
        // FNV-1a of nothing is its offset basis.
        assert_eq!(
            Migration::new(1, "a", "").checksum(),
            i64::from_ne_bytes(0xcbf2_9ce4_8422_2325_u64.to_ne_bytes())
        );
    }

    #[test]
    fn a_migration_names_itself_and_may_have_a_down() {
        let migration = Migration::new(3, "add notes", "CREATE TABLE notes (x INT)");
        assert_eq!(migration.name(), "3_add notes");
        assert_eq!(migration.down, None);
        assert_eq!(
            migration.with_down("DROP TABLE notes").down,
            Some("DROP TABLE notes")
        );
    }

    #[test]
    fn a_list_of_migrations_that_cannot_run_is_refused() {
        let ok = |version, description| Migration::new(version, description, "SELECT 1");
        assert!(check_migrations(&[]).is_ok());
        assert!(check_migrations(&[ok(1, "a"), ok(5, "b")]).is_ok());
        let long = "d".repeat(MAX_DESCRIPTION_CHARS);
        let leaked: &'static str = long.leak();
        assert!(check_migrations(&[ok(1, leaked)]).is_ok());
        let longer: &'static str = "d".repeat(MAX_DESCRIPTION_CHARS + 1).leak();
        for bad in [
            vec![ok(0, "a")],
            vec![ok(-1, "a")],
            vec![ok(2, "a"), ok(2, "b")],
            vec![ok(2, "a"), ok(1, "b")],
            vec![ok(1, "")],
            vec![ok(1, longer)],
            vec![ok(1, "a\nb")],
        ] {
            assert!(
                matches!(check_migrations(&bad), Err(Error::Migration(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_changed_migration_is_found_and_one_left_out_is_not() {
        let one = Migration::new(1, "a", "CREATE TABLE a (x INT)");
        let applied = [(1_i64, one.checksum()), (9_i64, 42_i64)];
        assert!(check_unchanged(&[one], &applied).is_ok());
        let changed = Migration::new(1, "a", "CREATE TABLE a (y INT)");
        assert!(matches!(
            check_unchanged(&[changed], &applied),
            Err(Error::Migration(text)) if text.contains("1_a")
        ));
    }

    #[test]
    fn errors_from_the_client_are_neither_violations_nor_unavailable() {
        for error in [DbError::Protocol("bad packet".into()), DbError::Utf8] {
            assert_eq!(violation(&error), None);
            assert!(!is_unique_violation(&error));
            assert!(!is_unavailable(&error));
        }
    }

    #[test]
    fn a_lost_connection_or_failed_tls_is_unavailable() {
        let lost = DbError::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ));
        assert!(is_unavailable(&lost));
        assert!(is_unavailable(&DbError::Tls("handshake".to_string())));
        assert!(Error::Query(lost).is_unavailable());
        assert!(Error::Pool(PoolError::Closed).is_unavailable());
        assert!(!Error::Migration("x".to_string()).is_unavailable());
        assert!(!Error::InvalidIdentifier("x".to_string()).is_unavailable());
    }

    #[test]
    fn the_unavailable_numbers_are_ones_that_pass() {
        for number in [3906, 976, 4060, 6005, 40613, 40501, 49918] {
            assert!(UNAVAILABLE.contains(&number), "{number}");
        }
        // A deadlock, a lock timeout, and a broken constraint are this request's.
        for number in [1205, 1222, 2627, 547] {
            assert!(!UNAVAILABLE.contains(&number), "{number}");
        }
    }
}
