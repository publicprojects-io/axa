//! `conditional::if_match_holds` with any `If-Match` headers: an answer, never a panic,
//! and none at all when no header was sent.

#![no_main]

use axa::conditional::{ETag, if_match_holds};
use axum::http::{HeaderMap, HeaderValue, header};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|values: Vec<Vec<u8>>| {
    let current = ETag::strong("v1").unwrap();
    let mut headers = HeaderMap::new();
    for value in values.iter().take(8) {
        if let Ok(value) = HeaderValue::from_bytes(value) {
            headers.append(header::IF_MATCH, value);
        }
    }
    let held = if_match_holds(&headers, &current);
    assert_eq!(held.is_none(), headers.is_empty());
});
