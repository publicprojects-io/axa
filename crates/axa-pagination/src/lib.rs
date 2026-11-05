//! Offset pagination, count headers, and `LIKE` escaping.
//!
//! A collection read takes `?limit=` and `?offset=`, and reports its total in
//! response headers, so the body stays a bare array.
//!
//! - `limit` is clamped to a maximum rather than rejected.
//! - Zero or unparsable values fall back to the default.
//! - An `offset` past the maximum is refused, because making the database
//!   read and discard arbitrarily many rows is a cheap denial of service.
//!
//! ```
//! use axa_pagination::{Limits, PageParams};
//!
//! let limits = Limits { default_limit: 100, max_limit: 500, max_offset: 10_000 };
//! let params = PageParams { limit: Some("9999".into()), offset: Some("50".into()) };
//! let page = params.resolve(limits).unwrap();
//! assert_eq!((page.limit, page.offset), (500, 50));
//! ```

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;

/// Header carrying the rows matching the query, ignoring the page.
pub const X_TOTAL_COUNT: HeaderName = HeaderName::from_static("x-total-count");
/// Header carrying the limit actually applied.
pub const X_LIMIT: HeaderName = HeaderName::from_static("x-limit");
/// Header carrying the offset actually applied.
pub const X_OFFSET: HeaderName = HeaderName::from_static("x-offset");

/// The bounds of one endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The limit when none is given.
    pub default_limit: u64,
    /// The largest limit applied.
    pub max_limit: u64,
    /// The largest offset accepted.
    pub max_offset: u64,
}

/// Raw query parameters. These are strings, so a bad value falls back to the
/// default rather than rejecting the whole query.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PageParams {
    /// Requested page size.
    pub limit: Option<String>,
    /// Requested rows to skip.
    pub offset: Option<String>,
}

/// A resolved page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// Rows to return.
    pub limit: u64,
    /// Rows to skip.
    pub offset: u64,
}

/// An offset past the endpoint's maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetTooLarge {
    /// The offset requested.
    pub requested: u64,
    /// The largest accepted.
    pub max_offset: u64,
}

impl std::fmt::Display for OffsetTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "offset {} exceeds the maximum of {}",
            self.requested, self.max_offset
        )
    }
}

impl std::error::Error for OffsetTooLarge {}

impl PageParams {
    /// Apply `limits`.
    ///
    /// # Errors
    ///
    /// Fails when the offset is past the maximum.
    pub fn resolve(&self, limits: Limits) -> Result<Page, OffsetTooLarge> {
        let parse = |value: &Option<String>| value.as_deref().and_then(|v| v.parse::<u64>().ok());
        let limit = parse(&self.limit)
            .filter(|limit| *limit > 0)
            .unwrap_or(limits.default_limit)
            .min(limits.max_limit);
        let offset = parse(&self.offset).unwrap_or(0);
        if offset > limits.max_offset {
            return Err(OffsetTooLarge {
                requested: offset,
                max_offset: limits.max_offset,
            });
        }
        Ok(Page { limit, offset })
    }
}

impl Page {
    /// The three count headers for a query matching `total` rows.
    #[must_use]
    pub fn headers(&self, total: u64) -> HeaderMap {
        let mut headers = HeaderMap::with_capacity(3);
        headers.insert(X_TOTAL_COUNT, HeaderValue::from(total));
        headers.insert(X_LIMIT, HeaderValue::from(self.limit));
        headers.insert(X_OFFSET, HeaderValue::from(self.offset));
        headers
    }
}

/// Escape `%`, `_`, and `\` so user text matches literally in a `LIKE`
/// pattern. Bind the result as a parameter with `ESCAPE '\'`.
///
/// ```
/// assert_eq!(axa_pagination::escape_like("100%_a"), r"100\%\_a");
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

    const LIMITS: Limits = Limits {
        default_limit: 100,
        max_limit: 500,
        max_offset: 1_000,
    };

    fn resolve(limit: Option<&str>, offset: Option<&str>) -> Result<Page, OffsetTooLarge> {
        PageParams {
            limit: limit.map(str::to_string),
            offset: offset.map(str::to_string),
        }
        .resolve(LIMITS)
    }

    #[test]
    fn omitted_values_use_the_defaults() {
        let page = resolve(None, None).unwrap();
        assert_eq!((page.limit, page.offset), (100, 0));
    }

    #[test]
    fn a_large_limit_is_clamped_not_rejected() {
        assert_eq!(resolve(Some("100000"), None).unwrap().limit, 500);
    }

    #[test]
    fn zero_and_unparsable_values_use_the_defaults() {
        assert_eq!(resolve(Some("0"), None).unwrap().limit, 100);
        let page = resolve(Some("many"), Some("-1")).unwrap();
        assert_eq!((page.limit, page.offset), (100, 0));
    }

    #[test]
    fn an_offset_at_the_maximum_is_allowed() {
        assert_eq!(resolve(None, Some("1000")).unwrap().offset, 1_000);
    }

    #[test]
    fn an_offset_past_the_maximum_is_refused() {
        let error = resolve(None, Some("1001")).unwrap_err();
        assert_eq!(error.requested, 1_001);
        assert!(error.to_string().contains("1000"));
    }

    #[test]
    fn query_strings_deserialize() {
        let params: PageParams = serde_json::from_str(r#"{"limit":"5"}"#).unwrap();
        assert_eq!(params.limit.as_deref(), Some("5"));
        assert_eq!(params.offset, None);
    }

    #[test]
    fn headers_report_what_was_applied() {
        let page = Page {
            limit: 25,
            offset: 50,
        };
        let headers = page.headers(431);
        assert_eq!(headers[X_TOTAL_COUNT], "431");
        assert_eq!(headers[X_LIMIT], "25");
        assert_eq!(headers[X_OFFSET], "50");
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
}
