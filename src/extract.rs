//! Extractors that answer a bad request as every other error is answered.
//!
//! Axum's own `Json`, `Path`, and `Query` reject a bad request with plain text, and
//! that text can quote what the caller sent. These take their place, and reject with
//! [`Error`], so a caller gets a problem and the text quotes nothing.
//!
//! ```
//! use axa::{Result, extract::{Json, Path}};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize, Serialize)]
//! struct Folder {
//!     name: String,
//! }
//!
//! async fn rename(Path(id): Path<u32>, Json(folder): Json<Folder>) -> Result<Json<Folder>> {
//!     # let _ = id;
//!     Ok(Json(folder))
//! }
//! ```
//!
//! The prelude keeps Axum's, because changing a name it exports would change what
//! existing handlers mean. A service opts in by importing these.
//!
//! [`Patch`] is a field of a `PATCH` body that tells a field left out from one set to
//! `null`, which `Option` cannot:
//!
//! ```
//! use axa::extract::Patch;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Change {
//!     #[serde(default)]
//!     nickname: Patch<String>,
//! }
//!
//! let change: Change = serde_json::from_str(r#"{"nickname": null}"#).unwrap();
//! let mut nickname = Some("Ada".to_string());
//! change.nickname.apply(&mut nickname);
//! assert_eq!(nickname, None);
//!
//! let change: Change = serde_json::from_str("{}").unwrap();
//! assert!(change.nickname.is_missing());
//! ```

use axum::{
    extract::{FromRequest, FromRequestParts},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Error;

/// A JSON request or response body.
#[derive(Debug, Clone, Copy, Default, FromRequest)]
#[from_request(via(axum::Json), rejection(Error))]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Values from the URL's path.
#[derive(Debug, Clone, Copy, Default, FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(Error))]
pub struct Path<T>(pub T);

/// A form body, `application/x-www-form-urlencoded`, or the query string of a `GET`.
#[derive(Debug, Clone, Copy, Default, FromRequest)]
#[from_request(via(axum::Form), rejection(Error))]
pub struct Form<T>(pub T);

/// Values from the URL's query string.
#[derive(Debug, Clone, Copy, Default, FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(Error))]
pub struct Query<T>(pub T);

/// What a page shows of a form sent back to be fixed: the values the person entered,
/// and each field's errors, for a template such as Tera's:
///
/// ```
/// use axa::extract::FormContext;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct SignUp {
///     email: String,
///     password: String,
/// }
///
/// let sent = SignUp { email: "ada@".into(), password: "secret".into() };
/// let form = FormContext::new(&sent)
///     .hide("password")
///     .error("email", "Enter a whole email address");
/// let shown = serde_json::to_value(&form).unwrap();
/// assert_eq!(shown["values"]["email"], "ada@");
/// assert!(shown["values"].get("password").is_none());
/// assert_eq!(shown["errors"]["email"][0], "Enter a whole email address");
/// ```
///
/// A template reads `form.values.email` and `form.errors.email`. Hide every field that
/// holds a secret, so it is never sent back in the page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FormContext {
    values: std::collections::BTreeMap<String, String>,
    errors: std::collections::BTreeMap<String, Vec<String>>,
}

impl FormContext {
    /// The top-level fields of `form` as text, a missing or `null` field left out, and
    /// one that is a list or an object left out too, as a form field holds one value.
    #[must_use]
    pub fn new<T: Serialize>(form: &T) -> Self {
        let values = match serde_json::to_value(form) {
            Ok(serde_json::Value::Object(fields)) => fields
                .into_iter()
                .filter_map(|(name, value)| {
                    let text = match value {
                        serde_json::Value::String(text) => text,
                        serde_json::Value::Number(number) => number.to_string(),
                        serde_json::Value::Bool(flag) => flag.to_string(),
                        _ => return None,
                    };
                    Some((name, text))
                })
                .collect(),
            _ => std::collections::BTreeMap::new(),
        };
        Self {
            values,
            errors: std::collections::BTreeMap::new(),
        }
    }

    /// Leave out `field`'s value, as for a password.
    #[must_use]
    pub fn hide(mut self, field: &str) -> Self {
        self.values.remove(field);
        self
    }

    /// Add `message` to `field`'s errors.
    #[must_use]
    pub fn error(mut self, field: &str, message: impl Into<String>) -> Self {
        self.errors
            .entry(field.to_string())
            .or_default()
            .push(message.into());
        self
    }

    /// Add each failure in `errors`, its message, or its code when it has none.
    #[cfg(feature = "model")]
    #[must_use]
    pub fn validation(mut self, errors: &validator::ValidationErrors) -> Self {
        for (field, failures) in errors.field_errors() {
            for failure in failures {
                let message = failure
                    .message
                    .as_ref()
                    .map_or_else(|| failure.code.to_string(), ToString::to_string);
                self = self.error(&field, message);
            }
        }
        self
    }

    /// Whether no field has an error.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// The value entered in `field`, if it is shown.
    #[must_use]
    pub fn value(&self, field: &str) -> Option<&str> {
        self.values.get(field).map(String::as_str)
    }

    /// The errors of `field`.
    #[must_use]
    pub fn errors(&self, field: &str) -> &[String] {
        self.errors.get(field).map_or(&[], Vec::as_slice)
    }
}

/// A field of a `PATCH` body: left out, set to `null`, or set to a value.
///
/// Mark the field `#[serde(default)]`, so a body without it is [`Patch::Missing`]
/// rather than a `422` for a missing field. When writing one out, mark it
/// `#[serde(skip_serializing_if = "Patch::is_missing")]`, as a missing field has no
/// value to write: on its own it is written as `null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Patch<T> {
    /// The field was left out, so the value is unchanged.
    #[default]
    Missing,
    /// The field was `null`, so the value is cleared.
    Null,
    /// The field was set to this value.
    Value(T),
}

impl<T> Patch<T> {
    /// Whether the field was left out.
    #[must_use]
    pub const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    /// Whether the field was `null`.
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// The value the field was set to, if any.
    #[must_use]
    pub const fn value(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Missing | Self::Null => None,
        }
    }

    /// `None` when the field was left out, `Some(None)` when it was `null`, and
    /// `Some(Some(value))` when it was set.
    #[must_use]
    pub fn into_option(self) -> Option<Option<T>> {
        match self {
            Self::Missing => None,
            Self::Null => Some(None),
            Self::Value(value) => Some(Some(value)),
        }
    }

    /// Change `target` as the field says: left alone when it was left out, cleared
    /// when it was `null`, and replaced when it was set.
    pub fn apply(self, target: &mut Option<T>) {
        if let Some(value) = self.into_option() {
            *target = value;
        }
    }
}

impl<T> From<Option<T>> for Patch<T> {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Self::Value)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(Self::from)
    }
}

impl<T: Serialize> Serialize for Patch<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(serializer)
    }
}

/// The largest integer a JSON client that reads numbers as IEEE 754 doubles, as
/// JavaScript does, holds exactly: 2^53 - 1.
pub const MAX_SAFE_INT: i64 = (1 << 53) - 1;

/// An integer every JSON client reads exactly, such as an amount in minor units: one
/// beyond [`MAX_SAFE_INT`] either way is refused when read, so a body field of this
/// type is a `422` with a pointer to it (ERR-15), and refused by [`SafeInt::new`] when
/// made, so none is sent that a browser would round.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SafeInt(i64);

impl SafeInt {
    /// `value`, or `None` beyond [`MAX_SAFE_INT`] either way.
    #[must_use]
    pub const fn new(value: i64) -> Option<Self> {
        if value.unsigned_abs() <= MAX_SAFE_INT.unsigned_abs() {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The integer.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl From<SafeInt> for i64 {
    fn from(value: SafeInt) -> Self {
        value.0
    }
}

impl TryFrom<i64> for SafeInt {
    type Error = i64;

    fn try_from(value: i64) -> Result<Self, i64> {
        Self::new(value).ok_or(value)
    }
}

impl<'de> Deserialize<'de> for SafeInt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            serde::de::Error::custom("an integer beyond what a JSON client holds exactly")
        })
    }
}

#[allow(clippy::as_conversions, reason = "RCA-SCOPE-4: tests may use `as`")]
#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
        routing::{get, post},
    };
    use serde::Deserialize;
    use tower::ServiceExt;

    use super::*;

    #[derive(Deserialize, Serialize)]
    struct Folder {
        name: String,
    }

    #[derive(Deserialize)]
    struct Page {
        size: u8,
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/folders",
                post(|Json(folder): Json<Folder>| async move { Json(folder) }),
            )
            .route(
                "/folders/{id}",
                get(|Path(id): Path<u32>| async move { id.to_string() }),
            )
            .route(
                "/list",
                get(|Query(page): Query<Page>| async move { page.size.to_string() }),
            )
    }

    async fn call(request: Request<Body>) -> (StatusCode, String, String) {
        let response = app().oneshot(request).await.unwrap();
        let status = response.status();
        let kind = response.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_string();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, kind, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn post_json(body: &str) -> Request<Body> {
        Request::post("/folders")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get_uri(uri: &str) -> Request<Body> {
        Request::get(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn a_good_request_passes_through() {
        let (status, _, body) = call(post_json(r#"{"name":"a"}"#)).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, r#"{"name":"a"}"#));
        assert_eq!(call(get_uri("/folders/7")).await.2, "7");
        assert_eq!(call(get_uri("/list?size=3")).await.2, "3");
    }

    #[tokio::test]
    async fn a_path_that_does_not_fit_its_route_is_a_server_fault() {
        let app = Router::new().route("/", get(|Path(_): Path<u32>| async {}));
        let request = Request::get("/").body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("internal_server_error"), "{body}");
        assert!(!body.contains("bad_request"), "{body}");
    }

    #[tokio::test]
    async fn every_rejection_is_a_problem_and_quotes_nothing() {
        let cases = [
            (post_json("{secret"), StatusCode::BAD_REQUEST),
            (
                post_json(r#"{"name":["secret"]}"#),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                Request::post("/folders")
                    .body(Body::from("secret"))
                    .unwrap(),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (get_uri("/folders/secret"), StatusCode::BAD_REQUEST),
            (get_uri("/list?size=secret"), StatusCode::BAD_REQUEST),
        ];
        for (request, expected) in cases {
            let (status, kind, body) = call(request).await;
            assert_eq!(status, expected, "{body}");
            assert_eq!(kind, crate::problem::PROBLEM_JSON, "{body}");
            assert!(body.contains("detail"), "{body}");
            assert!(!body.contains("secret"), "{body}");
            let code = expected.canonical_reason().unwrap();
            let code = code.to_ascii_lowercase().replace(' ', "_");
            assert!(body.contains(&format!("\"code\":\"{code}\"")), "{body}");
        }
    }

    #[derive(Deserialize, Serialize)]
    struct Change {
        #[serde(default, skip_serializing_if = "Patch::is_missing")]
        nickname: Patch<String>,
        #[serde(default)]
        size: Patch<u8>,
    }

    #[test]
    fn a_patch_tells_a_missing_field_from_null_and_from_a_value() {
        let change: Change = serde_json::from_str("{}").unwrap();
        assert_eq!(change.nickname, Patch::Missing);
        assert!(change.nickname.is_missing());
        let change: Change = serde_json::from_str(r#"{"nickname":null}"#).unwrap();
        assert_eq!(change.nickname, Patch::Null);
        assert!(change.nickname.is_null());
        let change: Change = serde_json::from_str(r#"{"nickname":"Ada","size":3}"#).unwrap();
        assert_eq!(change.nickname.value().map(String::as_str), Some("Ada"));
        assert_eq!(change.size, Patch::Value(3));
        assert_eq!(change.size.into_option(), Some(Some(3)));
    }

    #[test]
    fn applying_a_patch_keeps_clears_or_replaces() {
        let mut target = Some(1_u8);
        Patch::Missing.apply(&mut target);
        assert_eq!(target, Some(1));
        Patch::Value(2).apply(&mut target);
        assert_eq!(target, Some(2));
        Patch::Null.apply(&mut target);
        assert_eq!(target, None);
    }

    #[test]
    fn a_missing_patch_is_not_written_and_null_is() {
        let change = Change {
            nickname: Patch::Missing,
            size: Patch::Null,
        };
        assert_eq!(serde_json::to_string(&change).unwrap(), r#"{"size":null}"#);
        let change = Change {
            nickname: Patch::Value("Ada".into()),
            size: Patch::Value(3),
        };
        assert_eq!(
            serde_json::to_string(&change).unwrap(),
            r#"{"nickname":"Ada","size":3}"#
        );
    }

    #[tokio::test]
    async fn a_patch_of_the_wrong_type_is_a_422_that_quotes_nothing() {
        let app = Router::new().route(
            "/me",
            axum::routing::patch(|Json(change): Json<Change>| async move {
                format!("{:?}", change.size)
            }),
        );
        let request = Request::patch("/me")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"size":"secret"}"#))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!body.contains("secret"), "{body}");
        assert!(body.contains("#/size"), "{body}");
    }

    fn form_app() -> Router {
        Router::new().route(
            "/sign-up",
            post(|Form(folder): Form<Folder>| async move { folder.name }),
        )
    }

    async fn post_form(kind: Option<&str>, body: &str) -> (StatusCode, String) {
        let mut request = Request::post("/sign-up");
        if let Some(kind) = kind {
            request = request.header(CONTENT_TYPE, kind);
        }
        let response = form_app()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn a_form_passes_through_and_each_rejection_is_a_problem_quoting_nothing() {
        let form = Some("application/x-www-form-urlencoded");
        assert_eq!(
            post_form(form, "name=ada").await,
            (StatusCode::OK, "ada".to_string())
        );
        for (kind, body, expected) in [
            (None, "name=secret", StatusCode::UNSUPPORTED_MEDIA_TYPE),
            (
                Some("text/plain"),
                "name=secret",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (form, "other=secret", StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            let (status, body) = post_form(kind, body).await;
            assert_eq!(status, expected, "{body}");
            assert!(body.contains("\"detail\""), "{body}");
            assert!(!body.contains("secret"), "{body}");
            let code = expected.canonical_reason().unwrap();
            let code = code.to_ascii_lowercase().replace(' ', "_");
            assert!(body.contains(&format!("\"code\":\"{code}\"")), "{body}");
        }
    }

    #[derive(Serialize)]
    struct SignUp {
        email: String,
        password: String,
        age: u8,
        newsletter: bool,
        nickname: Option<String>,
        tags: Vec<String>,
    }

    fn sign_up() -> SignUp {
        SignUp {
            email: "ada@".into(),
            password: "hunter2".into(),
            age: 36,
            newsletter: true,
            nickname: None,
            tags: vec!["x".into()],
        }
    }

    #[test]
    fn a_form_context_keeps_the_values_and_errors_and_hides_a_secret() {
        let form = FormContext::new(&sign_up())
            .hide("password")
            .error("email", "Enter a whole email address")
            .error("email", "Use your work address");
        assert_eq!(form.value("email"), Some("ada@"));
        assert_eq!(form.value("age"), Some("36"));
        assert_eq!(form.value("newsletter"), Some("true"));
        assert_eq!(form.value("password"), None);
        assert_eq!(form.value("nickname"), None, "null is left out");
        assert_eq!(form.value("tags"), None, "a list is left out");
        assert_eq!(form.errors("email").len(), 2);
        assert_eq!(form.errors("age"), [] as [String; 0]);
        assert!(!form.is_valid());
        assert!(FormContext::new(&sign_up()).is_valid());
        let shown = serde_json::to_string(&form).unwrap();
        assert!(!shown.contains("hunter2"), "{shown}");
    }

    #[cfg(feature = "model")]
    #[test]
    fn a_form_context_takes_validation_failures_by_field() {
        use validator::Validate;

        #[derive(Serialize, Validate)]
        struct Contact {
            #[validate(email(message = "Enter a whole email address"))]
            email: String,
            #[validate(length(min = 1))]
            name: String,
        }

        let contact = Contact {
            email: "ada@".into(),
            name: String::new(),
        };
        let errors = contact.validate().unwrap_err();
        let form = FormContext::new(&contact).validation(&errors);
        assert_eq!(form.errors("email"), ["Enter a whole email address"]);
        assert_eq!(form.errors("name"), ["length"]);
        assert_eq!(form.value("email"), Some("ada@"));
    }

    #[cfg(feature = "tera")]
    #[test]
    fn a_tera_page_shows_the_form_escaped_with_its_errors() {
        #[derive(Serialize)]
        struct Sent {
            email: String,
        }
        let templates = crate::tera::Templates::from_raw([(
            "sign-up.html",
            r#"<input name="email" value="{{ form.values.email }}">{% for e in form.errors.email %}<p>{{ e }}</p>{% endfor %}"#,
        )])
        .unwrap();
        let form = FormContext::new(&Sent {
            email: "\"><script>".into(),
        })
        .error("email", "Enter a whole email address");
        let page = templates
            .render("sign-up.html", &serde_json::json!({ "form": form }))
            .unwrap();
        assert!(
            page.contains("value=\"&quot;&gt;&lt;script&gt;\""),
            "{page}"
        );
        assert!(
            page.contains("<p>Enter a whole email address</p>"),
            "{page}"
        );
    }

    #[test]
    fn a_safe_int_holds_what_a_json_client_holds_exactly_and_no_more() {
        assert_eq!(
            SafeInt::new(MAX_SAFE_INT).map(SafeInt::get),
            Some(MAX_SAFE_INT)
        );
        assert_eq!(
            SafeInt::new(-MAX_SAFE_INT).map(i64::from),
            Some(-MAX_SAFE_INT)
        );
        assert_eq!(SafeInt::new(MAX_SAFE_INT + 1), None);
        assert_eq!(SafeInt::new(i64::MIN), None);
        assert_eq!(SafeInt::try_from(MAX_SAFE_INT + 1), Err(MAX_SAFE_INT + 1));
        let read: SafeInt = serde_json::from_str("9007199254740991").unwrap();
        assert_eq!(serde_json::to_string(&read).unwrap(), "9007199254740991");
        assert!(serde_json::from_str::<SafeInt>("9007199254740992").is_err());
        assert!(serde_json::from_str::<SafeInt>("-9007199254740992").is_err());
        assert!(serde_json::from_str::<SafeInt>("1.5").is_err());
    }

    #[tokio::test]
    async fn a_body_field_beyond_a_safe_int_is_a_422_pointing_at_it() {
        #[derive(Deserialize)]
        struct Deal {
            #[allow(dead_code, reason = "only read")]
            amount_minor: SafeInt,
        }
        let app = axum::Router::new().route(
            "/deals",
            axum::routing::post(|Json(_): Json<Deal>| async { "kept" }),
        );
        let send = async |body: &'static str| {
            use tower::ServiceExt;
            let request = axum::http::Request::post("/deals")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        };
        assert_eq!(
            send(r#"{"amount_minor": 12345}"#).await.0,
            axum::http::StatusCode::OK
        );
        let (status, body) = send(r#"{"amount_minor": 9007199254740993}"#).await;
        assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["errors"][0]["pointer"], "#/amount_minor");
    }
}
