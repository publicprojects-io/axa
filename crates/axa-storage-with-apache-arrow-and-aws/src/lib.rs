//! Object storage for Axum services, over Apache Arrow's [`object_store`]: local disk,
//! memory, or S3, chosen by a URL, with keys that cannot climb out of the store and
//! presigned URLs on S3.
//!
//! ```no_run
//! use axa_storage_with_apache_arrow_and_aws::{Key, Storage};
//!
//! # async fn example(url: &str) -> Result<(), axa_storage_with_apache_arrow_and_aws::Error> {
//! let storage = Storage::from_url(url, true)?;
//! let key = Key::new("avatars/42.png")?;
//! storage.put(&key, bytes::Bytes::from_static(b"...")).await?;
//! let back = storage.get_limited(&key, 1_000_000).await?;
//! let link = storage.presigned_get(&key, std::time::Duration::from_mins(5)).await?;
//! # let _ = (back, link);
//! # Ok(())
//! # }
//! ```
//!
//! # Stores
//!
//! | URL | Store |
//! |---|---|
//! | `s3://bucket` or `s3://bucket/prefix` | S3, configured from the `AWS_` variables `object_store` reads |
//! | `file:///var/lib/service/files` | A directory on local disk, which must exist |
//! | `memory:` | In memory, for tests |
//!
//! S3 credentials, region, and endpoint come from the environment, as
//! [`AmazonS3Builder::from_env`] reads them: `AWS_ACCESS_KEY_ID`,
//! `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `AWS_ENDPOINT` for an S3-compatible store, and
//! the rest. In production, [`Storage::from_url`] refuses the memory store, from which
//! every file is gone at restart, and an S3 store reached over plain `http`.
//!
//! # TLS
//!
//! S3 is reached over rustls with ring, as the other axa crates reach their servers.
//! `reqwest` then needs rustls's process-wide provider installed before it builds a
//! client: [`Storage::from_url`] installs ring's when none is, and a service that builds
//! its own [`AmazonS3`] for [`Storage::s3`] calls [`install_ring_provider`] first. A
//! provider the service installed itself is kept.
//!
//! # Keys
//!
//! A [`Key`] is the only way to name an object, and it refuses what could reach outside
//! the store or mean two things: an empty key or segment, a `.` or `..` segment, a
//! leading or trailing `/`, a `\`, a control character, and more than
//! [`MAX_KEY_BYTES`]. Build a key from ids the service made, such as
//! `format!("avatars/{user_id}.png")`, never from a name someone sent: a file name to
//! show goes in the service's own table, beside the key.
//!
//! # Presigned URLs
//!
//! [`Storage::presigned_get`] and [`Storage::presigned_put`] give a URL that lets
//! whoever holds it read or write one object, for a time, without the service's
//! credentials, so a browser can download or upload straight to S3. Anyone holding the
//! URL can use it until it expires, so keep the time short, hand it only to the person
//! the object is for, and never log it. The local and memory stores cannot sign, and
//! answer [`Error::NotSupported`].

use std::{fmt, sync::Arc, time::Duration};

use bytes::Bytes;
pub use object_store;
use object_store::{
    ClientConfigKey, ObjectStore, ObjectStoreExt, PutPayload,
    aws::{AmazonS3, AmazonS3Builder, AmazonS3ConfigKey},
    local::LocalFileSystem,
    memory::InMemory,
    path::Path,
    prefix::PrefixStore,
    signer::{Method, Signer},
};

/// The longest key, in bytes, which S3 also allows.
pub const MAX_KEY_BYTES: usize = 1024;

/// Why storage could not be set up, or an object read or written.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The URL names no store this crate has, or does not parse.
    Url,
    /// Production refuses this store: memory, or S3 over plain `http`.
    Refused(&'static str),
    /// The key could reach outside the store or mean two things: see [`Key`].
    BadKey,
    /// The object is larger than the limit it was read with.
    TooLarge {
        /// Its size, in bytes.
        size: u64,
    },
    /// This store cannot sign URLs.
    NotSupported,
    /// The store refused, or could not be reached.
    Store(object_store::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url => {
                f.write_str("the storage URL must be s3://bucket, file:///directory, or memory:")
            }
            Self::Refused(why) => write!(f, "production refuses this store: {why}"),
            Self::BadKey => write!(
                f,
                "a key must be 1 to {MAX_KEY_BYTES} bytes of segments joined by `/`, none \
                 empty, `.`, or `..`, with no `\\` or control character"
            ),
            Self::TooLarge { size } => write!(f, "the object is {size} bytes, over the limit"),
            Self::NotSupported => f.write_str("this store cannot sign URLs"),
            Self::Store(error) => write!(f, "the store: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<object_store::Error> for Error {
    fn from(error: object_store::Error) -> Self {
        Self::Store(error)
    }
}

/// The name of one object: see [Keys](crate#keys).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key(String);

impl Key {
    /// A key, if `key` is one: segments joined by `/`, none empty, `.`, or `..`, with no
    /// leading or trailing `/`, no `\`, no control character, and at most
    /// [`MAX_KEY_BYTES`] bytes.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::BadKey`] for anything else.
    pub fn new(key: impl Into<String>) -> Result<Self, Error> {
        let key = key.into();
        let good = !key.is_empty()
            && key.len() <= MAX_KEY_BYTES
            && !key.contains('\\')
            && !key.chars().any(char::is_control)
            && key
                .split('/')
                .all(|segment| !matches!(segment, "" | "." | ".."))
            && Path::parse(&key).is_ok_and(|path| path.as_ref() == key);
        if good {
            Ok(Self(key))
        } else {
            Err(Error::BadKey)
        }
    }

    /// The key as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn path(&self) -> Path {
        Path::from(self.0.as_str())
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What is known of one object without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Meta {
    /// Its size, in bytes.
    pub size: u64,
    /// When it was last written.
    pub modified: std::time::SystemTime,
}

/// Install ring as rustls's process-wide crypto provider, unless one is installed, so
/// that the S3 client can be built: see [TLS](crate#tls). Safe to call more than once.
pub fn install_ring_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Another thread may install one first, which is as good.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// Refuse an S3 store reached over plain `http`, by its endpoint or `allow_http`.
fn refuse_plain_http(builder: &AmazonS3Builder) -> Result<(), Error> {
    let endpoint = builder
        .get_config_value(&AmazonS3ConfigKey::Endpoint)
        .unwrap_or_default();
    let allow_http = builder
        .get_config_value(&AmazonS3ConfigKey::Client(ClientConfigKey::AllowHttp))
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    if allow_http || endpoint.to_ascii_lowercase().starts_with("http://") {
        Err(Error::Refused("S3 must be reached over https"))
    } else {
        Ok(())
    }
}

/// An S3 store and the prefix this storage keeps its objects under, for signing.
#[derive(Clone)]
struct S3Signer {
    s3: Arc<AmazonS3>,
    prefix: Path,
}

/// Reads and writes objects in one store. Cheap to clone.
#[derive(Clone)]
pub struct Storage {
    store: Arc<dyn ObjectStore>,
    signer: Option<S3Signer>,
    kind: &'static str,
}

impl fmt::Debug for Storage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Storage")
            .field("store", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Storage {
    /// The store a URL names: see [Stores](crate#stores). With `production` set, the
    /// memory store and S3 over plain `http` are refused.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Url`] for a URL this crate cannot read, with
    /// [`Error::Refused`] for a store production refuses, and with [`Error::Store`] when
    /// the store cannot be built, such as a missing directory.
    pub fn from_url(url: &str, production: bool) -> Result<Self, Error> {
        if url == "memory:" {
            if production {
                return Err(Error::Refused(
                    "the memory store loses every file at restart",
                ));
            }
            return Ok(Self::memory());
        }
        if let Some(dir) = url.strip_prefix("file://") {
            if !dir.starts_with('/') {
                return Err(Error::Url);
            }
            return Self::local(dir);
        }
        let rest = url.strip_prefix("s3://").ok_or(Error::Url)?;
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty() || bucket.contains(['?', '#', '@', ':']) {
            return Err(Error::Url);
        }
        let prefix = prefix.trim_end_matches('/');
        let prefix = if prefix.is_empty() {
            None
        } else {
            Some(Key::new(prefix).map_err(|_| Error::Url)?)
        };
        let builder = AmazonS3Builder::from_env().with_bucket_name(bucket);
        if production {
            refuse_plain_http(&builder)?;
        }
        install_ring_provider();
        Ok(Self::s3(builder.build()?, prefix.as_ref()))
    }

    /// A store in memory, for tests. Production refuses it from a URL.
    #[must_use]
    pub fn memory() -> Self {
        Self {
            store: Arc::new(InMemory::new()),
            signer: None,
            kind: "memory",
        }
    }

    /// A store in the directory `dir` on local disk, which must exist.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Store`] when the directory cannot be used.
    pub fn local(dir: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        Ok(Self {
            store: Arc::new(LocalFileSystem::new_with_prefix(dir)?),
            signer: None,
            kind: "local",
        })
    }

    /// A store on `s3`, built as the service chooses, keeping its objects under `prefix`
    /// when there is one. Call [`install_ring_provider`] before building `s3`, unless
    /// the service installs a rustls provider of its own.
    #[must_use]
    pub fn s3(s3: AmazonS3, prefix: Option<&Key>) -> Self {
        let s3 = Arc::new(s3);
        let prefix = prefix.map_or_else(|| Path::from(""), Key::path);
        let store: Arc<dyn ObjectStore> = if prefix.as_ref().is_empty() {
            s3.clone()
        } else {
            let inner: Arc<dyn ObjectStore> = s3.clone();
            Arc::new(PrefixStore::new(inner, prefix.clone()))
        };
        Self {
            store,
            signer: Some(S3Signer { s3, prefix }),
            kind: "s3",
        }
    }

    /// The `object_store` store underneath, for what this crate does not cover, such as
    /// listing or multipart uploads. Its paths are not checked as a [`Key`] is.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn ObjectStore> {
        &self.store
    }

    /// Write `bytes` to `key`, whole or not at all, replacing what was there.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Store`] when the store refuses or cannot be reached.
    pub async fn put(&self, key: &Key, bytes: Bytes) -> Result<(), Error> {
        self.store.put(&key.path(), PutPayload::from(bytes)).await?;
        Ok(())
    }

    /// Read `key` whole, or `None` when there is no such object, refusing one larger
    /// than `max_bytes` before reading it.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::TooLarge`] for an object over the limit, and with
    /// [`Error::Store`] when the store refuses or cannot be reached.
    pub async fn get_limited(&self, key: &Key, max_bytes: u64) -> Result<Option<Bytes>, Error> {
        let result = match self.store.get(&key.path()).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if result.meta.size > max_bytes {
            return Err(Error::TooLarge {
                size: result.meta.size,
            });
        }
        Ok(Some(result.bytes().await?))
    }

    /// The size and time of `key`, or `None` when there is no such object.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Store`] when the store refuses or cannot be reached.
    pub async fn head(&self, key: &Key) -> Result<Option<Meta>, Error> {
        match self.store.head(&key.path()).await {
            Ok(meta) => Ok(Some(Meta {
                size: meta.size,
                modified: meta.last_modified.into(),
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Delete `key`. Deleting an object that is not there is not an error.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Store`] when the store refuses or cannot be reached.
    pub async fn delete(&self, key: &Key) -> Result<(), Error> {
        match self.store.delete(&key.path()).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// A URL that lets whoever holds it `GET` `key` for `expires_in`, on S3: see
    /// [Presigned URLs](crate#presigned-urls).
    ///
    /// # Errors
    ///
    /// Fails with [`Error::NotSupported`] on a store that cannot sign.
    pub async fn presigned_get(&self, key: &Key, expires_in: Duration) -> Result<String, Error> {
        self.presigned(Method::GET, key, expires_in).await
    }

    /// A URL that lets whoever holds it `PUT` `key` for `expires_in`, on S3, replacing
    /// what is there: see [Presigned URLs](crate#presigned-urls). It does not limit the
    /// size of what is sent.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::NotSupported`] on a store that cannot sign.
    pub async fn presigned_put(&self, key: &Key, expires_in: Duration) -> Result<String, Error> {
        self.presigned(Method::PUT, key, expires_in).await
    }

    async fn presigned(
        &self,
        method: Method,
        key: &Key,
        expires_in: Duration,
    ) -> Result<String, Error> {
        let signer = self.signer.as_ref().ok_or(Error::NotSupported)?;
        let path = if signer.prefix.as_ref().is_empty() {
            key.path()
        } else {
            Path::from(format!("{}/{}", signer.prefix, key.as_str()))
        };
        Ok(signer
            .s3
            .signed_url(method, &path, expires_in)
            .await?
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_cannot_climb_out_or_mean_two_things() {
        for good in [
            "a",
            "avatars/42.png",
            "a/b/c.d",
            "x..y",
            ".hidden",
            &"k".repeat(MAX_KEY_BYTES),
        ] {
            assert_eq!(Key::new(good).unwrap().as_str(), good);
        }
        for bad in [
            "",
            "..",
            ".",
            "../etc/passwd",
            "a/../b",
            "a/./b",
            "a/..",
            "/a",
            "a/",
            "a//b",
            "a\\b",
            "..\\a",
            "a\nb",
            "a\u{0}b",
            &"k".repeat(MAX_KEY_BYTES + 1),
        ] {
            assert!(matches!(Key::new(bad), Err(Error::BadKey)), "{bad:?}");
        }
    }

    #[test]
    fn production_refuses_memory_and_s3_over_plain_http() {
        assert!(matches!(
            Storage::from_url("memory:", true),
            Err(Error::Refused(_))
        ));
        assert!(Storage::from_url("memory:", false).is_ok());
        for url in [
            "",
            "memory",
            "http://bucket",
            "s3://",
            "s3://b?x",
            "file:relative",
            "file://relative",
        ] {
            assert!(
                matches!(Storage::from_url(url, false), Err(Error::Url)),
                "{url}"
            );
        }
        let plain = AmazonS3Builder::new()
            .with_bucket_name("b")
            .with_endpoint("http://minio.internal:9000");
        assert!(matches!(refuse_plain_http(&plain), Err(Error::Refused(_))));
        let allowed = AmazonS3Builder::new()
            .with_bucket_name("b")
            .with_allow_http(true);
        assert!(matches!(
            refuse_plain_http(&allowed),
            Err(Error::Refused(_))
        ));
        let tls = AmazonS3Builder::new()
            .with_bucket_name("b")
            .with_endpoint("https://minio.internal:9000");
        assert!(refuse_plain_http(&tls).is_ok());
    }

    #[test]
    fn an_s3_url_names_its_bucket_and_a_prefix_that_cannot_climb_out() {
        for url in ["s3://files", "s3://files/", "s3://files/tenant-a/avatars/"] {
            let storage = Storage::from_url(url, false).unwrap();
            assert!(format!("{storage:?}").contains("s3"), "{url}");
        }
        let prefixed = Storage::from_url("s3://files/tenant-a/", false).unwrap();
        let signer = prefixed.signer.as_ref().unwrap();
        assert_eq!(signer.prefix.as_ref(), "tenant-a", "trailing `/` dropped");
        for url in ["s3://files/../other", "s3://files/a//b", "s3://files/./a"] {
            assert!(
                matches!(Storage::from_url(url, false), Err(Error::Url)),
                "{url}"
            );
        }
    }

    #[test]
    fn each_error_says_what_went_wrong_and_only_a_store_error_has_a_source() {
        use std::error::Error as _;
        let store = Error::from(object_store::Error::NotFound {
            path: "a".to_string(),
            source: "gone".into(),
        });
        for (error, says) in [
            (Error::Url, "s3://bucket"),
            (Error::Refused("why"), "production refuses this store: why"),
            (Error::BadKey, "1 to 1024 bytes"),
            (Error::TooLarge { size: 12 }, "12 bytes, over the limit"),
            (Error::NotSupported, "cannot sign"),
        ] {
            assert!(error.to_string().contains(says), "{error}");
            assert!(error.source().is_none(), "{error}");
        }
        assert!(store.to_string().starts_with("the store: "), "{store}");
        assert!(store.source().is_some());
        assert_eq!(Key::new("a/b").unwrap().to_string(), "a/b");
    }
}
