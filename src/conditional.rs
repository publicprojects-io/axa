//! Conditional requests: `ETag`, `If-None-Match`, and `If-Match`, as RFC 9110 has them.
//!
//! There are three parts, because they cost different things.
//!
//! **Answering `304`** needs nothing from a service. [`not_modified`] is part of the
//! standard stack. When a `GET` or `HEAD` answers with an `ETag` that the caller holds,
//! the body is dropped and the answer becomes a `304`, however the tag got there.
//!
//! **Tagging an answer** is the handler's choice, as only it knows what the tag should
//! cover. An [`ETag`] goes on a response like any other header. This one leaves out a
//! timestamp, so the tag stands still until the data moves:
//!
//! ```
//! use axa::{Result, conditional::ETag, prelude::*};
//! use serde_json::{Value, json};
//!
//! async fn dashboard() -> Result<Response> {
//!     let payload = json!({ "open": 3 });
//!     let tag = ETag::weak_of(&payload)?;
//!     let mut body = payload;
//!     body["as_of"] = Value::from("2026-11-07T09:00:00Z");
//!     Ok((tag, format::json(body)?).into_response())
//! }
//! ```
//!
//! [`etag`] tags answers by hashing their bodies, for a route or router that opts in:
//!
//! ```
//! use axa::conditional;
//! use axum::{Router, middleware::from_fn, routing::get};
//!
//! # async fn list() -> &'static str { "[]" }
//! let routes: Router = Router::new()
//!     .route("/catalogue", get(list))
//!     .layer(from_fn(conditional::etag));
//! ```
//!
//! **Refusing a stale write** needs the record's current tag, so a handler asks, after
//! it has authorised the caller and loaded the record. A record that is not there is
//! a `404` before this is asked:
//!
//! ```
//! use axa::{Result, conditional::{self, ETag}, prelude::*};
//! use axum::http::HeaderMap;
//!
//! async fn rename(headers: HeaderMap) -> Result<Response> {
//!     # let version = 7;
//!     conditional::if_match(&headers, &ETag::strong(&format!("v{version}"))?)?;
//!     format::empty()
//! }
//! ```
//!
//! The check and the write are two steps. To close the gap, make them one, with a lock
//! on the row or `UPDATE ... WHERE version = $1`.
//!
//! # What this saves
//!
//! The handler has run by the time an answer can be compared, so a `304` saves the
//! bytes and not the query. A handler that wants to skip its work can return early.
//!
//! # Who can learn what
//!
//! A `304` is made from the answer the caller was about to get. Nothing is stored and
//! nothing short-circuits the request, so a caller who is refused gets the refusal
//! whatever they hold, and a tag covers what this caller may see. That holds wherever
//! the authentication layer sits.
//!
//! # Weak and strong tags
//!
//! A strong tag says two answers are the same bytes, and a compressed answer is not the
//! same bytes as a plain one. [`etag`] sits inside compression, where it reads the plain
//! body, so it makes a weak tag, which says only that the answers mean the same.
//! `If-None-Match` compares weakly, so a tag held from a compressed answer still matches.
//!
//! `If-Match` compares strongly, as RFC 9110 requires, so a weak tag never passes it. A
//! tag meant for `If-Match` is a strong one, such as a version. A handler's strong tag
//! is left alone when its answer is compressed, so an encoded answer shares a tag with a
//! plain one. It does no harm here, as no route of `axa`'s answers a `Range`, but
//! a service answering ranges should use a tag that differs by encoding.
//!
//! # Hashing
//!
//! The hash is std's `DefaultHasher`, so no dependency is needed. It gives every replica
//! the same tag, and a new Rust toolchain may change it, which costs each caller one
//! full answer. It catches accidents and not an adversary, so a tag defending against
//! someone who writes the data should be a version counter, or a real digest passed to
//! [`ETag::strong`].

use std::{
    convert::Infallible,
    fmt,
    hash::{DefaultHasher, Hasher},
    pin::Pin,
    task::{Context, Poll},
};

use axum::{
    body::{Body, Bytes, HttpBody, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponseParts, Response, ResponseParts},
};
use hyper::body::Frame;
use serde::Serialize;

use crate::{Error, Result, error::ErrorDetail};

/// The largest body [`etag`] hashes, in bytes. A larger one passes through untagged.
pub const MAX_BODY: usize = 8 * 1024 * 1024;

/// What describes a body, which a `304` has none of. Everything else stays, as a
/// header that would have been on the `200` still says what it said.
const CONTENT_HEADERS: [HeaderName; 8] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_ENCODING,
    header::CONTENT_LANGUAGE,
    header::CONTENT_RANGE,
    header::CONTENT_DISPOSITION,
    header::TRANSFER_ENCODING,
    header::TRAILER,
];

/// An entity tag, ready to go on a response.
///
/// It goes in a tuple before the response, as any header does, and its [`Display`] text
/// is what a caller sends back, for a preview that puts the tag in its body.
///
/// [`Display`]: fmt::Display
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ETag(HeaderValue);

impl ETag {
    /// A tag saying two answers mean the same, for `If-None-Match`.
    ///
    /// # Errors
    ///
    /// Fails when `text` has a character a tag cannot hold: a space, a double quote, or
    /// anything outside visible ASCII. That is the service's mistake, so it is a `500`.
    pub fn weak(text: &str) -> Result<Self> {
        Self::new("W/", text)
    }

    /// A tag saying two answers are the same, for `If-Match`. A version or a digest
    /// of the record is usual.
    ///
    /// # Errors
    ///
    /// As for [`ETag::weak`].
    pub fn strong(text: &str) -> Result<Self> {
        Self::new("", text)
    }

    /// The weak tag of `value`'s JSON, which is the tag [`etag`] gives `Json(value)`.
    ///
    /// # Errors
    ///
    /// Fails when `value` does not serialize.
    pub fn weak_of<T: Serialize>(value: &T) -> Result<Self> {
        Self::weak(&digest(&serde_json::to_vec(value)?))
    }

    /// The strong tag of `value`'s JSON, for an `If-Match` on a record with no version.
    /// `value` is what must not change, and it need not be what the answer holds.
    ///
    /// # Errors
    ///
    /// Fails when `value` does not serialize.
    pub fn strong_of<T: Serialize>(value: &T) -> Result<Self> {
        Self::strong(&digest(&serde_json::to_vec(value)?))
    }

    fn new(prefix: &str, text: &str) -> Result<Self> {
        if !text
            .bytes()
            .all(|byte| byte.is_ascii() && is_tag_byte(byte))
        {
            return Err(Error::string(
                "an entity tag has a character it cannot hold",
            ));
        }
        HeaderValue::from_str(&format!("{prefix}\"{text}\""))
            .map(Self)
            .map_err(Error::wrap)
    }

    fn parsed(&self) -> Option<Tag<'_>> {
        tag_alone(self.0.as_bytes())
    }
}

impl fmt::Display for ETag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.to_str().unwrap_or_default())
    }
}

impl IntoResponseParts for ETag {
    type Error = Infallible;

    fn into_response_parts(
        self,
        mut parts: ResponseParts,
    ) -> std::result::Result<ResponseParts, Self::Error> {
        parts.headers_mut().insert(header::ETAG, self.0);
        Ok(parts)
    }
}

/// The opaque text of a generated tag: the length, then a hash of the bytes.
fn digest(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    format!("{:x}-{:016x}", bytes.len(), hasher.finish())
}

/// One tag as it was written, borrowing from the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tag<'a> {
    weak: bool,
    opaque: &'a [u8],
}

/// `If-None-Match`'s test: the same text, whichever is weak.
fn weak_match(sent: Tag<'_>, current: Tag<'_>) -> bool {
    sent.opaque == current.opaque
}

/// `If-Match`'s test: the same text, and neither weak.
fn strong_match(sent: Tag<'_>, current: Tag<'_>) -> bool {
    !sent.weak && !current.weak && sent.opaque == current.opaque
}

/// `etagc` in RFC 9110: what may sit between the quotes.
const fn is_tag_byte(byte: u8) -> bool {
    matches!(byte, 0x21 | 0x23..=0x7e | 0x80..=0xff)
}

/// Read one tag from the start of `bytes`, and give back what follows it.
fn read_tag(bytes: &[u8]) -> Option<(Tag<'_>, &[u8])> {
    let (weak, rest) = match bytes.strip_prefix(b"W/") {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let rest = rest.strip_prefix(b"\"")?;
    let end = rest.iter().position(|byte| !is_tag_byte(*byte))?;
    (rest[end] == b'"').then_some((
        Tag {
            weak,
            opaque: &rest[..end],
        },
        &rest[end + 1..],
    ))
}

/// A header holding exactly one tag, as `ETag` does.
fn tag_alone(bytes: &[u8]) -> Option<Tag<'_>> {
    let (tag, rest) = read_tag(bytes.trim_ascii())?;
    rest.is_empty().then_some(tag)
}

/// Read a list of tags, which may run over several header lines. Commas are read
/// as separators only outside the quotes, as a tag can hold one, and RFC 9110 has a
/// reader accept empty members. `None` means it is not a list of tags.
fn read_tags<'a>(values: &[&'a HeaderValue], tags: &mut Vec<Tag<'a>>) -> Option<()> {
    for value in values {
        let mut rest = value.as_bytes().trim_ascii();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix(b",") {
                rest = after.trim_ascii_start();
                continue;
            }
            let (tag, after) = read_tag(rest)?;
            tags.push(tag);
            rest = after.trim_ascii_start();
            if !rest.is_empty() && !rest.starts_with(b",") {
                return None;
            }
        }
    }
    Some(())
}

/// Whether a condition header is satisfied by the answer tagged `current`.
/// `*` stands for any answer at all, and must stand alone. Anything unreadable holds
/// nothing, so a bad `If-None-Match` gets the whole answer and a bad `If-Match` is refused.
fn holds(
    values: &[&HeaderValue],
    current: Option<Tag<'_>>,
    compare: fn(Tag<'_>, Tag<'_>) -> bool,
) -> bool {
    if let [value] = values
        && value.as_bytes().trim_ascii() == b"*"
    {
        return true;
    }
    let mut tags = Vec::new();
    read_tags(values, &mut tags).is_some()
        && current.is_some_and(|current| tags.iter().any(|tag| compare(*tag, current)))
}

/// The body of a `304`. An empty `Body` says its size is zero, and Axum then writes
/// `content-length: 0`, which RFC 9110 forbids here: on a `304` that header can only state
/// the length of the `200`. This one does not say, so nothing is written.
struct NoBody;

impl HttpBody for NoBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, Infallible>>> {
        Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        true
    }
}

/// The `304` for an answer: its headers and no body. The body was never read, so a
/// compressed one was never compressed.
fn unchanged(response: Response) -> Response {
    let (mut parts, _) = response.into_parts();
    parts.status = StatusCode::NOT_MODIFIED;
    for name in CONTENT_HEADERS {
        parts.headers.remove(name);
    }
    Response::from_parts(parts, Body::new(NoBody))
}

/// Answer `304` to a `GET` or `HEAD` that holds the `ETag` of its answer.
///
/// This is part of the standard stack, outside compression, so that a `304` has the
/// `vary` the `200` would have had. A service with a stack of its own can use it alone:
/// `.layer(axum::middleware::from_fn(axa::conditional::not_modified))`.
///
/// Only a `2xx` is compared, as RFC 9110 says of every precondition, so a refusal stays
/// a refusal. Another method has already done its work by the time an answer is here,
/// so it is never touched. `If-None-Match: *` matches any `2xx`, tagged or not.
pub async fn not_modified(request: Request, next: Next) -> Response {
    let conditional = matches!(*request.method(), Method::GET | Method::HEAD);
    let held: Vec<HeaderValue> = if conditional {
        let values = request.headers().get_all(header::IF_NONE_MATCH);
        values.iter().cloned().collect()
    } else {
        Vec::new()
    };
    let response = next.run(request).await;
    if held.is_empty() || !response.status().is_success() {
        return response;
    }
    let current = response
        .headers()
        .get(header::ETAG)
        .and_then(|value| tag_alone(value.as_bytes()));
    let held: Vec<&HeaderValue> = held.iter().collect();
    if holds(&held, current, weak_match) {
        unchanged(response)
    } else {
        response
    }
}

/// Whether `Cache-Control` says this answer must not be kept.
fn is_no_store(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"))
}

/// Whether [`etag`] should hash this answer. A body with no exact size is a stream, and
/// one with an exact size is nearly always in memory already, so this is what keeps
/// streams and server-sent events unbuffered, however large or long they are.
fn should_tag(response: &Response) -> bool {
    response.status() == StatusCode::OK
        && !response.headers().contains_key(header::ETAG)
        && !response.headers().contains_key(header::CONTENT_ENCODING)
        && !is_no_store(response.headers())
        && response
            .body()
            .size_hint()
            .exact()
            .is_some_and(|size| size > 0 && size <= MAX_BODY as u64)
}

/// Give a `GET`'s `200` a weak `ETag` from a hash of its body.
///
/// Add it to a router, a `Routes` handler, or a single route, before
/// [`middleware::apply`](crate::middleware::apply), as with any other layer. Keep it
/// outside what shapes the body, such as paging, so that the tag covers what is sent,
/// and outside whatever sets `Cache-Control`, so that it can see `no-store`.
///
/// An answer is left alone when any of these is true:
///
/// - It already has an `ETag`, as a handler's own tag knows better than a hash.
/// - It says `no-store`, as nothing will revalidate it, and a hash of personal data
///   should not go out for no gain.
/// - It is empty, or over [`MAX_BODY`], or a stream of no known size.
/// - It is already encoded, because then this layer is outside compression.
///
/// `HEAD` gets no generated tag, because Axum has dropped the body before this sees it.
/// RFC 9110 lets a `HEAD` leave out what only the body can tell.
///
/// A body with a timestamp or a request id in it gets a new tag each time. That gains
/// nothing, as a caller never matches, and harms nothing, as a caller gets the whole
/// answer. Such a route wants [`ETag::weak_of`] over the part that matters.
pub async fn etag(request: Request, next: Next) -> Response {
    let is_get = request.method() == Method::GET;
    let response = next.run(request).await;
    if !is_get || !should_tag(&response) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_BODY).await else {
        // The headers have not gone yet, so a body that failed can still be an honest
        // error. Its text is not logged, as it can quote anything.
        tracing::error!("response_body_failed");
        return crate::error::internal_response();
    };
    if let Ok(tag) = ETag::weak(&digest(&bytes)) {
        parts.headers.insert(header::ETAG, tag.0);
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// Whether the caller's `If-Match` holds for a record tagged `current`, or `None` if
/// they sent none. For a service with its own answer to a stale write.
///
/// It compares strongly, so a weak tag on either side does not hold. `*` does.
#[must_use]
pub fn if_match_holds(headers: &HeaderMap, current: &ETag) -> Option<bool> {
    let values: Vec<&HeaderValue> = headers.get_all(header::IF_MATCH).iter().collect();
    (!values.is_empty()).then(|| holds(&values, current.parsed(), strong_match))
}

fn precondition_failed() -> Error {
    Error::CustomError(
        StatusCode::PRECONDITION_FAILED,
        ErrorDetail::new(
            "precondition_failed",
            "this changed since it was read; read it again, then retry",
        ),
    )
}

/// Refuse a write whose `If-Match` is out of date, and let through one with none, for
/// a route where callers are not yet made to send it.
///
/// # Errors
///
/// A `412` problem with the `code` `precondition_failed`.
pub fn if_match(headers: &HeaderMap, current: &ETag) -> Result<()> {
    if if_match_holds(headers, current) == Some(false) {
        return Err(precondition_failed());
    }
    Ok(())
}

/// Refuse a write whose `If-Match` is out of date or missing.
///
/// # Errors
///
/// A `428` problem with the `code` `precondition_required` when there is no `If-Match`,
/// and otherwise as for [`if_match`].
pub fn require_if_match(headers: &HeaderMap, current: &ETag) -> Result<()> {
    match if_match_holds(headers, current) {
        Some(true) => Ok(()),
        Some(false) => Err(precondition_failed()),
        None => Err(Error::CustomError(
            StatusCode::PRECONDITION_REQUIRED,
            ErrorDetail::new(
                "precondition_required",
                "send If-Match with the tag from your last read",
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &'static str) -> HeaderValue {
        HeaderValue::from_static(text)
    }

    fn weakly(sent: &[&'static str], current: &'static str) -> bool {
        let values: Vec<HeaderValue> = sent.iter().map(|text| value(text)).collect();
        let values: Vec<&HeaderValue> = values.iter().collect();
        holds(&values, tag_alone(current.as_bytes()), weak_match)
    }

    fn strongly(sent: &'static str, current: &'static str) -> bool {
        let sent = value(sent);
        holds(&[&sent], tag_alone(current.as_bytes()), strong_match)
    }

    #[test]
    fn weak_comparison_ignores_who_is_weak() {
        assert!(weakly(&["\"a\""], "\"a\""));
        assert!(weakly(&["W/\"a\""], "\"a\""));
        assert!(weakly(&["\"a\""], "W/\"a\""));
        assert!(weakly(&["W/\"a\""], "W/\"a\""));
        assert!(!weakly(&["W/\"a\""], "W/\"b\""));
    }

    #[test]
    fn strong_comparison_refuses_any_weak_tag() {
        assert!(strongly("\"a\"", "\"a\""));
        assert!(!strongly("W/\"a\"", "\"a\""));
        assert!(!strongly("\"a\"", "W/\"a\""));
        assert!(!strongly("W/\"a\"", "W/\"a\""));
        assert!(!strongly("\"a\"", "\"b\""));
    }

    #[test]
    fn a_list_is_read_with_commas_inside_tags_and_empty_members() {
        assert!(weakly(&["\"a\", W/\"b\" ,\"c\""], "\"b\""));
        assert!(weakly(&["\"x,y\""], "\"x,y\""));
        assert!(!weakly(&["\"x,y\""], "\"x\""));
        assert!(!weakly(&["\"x,y\""], "\"y\""));
        assert!(weakly(&[", \"a\",, \"b\","], "\"b\""));
    }

    #[test]
    fn a_list_can_run_over_several_header_lines() {
        assert!(weakly(&["\"a\"", "\"b\""], "\"b\""));
    }

    #[test]
    fn a_star_stands_for_anything_but_only_alone() {
        assert!(weakly(&["*"], "\"a\""));
        assert!(weakly(&[" * "], "\"a\""));
        assert!(!weakly(&["*, \"b\""], "\"a\""));
        assert!(!weakly(&["\"b\"", "*"], "\"a\""));
        let star = value("*");
        assert!(holds(&[&star], None, weak_match));
    }

    #[test]
    fn what_is_not_a_tag_matches_nothing() {
        for sent in [
            "a",
            "w/\"a\"",
            "\"a",
            "W/ \"a\"",
            "\"a\" \"a\"",
            "\"a\" junk",
            "\"a b\"",
            "\"a\", *",
        ] {
            assert!(!weakly(&[sent], "\"a\""), "{sent}");
        }
        assert!(!weakly(&["\"a\""], "a"));
    }

    #[test]
    fn bytes_outside_ascii_are_compared_as_bytes() {
        let sent = HeaderValue::from_bytes(b"\"\xE9\"").unwrap();
        assert!(holds(&[&sent], tag_alone(b"\"\xE9\""), weak_match));
    }

    #[test]
    fn no_store_is_read_as_a_directive() {
        let with = |text: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, value(text));
            is_no_store(&headers)
        };
        assert!(with("no-store"));
        assert!(with("private, No-Store"));
        assert!(!with("no-cache"));
        assert!(!with("private, max-age=60"));
        let mut headers = HeaderMap::new();
        headers.append(header::CACHE_CONTROL, value("private"));
        headers.append(header::CACHE_CONTROL, value("no-store"));
        assert!(is_no_store(&headers));
    }

    #[test]
    fn a_tag_holds_only_what_a_tag_can_hold() {
        assert_eq!(ETag::strong("v7").unwrap().to_string(), "\"v7\"");
        assert_eq!(ETag::weak("v7").unwrap().to_string(), "W/\"v7\"");
        assert_eq!(ETag::strong("").unwrap().to_string(), "\"\"");
        for text in ["a b", "a\"b", "a\nb", "é"] {
            assert!(ETag::strong(text).is_err(), "{text}");
            assert!(ETag::weak(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_digest_is_the_same_each_time_and_differs_by_content() {
        assert_eq!(digest(b"a"), digest(b"a"));
        assert_ne!(digest(b"a"), digest(b"b"));
        assert!(digest(b"abc").starts_with("3-"));
    }
}
