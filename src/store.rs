//! A small type-keyed store for values shared across handlers.
//!
//! ```
//! use axa::store::Store;
//!
//! let store = Store::default();
//! store.insert(42_u32);
//! assert_eq!(store.get::<u32>(), Some(42));
//! ```

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::{Arc, RwLock},
};

use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};

use crate::{AppContext, Error};

/// Values keyed by type. Holds one value per type.
#[derive(Default)]
pub struct Store {
    values: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}

impl Store {
    /// Insert a value, replacing any earlier value of the same type.
    ///
    /// # Panics
    ///
    /// If a previous holder of the lock panicked.
    pub fn insert<T: Send + Sync + 'static>(&self, value: T) {
        self.values
            .write()
            .unwrap()
            .insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// Get a shared handle to the value of type `T`.
    ///
    /// # Panics
    ///
    /// If a previous holder of the lock panicked.
    #[must_use]
    pub fn get_ref<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let value = self.values.read().unwrap().get(&TypeId::of::<T>())?.clone();
        value.downcast::<T>().ok()
    }

    /// Get a clone of the value of type `T`.
    #[must_use]
    pub fn get<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.get_ref::<T>().map(|value| (*value).clone())
    }

    /// Whether a value of type `T` is present.
    ///
    /// # Panics
    ///
    /// If a previous holder of the lock panicked.
    #[must_use]
    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.values.read().unwrap().contains_key(&TypeId::of::<T>())
    }
}

/// Extractor that clones a value out of the context's [`Store`].
#[derive(Debug, Clone)]
pub struct SharedStore<T>(pub T);

impl<S, T> FromRequestParts<S> for SharedStore<T>
where
    S: Send + Sync,
    AppContext: FromRef<S>,
    T: Clone + Send + Sync + 'static,
{
    type Rejection = Error;

    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn from_request_parts(_parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        AppContext::from_ref(state)
            .shared_store
            .get::<T>()
            .map(Self)
            .ok_or(Error::InternalServerError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_one_value_per_type() {
        let store = Store::default();
        store.insert(1_u32);
        store.insert(2_u32);
        store.insert("text".to_string());
        assert_eq!(store.get::<u32>(), Some(2));
        assert_eq!(store.get::<String>().as_deref(), Some("text"));
        assert!(!store.contains::<u8>());
    }

    // With a database in the context, a test cannot build one.
    #[cfg(axa_db = "none")]
    #[tokio::test]
    async fn a_missing_shared_value_is_the_generic_500() {
        use axum::{extract::FromRequestParts, http::Request, response::IntoResponse};

        use crate::{
            Environment,
            config::{Config, Defaults},
        };

        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        let ctx = AppContext::for_test(Environment::Test, config);
        let (mut parts, ()) = Request::new(()).into_parts();
        let Err(error) = SharedStore::<u8>::from_request_parts(&mut parts, &ctx).await else {
            panic!("nothing was stored");
        };
        let response = error.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            bytes,
            r#"{"type":"about:blank","title":"Internal Server Error","status":500,"code":"internal_server_error"}"#
        );
    }
}
