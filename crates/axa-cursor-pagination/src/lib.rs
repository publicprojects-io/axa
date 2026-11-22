//! Signed cursor pagination with no total, and `LIKE` escaping.
//!
//! A collection read takes `?limit=` and an opaque `?cursor=`. When more rows
//! follow, the response carries `Link: <...>; rel="next"`, and the last page
//! carries none. The body is a bare JSON array, and there is no total, because a
//! total would count rows the caller's record policy hides.
//!
//! A cursor is signed with the service's [`CursorKey`], over its position and the
//! query it belongs to: the path and every query parameter but `cursor` and
//! `limit`. So a caller cannot make one up, edit one, or carry one to another
//! list or filter, and may change `limit` between pages. A bad cursor or a bad
//! limit is a [`PageError`], which is the caller's to fix and answers `422`.
//!
//! The position is an offset. A row added or removed between two fetches can make
//! a later page skip or repeat one row, so order every list by something that
//! ends in a unique column, such as `ORDER BY created_at, id`, so that the order
//! itself is stable.

use std::fmt;

use axum::{
    Json,
    http::{HeaderValue, StatusCode, Uri, header::LINK},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The shortest secret [`CursorKey::new`] accepts, in bytes.
pub const MIN_KEY_LEN: usize = 32;

/// The first byte of every cursor, so the format can change.
const VERSION: u8 = 1;
const POSITION_LEN: usize = 8;
const TAG_LEN: usize = 32;
const CURSOR_LEN: usize = 1 + POSITION_LEN + TAG_LEN;

/// The service's secret for signing cursors. Its `Debug` never shows the key.
#[derive(Clone)]
pub struct CursorKey {
    mac: Hmac<Sha256>,
}

impl fmt::Debug for CursorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CursorKey(..)")
    }
}

/// A secret shorter than [`MIN_KEY_LEN`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyTooShort {
    /// The length given, in bytes.
    pub len: usize,
}

impl fmt::Display for KeyTooShort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a cursor key needs at least {MIN_KEY_LEN} bytes, and this has {}",
            self.len
        )
    }
}

impl std::error::Error for KeyTooShort {}

impl CursorKey {
    /// A key from `secret`, which must be at least [`MIN_KEY_LEN`] bytes. Keep it
    /// as secret as a session key: anyone holding it can make cursors.
    ///
    /// # Errors
    ///
    /// [`KeyTooShort`] when `secret` is shorter than [`MIN_KEY_LEN`].
    pub fn new(secret: &[u8]) -> Result<Self, KeyTooShort> {
        if secret.len() < MIN_KEY_LEN {
            return Err(KeyTooShort { len: secret.len() });
        }
        let mac = Hmac::<Sha256>::new_from_slice(secret)
            .map_err(|_| KeyTooShort { len: secret.len() })?;
        Ok(Self { mac })
    }

    fn tag(&self, position: u64, query: &[u8; 32]) -> Hmac<Sha256> {
        let mut mac = self.mac.clone();
        mac.update(&[VERSION]);
        mac.update(&position.to_be_bytes());
        mac.update(query);
        mac
    }

    fn encode(&self, position: u64, query: &[u8; 32]) -> String {
        let mut bytes = Vec::with_capacity(CURSOR_LEN);
        bytes.push(VERSION);
        bytes.extend_from_slice(&position.to_be_bytes());
        bytes.extend_from_slice(&self.tag(position, query).finalize().into_bytes());
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn decode(&self, cursor: &str, query: &[u8; 32]) -> Option<u64> {
        let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
        let (&version, rest) = bytes.split_first()?;
        if bytes.len() != CURSOR_LEN || version != VERSION {
            return None;
        }
        let (position, tag) = rest.split_at_checked(POSITION_LEN)?;
        let position = u64::from_be_bytes(position.try_into().ok()?);
        // Constant time, so a caller cannot learn the tag a byte at a time.
        self.tag(position, query).verify_slice(tag).ok()?;
        Some(position)
    }
}

/// The page sizes of one endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorLimits {
    /// The limit when none is given.
    pub default_limit: u64,
    /// The largest limit accepted. A larger one is refused, not clamped.
    pub max_limit: u64,
}

/// Why a page request was refused. Each is the caller's to fix, so each answers
/// `422`, and none quotes what the caller sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PageError {
    /// A limit that is zero, not a number, given twice, or above the maximum.
    BadLimit {
        /// The largest limit accepted.
        max_limit: u64,
    },
    /// A cursor that is malformed, carries a bad signature, is given twice, or
    /// belongs to another query.
    BadCursor,
}

impl PageError {
    /// The status to answer with, which is always `422`.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        StatusCode::UNPROCESSABLE_ENTITY
    }
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadLimit { max_limit } => {
                write!(f, "limit must be a whole number from 1 to {max_limit}")
            }
            Self::BadCursor => f.write_str("cursor is not valid"),
        }
    }
}

impl std::error::Error for PageError {}

/// One page of a list, read from the request's URI and checked.
///
/// ```
/// use axa_cursor_pagination::{CursorKey, CursorLimits, CursorPage};
///
/// let key = CursorKey::new(&[7; 32]).unwrap();
/// let limits = CursorLimits { default_limit: 50, max_limit: 200 };
/// let uri = "/things?status=open&limit=2".parse().unwrap();
/// let page = CursorPage::new(&key, limits, &uri).unwrap();
/// assert_eq!(page.bounds(), (3, 0)); // one more row than asked, to see if more follow
///
/// let response = page.respond(vec!["a", "b", "c"]);
/// let link = response.headers()["link"].to_str().unwrap();
/// assert!(link.starts_with("</things?status=open&limit=2&cursor="));
/// assert!(link.ends_with(r#">; rel="next""#));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorPage {
    limit: u64,
    offset: u64,
    /// `LIMIT` and `OFFSET` as signed numbers, checked to fit when the page is read.
    bounds: (i64, i64),
    next: String,
}

/// The query a cursor belongs to: the path and the parameters other than
/// `cursor` and `limit`, sorted so that their order does not matter, and the context,
/// if any. An empty context adds nothing, so the cursors [`CursorPage::new`] makes are
/// those it made before contexts were added. A context is marked by a byte no query
/// part can hold, and its length, so no context reads as another or as a filter.
/// A length as the 8 bytes of a big-endian `u64`, whatever the width of `usize`.
fn length_bytes(length: usize) -> [u8; 8] {
    let mut bytes = [0_u8; 8];
    for (to, from) in bytes
        .iter_mut()
        .rev()
        .zip(length.to_be_bytes().iter().rev())
    {
        *to = *from;
    }
    bytes
}

fn query_hash(path: &str, kept: &[&str], context: &str) -> [u8; 32] {
    let mut sorted = kept.to_vec();
    sorted.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(path.as_bytes());
    for part in sorted {
        hash.update([0]);
        hash.update(part.as_bytes());
    }
    if !context.is_empty() {
        hash.update([1]);
        hash.update(length_bytes(context.len()));
        hash.update(context.as_bytes());
    }
    hash.finalize().into()
}

fn name(part: &str) -> &str {
    part.split_once('=').map_or(part, |(name, _)| name)
}

fn value(part: &str) -> &str {
    part.split_once('=').map_or("", |(_, value)| value)
}

impl CursorPage {
    /// Read `limit` and `cursor` from `uri`, the request's URI.
    ///
    /// # Errors
    ///
    /// [`PageError::BadLimit`] for a limit that is zero, not a number, above
    /// `limits.max_limit`, or given twice. [`PageError::BadCursor`] for a cursor
    /// that this key did not sign for this path and these filters.
    pub fn new(key: &CursorKey, limits: CursorLimits, uri: &Uri) -> Result<Self, PageError> {
        Self::new_in(key, limits, uri, "")
    }

    /// As [`CursorPage::new`], with cursors that hold only in `context`, such as the
    /// caller's tenant. A cursor made in one context is a [`PageError::BadCursor`] in
    /// any other, so a cursor carried from one tenant to another is refused even where
    /// a query forgets to filter by tenant. An empty context is as [`CursorPage::new`].
    ///
    /// ```
    /// use axa_cursor_pagination::{CursorKey, CursorLimits, CursorPage, PageError};
    ///
    /// let key = CursorKey::new(&[7; 32]).unwrap();
    /// let limits = CursorLimits { default_limit: 50, max_limit: 200 };
    /// let first = CursorPage::new_in(&key, limits, &"/todos?limit=1".parse().unwrap(), "acme").unwrap();
    /// let link = first.next_link().unwrap();
    /// let next = link.to_str().unwrap().trim_start_matches('<').split('>').next().unwrap();
    /// let uri = next.parse().unwrap();
    /// assert!(CursorPage::new_in(&key, limits, &uri, "acme").is_ok());
    /// assert_eq!(CursorPage::new_in(&key, limits, &uri, "globex"), Err(PageError::BadCursor));
    /// ```
    ///
    /// # Errors
    ///
    /// As [`CursorPage::new`], and [`PageError::BadCursor`] for a cursor signed for
    /// another context.
    pub fn new_in(
        key: &CursorKey,
        limits: CursorLimits,
        uri: &Uri,
        context: &str,
    ) -> Result<Self, PageError> {
        let parts: Vec<&str> = uri
            .query()
            .unwrap_or("")
            .split('&')
            .filter(|part| !part.is_empty())
            .collect();
        let named = |wanted: &str| -> Vec<&str> {
            parts
                .iter()
                .filter(|part| name(part) == wanted)
                .map(|part| value(part))
                .collect()
        };
        let bad_limit = PageError::BadLimit {
            max_limit: limits.max_limit,
        };
        let limit = match named("limit").as_slice() {
            [] => limits.default_limit,
            [raw] => raw
                .parse::<u64>()
                .ok()
                .filter(|limit| (1..=limits.max_limit).contains(limit))
                .ok_or(bad_limit)?,
            _ => return Err(bad_limit),
        };
        let kept: Vec<&str> = parts
            .iter()
            .copied()
            .filter(|part| !matches!(name(part), "cursor" | "limit"))
            .collect();
        let query = query_hash(uri.path(), &kept, context);
        let offset = match named("cursor").as_slice() {
            [] => 0,
            [cursor] => key.decode(cursor, &query).ok_or(PageError::BadCursor)?,
            _ => return Err(PageError::BadCursor),
        };

        // A page that ends past what a number holds, or a bound that SQL cannot take, is
        // not a position a cursor can name.
        let next_offset = offset.checked_add(limit).ok_or(PageError::BadCursor)?;
        let rows = limit
            .checked_add(1)
            .and_then(|rows| i64::try_from(rows).ok());
        let skip = i64::try_from(offset).ok();
        let (Some(rows), Some(skip), Ok(_)) = (rows, skip, i64::try_from(next_offset)) else {
            return Err(PageError::BadCursor);
        };
        let bounds = (rows, skip);
        let next_cursor = key.encode(next_offset, &query);
        let mut next_query: Vec<&str> = parts
            .iter()
            .copied()
            .filter(|part| name(part) != "cursor")
            .collect();
        let cursor_part = format!("cursor={next_cursor}");
        next_query.push(&cursor_part);
        let next = format!("<{}?{}>; rel=\"next\"", uri.path(), next_query.join("&"));
        Ok(Self {
            limit,
            offset,
            bounds,
            next,
        })
    }

    /// Rows to return.
    #[must_use]
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// Rows to skip.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// `LIMIT` and `OFFSET` as Postgres takes them, which is as signed numbers.
    /// The limit is one more than the page, so that [`CursorPage::split`] can tell
    /// whether more rows follow. A page whose numbers do not fit `i64` is refused when
    /// it is read, as [`PageError::BadCursor`], so these always fit.
    #[must_use]
    pub const fn bounds(&self) -> (i64, i64) {
        self.bounds
    }

    /// Drop the extra row [`CursorPage::bounds`] asked for. The flag says whether
    /// more rows follow.
    #[must_use]
    pub fn split<T>(&self, mut rows: Vec<T>) -> (Vec<T>, bool) {
        let keep = usize::try_from(self.limit).unwrap_or(usize::MAX);
        let more = rows.len() > keep;
        rows.truncate(keep);
        (rows, more)
    }

    /// The `Link` header for the next page.
    #[must_use]
    pub fn next_link(&self) -> Option<HeaderValue> {
        HeaderValue::from_str(&self.next).ok()
    }

    /// `rows`, fetched with [`CursorPage::bounds`], as a bare JSON array, with
    /// `Link: rel="next"` when more rows follow. There is no total.
    #[must_use]
    pub fn respond<T: Serialize>(&self, rows: Vec<T>) -> Response {
        let (rows, more) = self.split(rows);
        let mut response = Json(rows).into_response();
        if more && let Some(link) = self.next_link() {
            response.headers_mut().insert(LINK, link);
        }
        response
    }
}

/// Escape `%`, `_`, and `\` so user text matches literally in a `LIKE`
/// pattern. Bind the result as a parameter with `ESCAPE '\'`.
///
/// ```
/// assert_eq!(axa_cursor_pagination::escape_like("100%_a"), r"100\%\_a");
/// ```
#[must_use]
pub fn escape_like(text: &str) -> String {
    // Backslash first, so the escapes added below are not escaped again.
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "RCA-SCOPE-4: tests may use plain operators"
)]
#[cfg(test)]
#[allow(clippy::string_slice, reason = "RCA-SCOPE-4: tests may slice strings")]
mod tests {
    use super::*;

    const LIMITS: CursorLimits = CursorLimits {
        default_limit: 10,
        max_limit: 50,
    };

    fn key() -> CursorKey {
        CursorKey::new(b"a service secret of thirty-two b").unwrap()
    }

    fn page(uri: &str) -> Result<CursorPage, PageError> {
        CursorPage::new(&key(), LIMITS, &uri.parse().unwrap())
    }

    /// The cursor in a page's next link.
    fn next_cursor(page: &CursorPage) -> String {
        let link = page.next.clone();
        let start = link.find("cursor=").unwrap() + "cursor=".len();
        let end = link[start..].find(['&', '>']).unwrap() + start;
        link[start..end].to_string()
    }

    #[test]
    fn a_cursor_made_in_one_context_is_refused_in_another() {
        let in_context = |uri: &str, context: &str| {
            CursorPage::new_in(&key(), LIMITS, &uri.parse().unwrap(), context)
        };
        let first = in_context("/todos?status=open", "acme").unwrap();
        let next = format!("/todos?status=open&cursor={}", next_cursor(&first));
        assert_eq!(in_context(&next, "acme").unwrap().offset(), 10);
        for other in ["globex", "acme ", "acm", ""] {
            assert_eq!(
                in_context(&next, other),
                Err(PageError::BadCursor),
                "{other:?}"
            );
        }
        assert_eq!(page(&next), Err(PageError::BadCursor));

        // No context is the cursor `new` has always made, so old cursors still hold.
        let plain = page("/todos?status=open").unwrap();
        let next = format!("/todos?status=open&cursor={}", next_cursor(&plain));
        assert_eq!(in_context(&next, "").unwrap().offset(), 10);
        assert_eq!(in_context(&next, "acme"), Err(PageError::BadCursor));
        // The cursor `new` makes for a fixed key and query, worked out apart from this
        // code as the crate made it before contexts were added.
        assert_eq!(
            next_cursor(&page("/things").unwrap()),
            "AQAAAAAAAAAKZZFSvWeCDy_beDgOauJnX_EjAV2tWtfGim0KJozjf_I"
        );
    }

    #[test]
    fn a_missing_limit_uses_the_default_and_the_first_page_starts_at_zero() {
        let page = page("/things").unwrap();
        assert_eq!((page.limit(), page.offset()), (10, 0));
    }

    #[test]
    fn there_is_no_offset_parameter() {
        // PAG-7: `?offset=` is only a filter like any other, so it moves nothing,
        // and a cursor made without it does not fit a query that has it.
        let page = page("/things?offset=40").unwrap();
        assert_eq!(page.offset(), 0);
    }

    #[test]
    fn a_response_carries_no_total() {
        // PAG-7 and PAG-10.
        let response = page("/things?limit=2").unwrap().respond(vec![1, 2, 3]);
        for header in ["x-total-count", "x-limit", "x-offset"] {
            assert!(response.headers().get(header).is_none(), "{header}");
        }
    }

    #[test]
    fn the_next_page_is_a_link_and_the_last_page_has_none() {
        // PAG-8.
        let page = page("/things?status=open&limit=2").unwrap();
        let more = page.respond(vec![1, 2, 3]);
        assert_eq!(more.status(), StatusCode::OK);
        let link = more.headers()[LINK].to_str().unwrap();
        assert_eq!(
            link,
            format!(
                "</things?status=open&limit=2&cursor={}>; rel=\"next\"",
                next_cursor(&page)
            )
        );
        let last = page.respond(vec![1, 2]);
        assert!(last.headers().get(LINK).is_none());
    }

    #[test]
    fn a_body_is_a_bare_json_array_of_the_page() {
        let response = page("/things?limit=2")
            .unwrap()
            .respond(vec!["a", "b", "c"]);
        assert_eq!(response.headers()["content-type"], "application/json");
        let (rows, more) = page("/things?limit=2").unwrap().split(vec!["a", "b", "c"]);
        assert_eq!((rows, more), (vec!["a", "b"], true));
    }

    #[test]
    fn a_cursor_round_trips_to_the_next_page() {
        let first = page("/things?status=open&limit=2").unwrap();
        let cursor = next_cursor(&first);
        let second = page(&format!("/things?status=open&limit=2&cursor={cursor}")).unwrap();
        assert_eq!(second.offset(), 2);
        let third = page(&format!(
            "/things?status=open&limit=2&cursor={}",
            next_cursor(&second)
        ))
        .unwrap();
        assert_eq!(third.offset(), 4);
    }

    #[test]
    fn the_filters_may_come_in_any_order_and_the_limit_may_change() {
        let cursor = next_cursor(&page("/things?a=1&b=2&limit=2").unwrap());
        let later = page(&format!("/things?limit=5&b=2&cursor={cursor}&a=1")).unwrap();
        assert_eq!((later.offset(), later.limit()), (2, 5));
    }

    #[test]
    fn a_bad_limit_is_refused_not_clamped() {
        // PAG-9.
        for limit in ["0", "51", "many", "-1", "", "1.5", "99999999999999999999"] {
            let error = page(&format!("/things?limit={limit}")).unwrap_err();
            assert_eq!(error, PageError::BadLimit { max_limit: 50 }, "{limit}");
            assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
        assert!(page("/things?limit=2&limit=3").is_err());
        assert_eq!(page("/things?limit=50").unwrap().limit(), 50);
        assert_eq!(page("/things?limit=1").unwrap().limit(), 1);
    }

    #[test]
    fn a_made_up_cursor_is_refused() {
        // PAG-11.
        let forged = URL_SAFE_NO_PAD.encode([VERSION; CURSOR_LEN]);
        for cursor in ["junk", "", "AAAA", forged.as_str()] {
            let error = page(&format!("/things?cursor={cursor}")).unwrap_err();
            assert_eq!(error, PageError::BadCursor, "{cursor}");
            assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    #[test]
    fn an_edited_cursor_is_refused() {
        let cursor = next_cursor(&page("/things?limit=2").unwrap());
        let mut bytes = URL_SAFE_NO_PAD.decode(&cursor).unwrap();
        bytes[POSITION_LEN] ^= 1; // the position's last byte: offset 2 becomes 3
        let edited = URL_SAFE_NO_PAD.encode(&bytes);
        assert_eq!(
            page(&format!("/things?cursor={edited}")).unwrap_err(),
            PageError::BadCursor
        );
        let mut bytes = URL_SAFE_NO_PAD.decode(&cursor).unwrap();
        bytes[0] = 2; // another version
        let other = URL_SAFE_NO_PAD.encode(&bytes);
        assert!(page(&format!("/things?cursor={other}")).is_err());
        assert!(page(&format!("/things?cursor={cursor}&cursor={cursor}")).is_err());
        assert!(page(&format!("/things?cursor={cursor}")).is_ok());
    }

    #[test]
    fn a_cursor_from_another_query_is_refused() {
        let cursor = next_cursor(&page("/things?status=open&limit=2").unwrap());
        for uri in [
            format!("/other?status=open&cursor={cursor}"),
            format!("/things?status=closed&cursor={cursor}"),
            format!("/things?cursor={cursor}"),
            format!("/things?status=open&extra=1&cursor={cursor}"),
        ] {
            assert_eq!(page(&uri).unwrap_err(), PageError::BadCursor, "{uri}");
        }
    }

    #[test]
    fn a_cursor_signed_by_another_key_is_refused() {
        let other = CursorKey::new(&[9; 32]).unwrap();
        let theirs = CursorPage::new(&other, LIMITS, &"/things?limit=2".parse().unwrap()).unwrap();
        let cursor = next_cursor(&theirs);
        assert_eq!(
            page(&format!("/things?cursor={cursor}")).unwrap_err(),
            PageError::BadCursor
        );
    }

    #[test]
    fn a_short_key_is_refused_and_a_key_is_never_shown() {
        assert_eq!(
            CursorKey::new(&[1; 31]).unwrap_err(),
            KeyTooShort { len: 31 }
        );
        assert!(CursorKey::new(&[]).is_err());
        let shown = format!("{:?}", key());
        assert_eq!(shown, "CursorKey(..)");
        assert!(!shown.contains("secret"));
    }

    #[test]
    fn an_error_quotes_nothing_the_caller_sent() {
        let error = page("/things?limit=secret").unwrap_err();
        assert!(!error.to_string().contains("secret"));
        let error = page("/things?cursor=secret").unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn like_wildcards_are_neutralised() {
        assert_eq!(escape_like("smith"), "smith");
        assert_eq!(escape_like("100%"), r"100\%");
        assert_eq!(escape_like("a_b"), r"a\_b");
    }

    #[test]
    fn backslash_is_escaped_first() {
        assert_eq!(escape_like(r"a\%"), r"a\\\%");
    }

    #[test]
    fn bounds_fetch_one_more_row() {
        let page = page("/things?limit=2").unwrap();
        assert_eq!(page.bounds(), (3, 0));
    }

    #[test]
    fn a_cursor_past_what_sql_holds_is_refused() {
        let query = query_hash("/things", &[], "");
        for offset in [u64::MAX, u64::MAX / 2 + 1, u64::MAX / 2] {
            let cursor = key().encode(offset, &query);
            assert_eq!(
                page(&format!("/things?cursor={cursor}&limit=2")),
                Err(PageError::BadCursor),
                "{offset}"
            );
        }
    }

    #[test]
    fn a_length_is_eight_big_endian_bytes() {
        assert_eq!(length_bytes(0x0102), [0, 0, 0, 0, 0, 0, 1, 2]);
    }
}

/// Properties over generated input: a cursor holds for its own query only, a walk
/// through pages lands where the limits say, nothing a caller sends panics, and
/// escaped text holds no live wildcard.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "RCA-SCOPE-4: tests may use plain operators"
)]
#[cfg(test)]
mod properties {
    use proptest::prelude::*;

    use super::*;

    const LIMITS: CursorLimits = CursorLimits {
        default_limit: 10,
        max_limit: 100,
    };

    fn key(seed: u8) -> CursorKey {
        CursorKey::new(&[seed; 32]).unwrap()
    }

    /// The URI in a page's `Link: rel="next"`.
    fn next_uri(page: &CursorPage) -> Uri {
        let link = page.next_link().unwrap();
        let link = link.to_str().unwrap();
        link.trim_start_matches('<')
            .split('>')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn filters() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec("[a-z]{1,6}=[a-z0-9]{0,6}", 0..4).prop_filter(
            "no limit or cursor filter",
            |parts| {
                parts
                    .iter()
                    .all(|part| !part.starts_with("limit=") && !part.starts_with("cursor="))
            },
        )
    }

    fn uri(path: &str, filters: &[String], limit: u64) -> Uri {
        let mut parts = filters.to_vec();
        parts.push(format!("limit={limit}"));
        format!("{path}?{}", parts.join("&")).parse().unwrap()
    }

    proptest! {
        #[test]
        fn following_next_links_steps_by_the_limit(
            filters in filters(),
            limit in 1..=LIMITS.max_limit,
            steps in 0..12_u64,
        ) {
            let key = key(7);
            let mut page = CursorPage::new(&key, LIMITS, &uri("/things", &filters, limit)).unwrap();
            for _ in 0..steps {
                page = CursorPage::new(&key, LIMITS, &next_uri(&page)).unwrap();
            }
            prop_assert_eq!(page.limit(), limit);
            prop_assert_eq!(page.offset(), steps * limit);
        }

        #[test]
        fn a_cursor_is_refused_by_another_key_context_path_or_filter(
            filters in filters(),
            limit in 1..=LIMITS.max_limit,
            context in "[a-z]{1,8}",
            other in "[a-z]{1,8}",
        ) {
            prop_assume!(context != other);
            let first = CursorPage::new_in(&key(7), LIMITS, &uri("/things", &filters, limit), &context).unwrap();
            let next = next_uri(&first);
            prop_assert!(CursorPage::new_in(&key(7), LIMITS, &next, &context).is_ok());
            prop_assert_eq!(CursorPage::new_in(&key(8), LIMITS, &next, &context), Err(PageError::BadCursor));
            prop_assert_eq!(CursorPage::new_in(&key(7), LIMITS, &next, &other), Err(PageError::BadCursor));

            let query = next.query().unwrap();
            let moved: Uri = format!("/other?{query}").parse().unwrap();
            prop_assert_eq!(CursorPage::new_in(&key(7), LIMITS, &moved, &context), Err(PageError::BadCursor));
            let filtered: Uri = format!("/things?{query}&zzz=1").parse().unwrap();
            prop_assert_eq!(CursorPage::new_in(&key(7), LIMITS, &filtered, &context), Err(PageError::BadCursor));
        }

        #[test]
        fn the_order_of_filters_does_not_change_a_cursor(
            filters in filters(),
            limit in 1..=LIMITS.max_limit,
        ) {
            let first = CursorPage::new(&key(7), LIMITS, &uri("/things", &filters, limit)).unwrap();
            let next = next_uri(&first);
            let mut parts: Vec<&str> = next.query().unwrap().split('&').collect();
            parts.reverse();
            let reordered: Uri = format!("/things?{}", parts.join("&")).parse().unwrap();
            prop_assert_eq!(
                CursorPage::new(&key(7), LIMITS, &reordered).map(|page| page.offset()),
                Ok(limit)
            );
        }

        #[test]
        fn any_query_a_caller_sends_is_a_page_or_a_refusal(query in "[ -~]{0,80}") {
            let Ok(uri) = format!("/things?{query}").parse::<Uri>() else {
                return Ok(());
            };
            match CursorPage::new(&key(7), LIMITS, &uri) {
                Ok(page) => {
                    prop_assert!((1..=LIMITS.max_limit).contains(&page.limit()));
                    prop_assert!(page.offset() == 0 || query.contains("cursor="));
                }
                Err(error) => prop_assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY),
            }
        }

        #[test]
        fn split_keeps_at_most_the_limit_and_says_whether_more_follow(
            limit in 1..=LIMITS.max_limit,
            rows in 0..250_usize,
        ) {
            let page = CursorPage::new(&key(7), LIMITS, &uri("/things", &[], limit)).unwrap();
            let (kept, more) = page.split((0..rows).collect::<Vec<_>>());
            let limit = usize::try_from(limit).unwrap();
            prop_assert_eq!(kept.len(), rows.min(limit));
            prop_assert_eq!(more, rows > limit);
        }

        #[test]
        fn escaped_text_has_no_live_wildcard_and_unescapes_to_itself(text in "\\PC{0,40}") {
            let escaped = escape_like(&text);
            let mut unescaped = String::new();
            let mut chars = escaped.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        let escaped_char = chars.next();
                        prop_assert!(matches!(escaped_char, Some('\\' | '%' | '_')));
                        unescaped.extend(escaped_char);
                    }
                    '%' | '_' => prop_assert!(false, "a live wildcard in {escaped:?}"),
                    c => unescaped.push(c),
                }
            }
            prop_assert_eq!(unescaped, text);
        }
    }

    #[test]
    fn each_error_says_what_to_fix_without_quoting_the_request() {
        assert_eq!(
            PageError::BadLimit { max_limit: 100 }.to_string(),
            "limit must be a whole number from 1 to 100"
        );
        assert_eq!(PageError::BadCursor.to_string(), "cursor is not valid");
        let short = CursorKey::new(&[7; 8]).unwrap_err().to_string();
        assert!(short.contains("32"), "{short}");
    }
}
