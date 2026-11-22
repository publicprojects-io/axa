//! Web sessions for Axum services, over [`axum_session`], with safe defaults.
//!
//! `axum_session` does the work. This crate decides what a service gets if it
//! says nothing, because a session that fails open gives no sign of it:
//!
//! | Setting | `axum_session` alone | [`SessionOptions::production`] |
//! |---|---|---|
//! | Cookie signing | Off until a key is set | Required |
//! | Data in the database | Plain text until a key is set | Encrypted, key required |
//! | `Secure` cookie | Off | On |
//! | `SameSite` | `Lax` | `Lax` |
//! | Session lifetime | 6 hours | 8 hours |
//! | Cookie lifetime | 100 days | 7 days |
//! | Ended a fixed time after sign-in, however busy | No | Yes, 7 days |
//! | Bound to the client's IP | Yes, when signed | No: phones change networks |
//! | Stored for an anonymous visitor | Yes | No, until [`sign_in`] |
//!
//! Unsigned cookies are possible only through [`SessionOptions::development`].
//!
//! ```
//! use axa_web_session::{Session, SessionNullPool, SessionOptions, layer, memory_store};
//! use axum::{Router, routing::get};
//!
//! async fn sign_in(session: Session<SessionNullPool>) -> &'static str {
//!     axa_web_session::sign_in(&session);
//!     session.set("user", "ada");
//!     "signed in"
//! }
//!
//! # async fn run() -> Result<(), axa_web_session::Error> {
//! let options = SessionOptions::development();
//! let store = memory_store(&options).await?;
//! let app: Router = Router::new().route("/", get(sign_in)).layer(layer(store, &options));
//! # Ok(())
//! # }
//! ```
//!
//! # Keys
//!
//! Give a production service two different keys of at least [`MIN_KEY_BYTES`]
//! bytes, from a secret store. They must stay the same across restarts and
//! replicas: a new signing key signs everyone out.
//!
//! # Signing in
//!
//! **Nothing is stored for a visitor until a service says so.** A visitor keeps no
//! cookie, as they are sent only a deletion, and has no stored session, so bots and
//! health probes cost no row, and nobody is tracked before they have agreed to
//! anything. So `session.set` on an
//! anonymous visitor is forgotten at the end of the request.
//!
//! - Call [`sign_in`] when someone signs in. It starts storing the session and
//!   gives it a new identifier, so one planted beforehand is worthless.
//! - Call [`sign_out`] when someone signs out.
//! - A session in use stays alive past its [`SessionOptions::lifetime`], as each
//!   request extends it. [`layer`] ends one at [`SessionOptions::sign_in_lifetime`]
//!   after [`sign_in`], however busy, so a stolen cookie stops working in time. The
//!   next request after that is an anonymous visitor's.
//! - [`Session::set_store`] stores a session for any other reason, such as a
//!   basket. [`SessionOptions::store_every_visitor`] stores them all.
//!
//! # Forms
//!
//! A browser sends the session cookie with a form posted from another site unless
//! `SameSite` stops it, and `Lax` does not cover every case. So put
//! [`csrf_token`] in each form, and check it with [`csrf_verify`] before acting on
//! the post. Asking for a token stores the session, because the token lives there.
//!
//! Serve the application with connection information, as `axa`'s `server` module
//! does: `into_make_service_with_connect_info`.

use std::{
    fmt,
    marker::PhantomData,
    task::{Context, Poll},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum_session::DatabasePool;
pub use axum_session::{
    Key, ReadOnlySession, SameSite, Session, SessionConfig, SessionError, SessionLayer,
    SessionMode, SessionNullPool, SessionStore,
};
#[cfg(feature = "postgres")]
pub use axum_session_sqlx::{SessionPgPool, SessionPgSession, SessionPgSessionStore};
use tower_layer::{Layer, Stack};
use tower_service::Service;
use uuid::Uuid;

/// The fewest bytes a key may have. `axum_session` panics on a shorter one, so
/// this crate refuses it first.
pub const MIN_KEY_BYTES: usize = 64;

/// How long a session lasts without being used, by default.
pub const DEFAULT_LIFETIME: Duration = Duration::from_hours(8);

/// How long a long-term session and its cookie last, by default.
pub const DEFAULT_MAX_LIFETIME: Duration = Duration::from_hours(24 * 7);

/// How long a session lasts after [`sign_in`], however busy, by default.
pub const DEFAULT_SIGN_IN_LIFETIME: Duration = DEFAULT_MAX_LIFETIME;

/// A session failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A key is too short.
    #[error("the session {name} key has {length} bytes and needs at least {MIN_KEY_BYTES}")]
    KeyTooShort {
        /// Which key.
        name: &'static str,
        /// How many bytes it has.
        length: usize,
    },
    /// The signing and database keys are the same.
    #[error("the session signing and database keys must differ")]
    SameKey,
    /// A lifetime that does not fit.
    #[error("the session lifetime is too long")]
    LifetimeTooLong,
    /// The store failed.
    #[error(transparent)]
    Store(#[from] SessionError),
}

/// How a service's sessions behave.
#[derive(Clone)]
pub struct SessionOptions {
    cookie_name: String,
    table_name: String,
    lifetime: Duration,
    max_lifetime: Duration,
    sign_in_lifetime: Duration,
    secure: bool,
    store_every_visitor: bool,
    create_table: bool,
    signing_key: Option<Key>,
    database_key: Option<Key>,
}

impl fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Whether the keys are there, and never what they are.
        f.debug_struct("SessionOptions")
            .field("cookie_name", &self.cookie_name)
            .field("table_name", &self.table_name)
            .field("lifetime", &self.lifetime)
            .field("max_lifetime", &self.max_lifetime)
            .field("sign_in_lifetime", &self.sign_in_lifetime)
            .field("secure", &self.secure)
            .field("store_every_visitor", &self.store_every_visitor)
            .field("create_table", &self.create_table)
            .field("signed", &self.signing_key.is_some())
            .field("encrypted", &self.database_key.is_some())
            .finish()
    }
}

fn key(name: &'static str, bytes: &[u8]) -> Result<Key, Error> {
    if bytes.len() < MIN_KEY_BYTES {
        return Err(Error::KeyTooShort {
            name,
            length: bytes.len(),
        });
    }
    Ok(Key::from(bytes))
}

impl SessionOptions {
    /// Signed cookies over HTTPS, with session data encrypted in the database.
    ///
    /// # Errors
    ///
    /// Fails when a key is under [`MIN_KEY_BYTES`], or the two are the same.
    pub fn production(signing_key: &[u8], database_key: &[u8]) -> Result<Self, Error> {
        let signing = key("signing", signing_key)?;
        let database = key("database", database_key)?;
        if signing_key == database_key {
            return Err(Error::SameKey);
        }
        Ok(Self {
            secure: true,
            signing_key: Some(signing),
            database_key: Some(database),
            ..Self::development()
        })
    }

    /// Unsigned cookies over plain HTTP, for a developer's machine and for tests.
    /// Anyone can forge these, so it does not belong in production.
    #[must_use]
    pub fn development() -> Self {
        Self {
            cookie_name: "session".to_string(),
            table_name: "sessions".to_string(),
            lifetime: DEFAULT_LIFETIME,
            max_lifetime: DEFAULT_MAX_LIFETIME,
            sign_in_lifetime: DEFAULT_SIGN_IN_LIFETIME,
            secure: false,
            store_every_visitor: false,
            create_table: true,
            signing_key: None,
            database_key: None,
        }
    }

    /// Name the cookie.
    #[must_use]
    pub fn cookie_name(mut self, name: impl Into<String>) -> Self {
        self.cookie_name = name.into();
        self
    }

    /// Name the Postgres table.
    #[must_use]
    pub fn table_name(mut self, name: impl Into<String>) -> Self {
        self.table_name = name.into();
        self
    }

    /// How long a session lasts without being used.
    #[must_use]
    pub fn lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// How long a long-term session and its cookie last.
    #[must_use]
    pub fn max_lifetime(mut self, max_lifetime: Duration) -> Self {
        self.max_lifetime = max_lifetime;
        self
    }

    /// How long a session lasts after [`sign_in`], however busy. [`layer`] ends it
    /// then, and the visitor signs in again.
    #[must_use]
    pub fn sign_in_lifetime(mut self, sign_in_lifetime: Duration) -> Self {
        self.sign_in_lifetime = sign_in_lifetime;
        self
    }

    /// Store a session for every visitor, including one who has agreed to
    /// nothing. Each new visitor then costs memory, and a database row, for as
    /// long as the session lasts, so mount the layer only on routes that need it.
    #[must_use]
    pub fn store_every_visitor(mut self) -> Self {
        self.store_every_visitor = true;
        self
    }

    /// Whether starting a store creates its table, if it is not there. It does
    /// unless this says otherwise. Say `false` when the database role may not create
    /// tables, or when every change to the schema goes through the service's own
    /// migrations: make the table there, as [`postgres_table_sql`] gives it.
    #[must_use]
    pub fn create_table(mut self, create: bool) -> Self {
        self.create_table = create;
        self
    }

    /// The `axum_session` settings these options describe.
    ///
    /// # Errors
    ///
    /// Fails when a lifetime does not fit `axum_session`'s duration type.
    pub fn config(&self) -> Result<SessionConfig, Error> {
        let duration =
            |value: Duration| time::Duration::try_from(value).map_err(|_| Error::LifetimeTooLong);
        let max_lifetime = duration(self.max_lifetime)?;
        let mut config = SessionConfig::default()
            .with_session_name(self.cookie_name.clone())
            .with_table_name(self.table_name.clone())
            // `axum_session` 0.23 no longer makes the table unless asked, and this
            // crate's stores do unless a service says otherwise.
            .with_database_initiation(self.create_table)
            .with_lifetime(duration(self.lifetime)?)
            .with_max_lifetime(max_lifetime)
            .with_max_age(Some(max_lifetime))
            .with_secure(self.secure)
            .with_http_only(true)
            .with_cookie_same_site(SameSite::Lax)
            .with_hashed_ip(false);
        if !self.store_every_visitor {
            config = config.with_mode(SessionMode::OptIn);
        }
        if let Some(key) = &self.signing_key {
            config = config.with_key(key.clone());
        }
        if let Some(key) = &self.database_key {
            config = config.with_database_key(key.clone());
        }
        Ok(config)
    }
}

/// Where a form's token lives in the session.
const CSRF_KEY: &str = "axa.csrf";

/// Where the time of sign-in lives in the session, in milliseconds since 1970.
const SIGNED_IN_KEY: &str = "axa.signed_in_at";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Start storing `session`, and give it a new identifier, so one planted before
/// sign-in is worthless. Any form token from before is dropped too.
pub fn sign_in<T>(session: &Session<T>)
where
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    session.set_store(true);
    session.remove(CSRF_KEY);
    session.set(SIGNED_IN_KEY, now_ms());
    session.renew();
}

/// End `session`, here and in the store.
pub fn sign_out<T>(session: &Session<T>)
where
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    session.destroy();
}

/// The session's form token, made if it has none. This stores the session.
#[must_use]
pub fn csrf_token<T>(session: &Session<T>) -> String
where
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    if let Some(token) = session.get::<String>(CSRF_KEY) {
        return token;
    }
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    session.set_store(true);
    session.set(CSRF_KEY, &token);
    token
}

/// Whether `submitted` is the session's form token. Refuses a session with none.
#[must_use]
pub fn csrf_verify<T>(session: &Session<T>, submitted: &str) -> bool
where
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    session
        .get::<String>(CSRF_KEY)
        .is_some_and(|token| same_text(&token, submitted))
}

/// Compare without stopping at the first difference, so timing shows nothing.
fn same_text(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0_u8, |seen, (x, y)| seen | (x ^ y))
            == 0
}

/// A store over `client`.
///
/// # Errors
///
/// Fails when the options do not fit, or the store cannot start.
pub async fn store<T>(client: Option<T>, options: &SessionOptions) -> Result<SessionStore<T>, Error>
where
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    Ok(SessionStore::new(client, options.config()?).await?)
}

/// A store held in this process. Replicas do not share it, and a restart loses it.
///
/// # Errors
///
/// Fails when the options do not fit.
pub async fn memory_store(
    options: &SessionOptions,
) -> Result<SessionStore<SessionNullPool>, Error> {
    store(None, options).await
}

/// A store in Postgres, creating its table if needed, unless
/// [`SessionOptions::create_table`] says not to.
///
/// # Errors
///
/// Fails when the options do not fit, or the table cannot be created.
#[cfg(feature = "postgres")]
pub async fn postgres_store(
    pool: sqlx::PgPool,
    options: &SessionOptions,
) -> Result<SessionPgSessionStore, Error> {
    store(Some(pool.into()), options).await
}

/// The statement that makes the Postgres table named `table_name`, as the store
/// makes it, for a service's own migration when [`SessionOptions::create_table`] is
/// `false`. It is the table of `axum_session_sqlx` 0.13. The name is written as
/// given, unquoted, so it must be a plain identifier the service chose.
#[cfg(feature = "postgres")]
#[must_use]
pub fn postgres_table_sql(table_name: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {table_name} (\n    \"id\" VARCHAR(128) NOT NULL PRIMARY KEY,\n    \"expires\" BIGINT NULL,\n    \"session\" TEXT NOT NULL\n)"
    )
}

/// The layer that gives handlers a [`Session`], ending each at the options'
/// [`SessionOptions::sign_in_lifetime`] after [`sign_in`].
#[must_use]
pub fn layer<T>(
    store: SessionStore<T>,
    options: &SessionOptions,
) -> Stack<SignInDeadline<T>, SessionLayer<T>>
where
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    let deadline = SignInDeadline {
        lifetime_ms: u64::try_from(options.sign_in_lifetime.as_millis()).unwrap_or(u64::MAX),
        pool: PhantomData,
    };
    Stack::new(deadline, SessionLayer::new(store))
}

/// The part of [`layer`] that ends a session at its time after sign-in. It runs
/// inside the session layer, before the handler.
pub struct SignInDeadline<T> {
    lifetime_ms: u64,
    pool: PhantomData<fn() -> T>,
}

impl<T> Clone for SignInDeadline<T> {
    fn clone(&self) -> Self {
        Self {
            lifetime_ms: self.lifetime_ms,
            pool: PhantomData,
        }
    }
}

impl<T> fmt::Debug for SignInDeadline<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignInDeadline")
            .field("lifetime_ms", &self.lifetime_ms)
            .finish()
    }
}

impl<S, T> Layer<S> for SignInDeadline<T> {
    type Service = SignInDeadlineService<S, T>;

    fn layer(&self, inner: S) -> Self::Service {
        SignInDeadlineService {
            inner,
            deadline: self.clone(),
        }
    }
}

/// The service [`SignInDeadline`] makes.
pub struct SignInDeadlineService<S, T> {
    inner: S,
    deadline: SignInDeadline<T>,
}

impl<S: Clone, T> Clone for SignInDeadlineService<S, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            deadline: self.deadline.clone(),
        }
    }
}

impl<S, T> fmt::Debug for SignInDeadlineService<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignInDeadlineService")
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl<S, T, B> Service<http::Request<B>> for SignInDeadlineService<S, T>
where
    S: Service<http::Request<B>>,
    T: DatabasePool + Clone + fmt::Debug + Sync + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        if let Some(session) = request.extensions().get::<Session<T>>()
            && let Some(at) = session.get::<u64>(SIGNED_IN_KEY)
            // A session signed in at a later time than now has no age, so it is invalid.
            && now_ms()
                .checked_sub(at)
                .is_none_or(|age| age >= self.deadline.lifetime_ms)
        {
            session.clear();
            session.destroy();
        }
        self.inner.call(request)
    }
}

#[cfg(test)]
mod tests {
    use super::same_text;

    #[test]
    fn only_the_same_text_is_the_same() {
        assert!(same_text("abc", "abc"));
        assert!(!same_text("abc", "abd"));
        assert!(!same_text("abc", "ab"));
        assert!(!same_text("", "a"));
    }

    #[test]
    fn the_deadline_and_its_service_show_the_lifetime_and_nothing_else() {
        use tower::Layer as _;
        let deadline = super::SignInDeadline::<super::SessionNullPool> {
            lifetime_ms: 90_000,
            pool: std::marker::PhantomData,
        };
        assert_eq!(
            format!("{deadline:?}"),
            "SignInDeadline { lifetime_ms: 90000 }"
        );
        let service = deadline.layer("the inner service");
        assert_eq!(
            format!("{:?}", service.clone()),
            "SignInDeadlineService { deadline: SignInDeadline { lifetime_ms: 90000 }, .. }"
        );
    }
}
