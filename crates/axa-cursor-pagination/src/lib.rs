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
        if bytes.len() != CURSOR_LEN || bytes[0] != VERSION {
            return None;
        }
        let position = u64::from_be_bytes(bytes[1..=POSITION_LEN].try_into().ok()?);
        // Constant time, so a caller cannot learn the tag a byte at a time.
        self.tag(position, query)
            .verify_slice(&bytes[1 + POSITION_LEN..])
            .ok()?;
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
    next: String,
}

/// The query a cursor belongs to: the path and the parameters other than
/// `cursor` and `limit`, sorted so that their order does not matter.
fn query_hash(path: &str, kept: &[&str]) -> [u8; 32] {
    let mut sorted = kept.to_vec();
    sorted.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(path.as_bytes());
    for part in sorted {
        hash.update([0]);
        hash.update(part.as_bytes());
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
        let query = query_hash(uri.path(), &kept);
        let offset = match named("cursor").as_slice() {
            [] => 0,
            [cursor] => key.decode(cursor, &query).ok_or(PageError::BadCursor)?,
            _ => return Err(PageError::BadCursor),
        };

        let next_cursor = key.encode(offset.saturating_add(limit), &query);
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
    /// whether more rows follow. A value too large for `i64` saturates, so an offset
    /// past every row is an empty page, not a failed query.
    #[must_use]
    pub fn bounds(&self) -> (i64, i64) {
        (
            i64::try_from(self.limit.saturating_add(1)).unwrap_or(i64::MAX),
            i64::try_from(self.offset).unwrap_or(i64::MAX),
        )
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

#[cfg(test)]
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
    fn bounds_fetch_one_more_row_and_saturate() {
        let page = page("/things?limit=2").unwrap();
        assert_eq!(page.bounds(), (3, 0));
        let far = CursorPage {
            limit: u64::MAX,
            offset: u64::MAX,
            next: String::new(),
        };
        assert_eq!(far.bounds(), (i64::MAX, i64::MAX));
        assert_eq!(far.split(vec![1]), (vec![1], false));
    }
}
