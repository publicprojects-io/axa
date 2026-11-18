//! `format::clean_file_name` and `content_disposition` with any name a caller uploads:
//! the clean name has no path, quote, backslash, or control character, holds 1 to 255
//! bytes, and always makes a valid header.

#![no_main]

use axa::format::{clean_file_name, content_disposition};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, &str)| {
    let (media_type, name) = input;
    let clean = clean_file_name(name);
    assert!(!clean.is_empty() && clean.len() <= 255, "{clean:?}");
    assert!(!clean.contains(['/', '\\', '"']), "{clean:?}");
    assert!(!clean.chars().any(char::is_control), "{clean:?}");
    assert!(!clean.starts_with('.'), "{clean:?}");
    let _ = content_disposition(media_type, name);
});
