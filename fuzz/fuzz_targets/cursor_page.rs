//! `CursorPage::new` with any query a caller sends: a page within the limits, or a
//! refusal, and only a cursor this key signed moves the offset.

#![no_main]

use axa_cursor_pagination::{CursorKey, CursorLimits, CursorPage};
use axum::http::Uri;
use libfuzzer_sys::fuzz_target;

const LIMITS: CursorLimits = CursorLimits {
    default_limit: 10,
    max_limit: 100,
};

fuzz_target!(|query: &str| {
    let Ok(uri) = format!("/things?{query}").parse::<Uri>() else {
        return;
    };
    let key = CursorKey::new(&[7; 32]).unwrap();
    if let Ok(page) = CursorPage::new(&key, LIMITS, &uri) {
        assert!((1..=LIMITS.max_limit).contains(&page.limit()));
        assert!(page.offset() == 0 || query.contains("cursor="));
        let _ = page.next_link();
    }
});
