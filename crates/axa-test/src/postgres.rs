//! A Postgres database of its own for each test, cloned from a template that is
//! migrated once.
//!
//! Migrating a fresh database for every test is slow, and sharing one database makes
//! tests wait for each other or see each other's rows. [`TestDatabase::new`] migrates a
//! template database once, named for the migrations it holds, and then clones it with
//! `CREATE DATABASE ... TEMPLATE`, which copies files rather than running SQL.
//!
//! [`TestDatabase::close`] drops the clone and waits. A guard dropped without it, as
//! when a test panics, hands its clone to a thread of its own that drops it, rather
//! than blocking the test's runtime: `DROP DATABASE` waits on every connection being
//! opened, and a blocked runtime would never finish opening its own. A clone left by a
//! process that ended first is swept by the next [`TestDatabase::new`], once no session
//! has used it for two minutes.
//!
//! ```ignore
//! use axa_test::postgres::TestDatabase;
//!
//! static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();
//!
//! #[tokio::test]
//! #[ignore = "needs AXA_TEST_DB_URL"]
//! async fn a_todo_is_saved() {
//!     let db = TestDatabase::new(&MIGRATOR).await.unwrap();
//!     sqlx::query("INSERT INTO todos (title) VALUES ('Milk')")
//!         .execute(&db.pool)
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! `AXA_TEST_DB_URL` names a server and a role that may create databases. Each
//! template is kept for the next run, as `axa_test_template_` and a hash of the
//! migrations, so a changed migration gets a template of its own. Drop old ones with
//! [`drop_templates`].

use std::{
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{Sender, channel},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axa_postgres_sqlx::{
    quote_identifier,
    sqlx::{
        self, AssertSqlSafe, Connection, PgConnection, PgPool, Row,
        migrate::Migrator,
        postgres::{PgConnectOptions, PgPoolOptions},
    },
};

/// The variable that names the server tests may create databases on.
pub const URL_VAR: &str = "AXA_TEST_DB_URL";

/// What every template's name starts with.
const TEMPLATE_PREFIX: &str = "axa_test_template_";

/// What every clone's name starts with.
const CLONE_PREFIX: &str = "axa_test_";

/// The advisory lock a process holds while it builds a template, so that two test
/// binaries starting together build it once.
const TEMPLATE_LOCK: i64 = 0x0061_7861_7465_7374;

/// How many clones this process has made, to keep their names apart.
static CLONES: AtomicU64 = AtomicU64::new(0);

/// How old a clone no session is using must be before another process sweeps it.
const ABANDONED_SECONDS: u64 = 120;

/// Seconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A database of a test's own, dropped when this is.
pub struct TestDatabase {
    /// A pool for the database.
    pub pool: PgPool,
    /// How to connect to the database, as for a service's own pool.
    pub options: PgConnectOptions,
    name: String,
    server: PgConnectOptions,
}

impl std::fmt::Debug for TestDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestDatabase")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// The template's name for `migrator`: a hash of each migration's version and
/// checksum, so that a changed migration names another template. It is FNV-1a, stable
/// across builds and Rust versions, so a template outlives the build that made it.
#[must_use]
pub fn template_name(migrator: &Migrator) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for migration in migrator.iter() {
        let version = migration.version.to_le_bytes();
        for byte in version.iter().chain(migration.checksum.iter()) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{TEMPLATE_PREFIX}{hash:016x}")
}

/// A name no other clone has, in this process or another, that starts with when it
/// was made, for the sweep.
fn clone_name() -> String {
    format!(
        "{CLONE_PREFIX}{}_{}_{}",
        now(),
        std::process::id(),
        CLONES.fetch_add(1, Ordering::Relaxed)
    )
}

/// When the clone `name` was made, in seconds since the Unix epoch, or `None` for a
/// name that is not a clone's.
fn made_at(name: &str) -> Option<u64> {
    let rest = name.strip_prefix(CLONE_PREFIX)?;
    let mut parts = rest.split('_');
    let made = parts.next()?.parse().ok()?;
    (parts.count() == 2).then_some(made)
}

/// Drop every clone made over two minutes ago that no session is using. A clone in
/// use always has one, as its pool keeps a connection open.
async fn sweep(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    let names: Vec<String> = sqlx::query(
        "SELECT datname FROM pg_database AS d WHERE datname LIKE 'axa\\_test\\_%' \
         AND NOT EXISTS (SELECT FROM pg_stat_activity AS a WHERE a.datname = d.datname)",
    )
    .fetch_all(&mut *connection)
    .await?
    .iter()
    .map(|row| row.try_get("datname"))
    .collect::<Result<_, _>>()?;
    let cutoff = now().saturating_sub(ABANDONED_SECONDS);
    for name in names {
        if made_at(&name).is_some_and(|made| made < cutoff) {
            execute(
                connection,
                format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quoted(&name)?),
            )
            .await?;
        }
    }
    Ok(())
}

/// A clone for the cleaner to drop, and where to say it is gone.
type Job = (PgConnectOptions, String, Sender<()>);

/// How long a dropped guard waits for its clone to go. Its test's runtime is blocked
/// meanwhile, and a connection that runtime was opening holds up `DROP DATABASE`, so
/// the wait is short: past it, the clone still goes once the runtime moves on.
const DROP_WAIT: Duration = Duration::from_secs(2);

/// The thread that drops the clones of guards dropped without [`TestDatabase::close`].
fn cleaner() -> &'static Mutex<Sender<Job>> {
    static CLEANER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    CLEANER.get_or_init(|| {
        let (sender, receiver) = channel::<Job>();
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("axa-test: cannot drop test databases: {error}");
                    return;
                }
            };
            for (server, name, done) in receiver {
                runtime.block_on(drop_database(&server, &name));
                let _ = done.send(());
            }
        });
        Mutex::new(sender)
    })
}

/// `name`, quoted, which every name here is safe to be.
fn quoted(name: &str) -> Result<String, sqlx::Error> {
    quote_identifier(name)
        .ok_or_else(|| sqlx::Error::Configuration(format!("{name} is not a safe name").into()))
}

async fn execute(connection: &mut PgConnection, sql: String) -> Result<(), sqlx::Error> {
    sqlx::query(AssertSqlSafe(sql))
        .execute(&mut *connection)
        .await
        .map(|_| ())
}

async fn database_exists(connection: &mut PgConnection, name: &str) -> Result<bool, sqlx::Error> {
    sqlx::query("SELECT EXISTS (SELECT FROM pg_database WHERE datname = $1) AS found")
        .bind(name)
        .fetch_one(&mut *connection)
        .await?
        .try_get("found")
}

/// Make the template for `migrator`, unless it is there. It is built under another
/// name and renamed when its migrations have run, so a run that dies halfway leaves no
/// template that looks finished.
async fn ensure_template(
    connection: &mut PgConnection,
    server: &PgConnectOptions,
    migrator: &Migrator,
    template: &str,
) -> Result<(), sqlx::Error> {
    if database_exists(connection, template).await? {
        return Ok(());
    }
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(TEMPLATE_LOCK)
        .execute(&mut *connection)
        .await?;
    let built = build_template(connection, server, migrator, template).await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(TEMPLATE_LOCK)
        .execute(&mut *connection)
        .await?;
    built
}

async fn build_template(
    connection: &mut PgConnection,
    server: &PgConnectOptions,
    migrator: &Migrator,
    template: &str,
) -> Result<(), sqlx::Error> {
    // Another process may have built it while this one waited for the lock.
    if database_exists(connection, template).await? {
        return Ok(());
    }
    let partial = format!("{template}_partial");
    execute(
        connection,
        format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quoted(&partial)?),
    )
    .await?;
    execute(connection, format!("CREATE DATABASE {}", quoted(&partial)?)).await?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(server.clone().database(&partial))
        .await?;
    let migrated = axa_postgres_sqlx::migrate(&pool, migrator).await;
    pool.close().await;
    if let Err(error) = migrated {
        drop_database(server, &partial).await;
        return Err(error.into());
    }
    execute(
        connection,
        format!(
            "ALTER DATABASE {} RENAME TO {}",
            quoted(&partial)?,
            quoted(template)?
        ),
    )
    .await
}

impl TestDatabase {
    /// A database migrated by `migrator`, on the server `AXA_TEST_DB_URL` names.
    ///
    /// # Errors
    ///
    /// Fails when `AXA_TEST_DB_URL` is unset or is not a Postgres URL, when the server
    /// cannot be reached or the role may not create databases, or when a migration
    /// fails.
    pub async fn new(migrator: &Migrator) -> Result<Self, sqlx::Error> {
        let url = std::env::var(URL_VAR)
            .map_err(|_| sqlx::Error::Configuration(format!("{URL_VAR} is not set").into()))?;
        Self::on(url.parse()?, migrator).await
    }

    /// A database migrated by `migrator`, on the server `server` reaches. The
    /// database `server` names is used only to create and drop the others.
    ///
    /// # Errors
    ///
    /// As [`TestDatabase::new`].
    pub async fn on(server: PgConnectOptions, migrator: &Migrator) -> Result<Self, sqlx::Error> {
        let template = template_name(migrator);
        let name = clone_name();
        let mut connection = PgConnection::connect_with(&server).await?;
        sweep(&mut connection).await?;
        ensure_template(&mut connection, &server, migrator, &template).await?;
        execute(
            &mut connection,
            format!(
                "CREATE DATABASE {} TEMPLATE {}",
                quoted(&name)?,
                quoted(&template)?
            ),
        )
        .await?;
        connection.close().await?;
        let options = server.clone().database(&name);
        // The connection kept open is what tells the sweep this clone is in use.
        let pool = match PgPoolOptions::new()
            .min_connections(1)
            .max_connections(5)
            .idle_timeout(None)
            .connect_with(options.clone())
            .await
        {
            Ok(pool) => pool,
            Err(error) => {
                drop_database(&server, &name).await;
                return Err(error);
            }
        };
        Ok(Self {
            pool,
            options,
            name,
            server,
        })
    }

    /// The database's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Close the pool and drop the database, and wait for both.
    pub async fn close(mut self) {
        let name = std::mem::take(&mut self.name);
        self.pool.close().await;
        drop_database(&self.server, &name).await;
    }
}

/// Drop the database `name`, ending its sessions. A failure is printed, as there is
/// nothing a test could do about it.
async fn drop_database(server: &PgConnectOptions, name: &str) {
    let dropped = async {
        let mut connection = PgConnection::connect_with(server).await?;
        execute(
            &mut connection,
            format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quoted(name)?),
        )
        .await?;
        connection.close().await
    };
    if let Err(error) = dropped.await {
        eprintln!("axa-test: could not drop the test database {name}: {error}");
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if self.name.is_empty() {
            return;
        }
        // Calling it marks the pool closed at once, so it opens no more connections,
        // which `DROP DATABASE` would wait on. Its sessions end with `WITH (FORCE)`.
        drop(self.pool.close());
        let (done, gone) = channel();
        let job = (self.server.clone(), std::mem::take(&mut self.name), done);
        let sent = cleaner()
            .lock()
            .map_err(|_| ())
            .and_then(|sender| sender.send(job).map_err(|_| ()));
        if sent.is_err() {
            eprintln!("axa-test: a test database was left for the next run to sweep");
            return;
        }
        // Mostly this is quick, and so the clone is gone before a test binary exits.
        let _ = gone.recv_timeout(DROP_WAIT);
    }
}

/// Drop every template on the server `server` reaches but `keep`, as after a
/// migration changed. It answers how many it dropped.
///
/// # Errors
///
/// Fails when the server cannot be reached, or a template cannot be dropped.
pub async fn drop_templates(
    server: &PgConnectOptions,
    keep: &Migrator,
) -> Result<usize, sqlx::Error> {
    let keep = template_name(keep);
    let mut connection = PgConnection::connect_with(server).await?;
    let names: Vec<String> = sqlx::query("SELECT datname FROM pg_database WHERE datname LIKE $1")
        .bind(format!("{TEMPLATE_PREFIX}%"))
        .fetch_all(&mut connection)
        .await?
        .iter()
        .map(|row| row.try_get("datname"))
        .collect::<Result<_, _>>()?;
    let mut dropped = 0;
    for name in names.iter().filter(|name| **name != keep) {
        execute(
            &mut connection,
            format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quoted(name)?),
        )
        .await?;
        dropped += 1;
    }
    connection.close().await?;
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_name_is_safe_and_new_each_time() {
        let (first, second) = (clone_name(), clone_name());
        assert_ne!(first, second);
        for name in [&first, &second] {
            assert!(quote_identifier(name).is_some(), "{name}");
            assert!(name.len() <= 63, "{name}");
        }
    }

    #[test]
    fn a_clone_name_says_when_it_was_made() {
        let name = clone_name();
        let made = made_at(&name).unwrap();
        assert!(now().abs_diff(made) < 5, "{name}");
        assert_eq!(made_at("axa_test_template_cbf29ce484222325"), None);
        assert_eq!(made_at("axa_test_template_cbf29ce484222325_partial"), None);
        assert_eq!(made_at("someone_elses"), None);
    }

    #[test]
    fn a_template_name_follows_the_migrations() {
        let empty = Migrator::DEFAULT;
        let name = template_name(&empty);
        assert_eq!(name, format!("{TEMPLATE_PREFIX}cbf29ce484222325"));
        assert!(quote_identifier(&name).is_some());
        assert!(format!("{name}_partial").len() <= 63);
    }
}
