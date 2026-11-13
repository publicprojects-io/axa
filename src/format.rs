//! Small response helpers for handlers that return [`Result<Response>`].

use std::fmt::Write as _;

use axum::{
    Json,
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Serialize;

use crate::Result;

/// A `200` with a JSON body.
///
/// # Errors
///
/// Infallible in practice; fallible to match how handlers compose it.
pub fn json<T: Serialize>(value: T) -> Result<Response> {
    Ok(Json(value).into_response())
}

/// A `200` with an empty JSON object.
///
/// # Errors
///
/// Infallible in practice.
pub fn empty_json() -> Result<Response> {
    json(serde_json::json!({}))
}

/// A `200` with no body.
///
/// # Errors
///
/// Infallible in practice.
pub fn empty() -> Result<Response> {
    Ok(StatusCode::OK.into_response())
}

/// A `200` with a plain-text body.
///
/// # Errors
///
/// Infallible in practice.
pub fn text(body: &str) -> Result<Response> {
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body.to_string(),
    )
        .into_response())
}

/// A `200` with an HTML body.
///
/// # Errors
///
/// Infallible in practice.
pub fn html(body: &str) -> Result<Response> {
    Ok(Html(body.to_string()).into_response())
}

/// A `303` redirect.
///
/// # Errors
///
/// Infallible in practice.
pub fn redirect(to: &str) -> Result<Response> {
    Ok(Redirect::to(to).into_response())
}

/// The media types a browser may show in place: none of them can run script. Anything
/// else, such as HTML, SVG, XML, or PDF, is downloaded instead.
const INLINE_MEDIA_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "audio/mpeg",
    "audio/ogg",
    "audio/wav",
    "audio/webm",
    "audio/flac",
    "video/mp4",
    "video/webm",
    "video/ogg",
    "text/plain",
];

/// The longest cleaned file name, in bytes, which most file systems allow.
const MAX_FILE_NAME_BYTES: usize = 255;

/// The name a cleaned file name falls back to when nothing is left of it.
const FALLBACK_FILE_NAME: &str = "download";

/// A `200` that sends `body` as a file named `file_name`, with a safe
/// `content-disposition` from [`content_disposition`].
///
/// A `media_type` that is not a valid header value is sent as
/// `application/octet-stream`, and so as an attachment.
///
/// ```
/// let response = axa::format::download(b"a,b\n".to_vec(), "text/csv", "../report.csv").unwrap();
/// assert_eq!(
///     response.headers()["content-disposition"],
///     r#"attachment; filename="report.csv""#
/// );
/// ```
///
/// # Errors
///
/// Infallible in practice.
pub fn download(body: impl Into<Body>, media_type: &str, file_name: &str) -> Result<Response> {
    let (media_type, content_type) = match HeaderValue::from_str(media_type) {
        Ok(value) => (media_type, value),
        Err(_) => (
            "application/octet-stream",
            HeaderValue::from_static("application/octet-stream"),
        ),
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_DISPOSITION,
                content_disposition(media_type, file_name),
            ),
        ],
        body.into(),
    )
        .into_response())
}

/// A `content-disposition` for a file of `media_type` named `file_name`: `inline` only
/// for a media type that cannot run script, `attachment` otherwise, with the name
/// cleaned by [`clean_file_name`]. A name that is not all ASCII is sent as `filename*`
/// too, in UTF-8, with an ASCII `filename` for older clients (RFC 6266).
#[must_use]
pub fn content_disposition(media_type: &str, file_name: &str) -> HeaderValue {
    let essence = media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let disposition = if INLINE_MEDIA_TYPES.contains(&essence.as_str()) {
        "inline"
    } else {
        "attachment"
    };
    let name = clean_file_name(file_name);
    let ascii: String = name
        .chars()
        .map(|c| {
            if (c.is_ascii_graphic() && !matches!(c, '"' | '\\' | '%')) || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut value = format!("{disposition}; filename=\"{ascii}\"");
    if ascii != name {
        value.push_str("; filename*=UTF-8''");
        for byte in name.bytes() {
            if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
                value.push(char::from(byte));
            } else {
                let _ = write!(value, "%{byte:02X}");
            }
        }
    }
    // Every character is visible ASCII or a space, so this cannot fail.
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

/// `name` made safe to send or save: its last path component, without control or
/// bidirectional characters, quotes, or backslashes, without leading dots or trailing dots and
/// spaces, at most 255 bytes, keeping the extension, and `download` when nothing is
/// left.
#[must_use]
pub fn clean_file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let kept: String = last
        .chars()
        .filter(|&c| !c.is_control() && !is_bidi_control(c) && !matches!(c, '"' | '\\'))
        .collect();
    let trimmed = kept
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    let cleaned = shorten(trimmed, MAX_FILE_NAME_BYTES);
    if cleaned.is_empty() {
        FALLBACK_FILE_NAME.to_string()
    } else {
        cleaned
    }
}

/// A character that reorders the text around it, which can make `evil\u{202E}fdp.exe`
/// show as `evilexe.pdf`.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// `name` in at most `max` bytes, cut on a character boundary, keeping a short
/// extension.
fn shorten(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    let extension = name
        .rfind('.')
        .map(|dot| &name[dot..])
        .filter(|extension| extension.len() <= 16)
        .unwrap_or_default();
    let mut stem_end = max - extension.len();
    while !name.is_char_boundary(stem_end) {
        stem_end -= 1;
    }
    let stem = name[..stem_end].trim_end_matches(['.', ' ']);
    format!("{stem}{extension}")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn json_serializes_with_a_json_content_type() {
        let response = json(serde_json::json!({ "a": 1 })).unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(body_text(response).await, r#"{"a":1}"#);
    }

    #[tokio::test]
    async fn empty_json_is_an_empty_object() {
        assert_eq!(body_text(empty_json().unwrap()).await, "{}");
    }

    #[tokio::test]
    async fn empty_has_no_body() {
        let response = empty().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_text(response).await, "");
    }

    #[tokio::test]
    async fn text_and_html_set_their_content_types() {
        let response = text("hi").unwrap();
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/plain")
        );
        let response = html("<p>hi</p>").unwrap();
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
    }

    #[test]
    fn redirect_points_at_the_target() {
        let response = redirect("/next").unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/next");
    }

    fn disposition(media_type: &str, file_name: &str) -> String {
        content_disposition(media_type, file_name)
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn only_media_types_that_cannot_run_script_are_inline() {
        assert_eq!(
            disposition("image/png", "cat.png"),
            r#"inline; filename="cat.png""#
        );
        assert_eq!(
            disposition("Text/Plain; charset=utf-8", "notes.txt"),
            r#"inline; filename="notes.txt""#
        );
        for media_type in [
            "text/html",
            "image/svg+xml",
            "application/pdf",
            "application/xhtml+xml",
            "text/xml",
            "application/octet-stream",
            "",
        ] {
            assert!(
                disposition(media_type, "f").starts_with("attachment;"),
                "{media_type}"
            );
        }
    }

    #[test]
    fn a_file_name_loses_its_path_control_characters_and_quotes() {
        assert_eq!(clean_file_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_file_name("C:\\Users\\me\\report.csv"), "report.csv");
        assert_eq!(clean_file_name("a\"b\r\nc.txt"), "abc.txt");
        assert_eq!(clean_file_name(".htaccess"), "htaccess");
        assert_eq!(clean_file_name("name. . "), "name");
        assert_eq!(clean_file_name(""), "download");
        assert_eq!(clean_file_name("../.."), "download");
        assert_eq!(clean_file_name("dir/"), "download");
        assert_eq!(clean_file_name("evil\u{202E}fdp.exe"), "evilfdp.exe");
    }

    #[test]
    fn a_long_file_name_is_cut_on_a_character_boundary_keeping_its_extension() {
        let long = format!("{}.pdf", "é".repeat(200));
        let cleaned = clean_file_name(&long);
        assert!(cleaned.len() <= 255);
        assert_eq!(&cleaned[cleaned.len() - 4..], ".pdf");
        assert!(cleaned.starts_with('é'));
    }

    #[test]
    fn a_name_that_is_not_ascii_is_sent_in_utf8_beside_an_ascii_fallback() {
        assert_eq!(
            disposition("application/pdf", "résumé 100%.pdf"),
            "attachment; filename=\"r_sum_ 100_.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20100%25.pdf"
        );
    }

    #[tokio::test]
    async fn download_sets_the_type_and_disposition() {
        let response = download("x,y", "text/csv", "../q3.csv").unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/csv");
        assert_eq!(
            response.headers()[header::CONTENT_DISPOSITION],
            r#"attachment; filename="q3.csv""#
        );
        assert_eq!(body_text(response).await, "x,y");
        let response = download("x", "text/html\n", "a.html").unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert!(
            response.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .starts_with("attachment;")
        );
    }
}
