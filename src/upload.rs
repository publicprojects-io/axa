//! Bounded multipart uploads, with files written to temporary files.
//!
//! Axum's [`Multipart`] reads parts as they arrive and leaves the limits to the
//! service. [`read`] holds every part to [`Limits`]: how many parts, how large each
//! text field and each file, how much text is held in memory, and how large all of it.
//! Text fields are kept in memory, and each file goes to a temporary file that is
//! deleted when its [`File`] is dropped, so a large upload costs disk, not memory.
//!
//! ```no_run
//! use axa::{Result, format, upload::{self, Limits}};
//! use axum::extract::Multipart;
//!
//! async fn avatar(multipart: Multipart) -> Result<axum::response::Response> {
//!     let limits = Limits {
//!         file_bytes: 2_000_000,
//!         files: 1,
//!         accept: Some(vec!["image/png", "image/jpeg"]),
//!         ..Limits::default()
//!     };
//!     let upload = upload::read(multipart, &limits).await?;
//!     let Some(file) = upload.files.first() else {
//!         return axa::prelude::bad_request("send one image");
//!     };
//!     // `file.path()` is the temporary file, which is deleted when `upload` drops:
//!     // copy or persist it first.
//!     format::json(serde_json::json!({ "name": file.file_name, "bytes": file.size }))
//! }
//! ```
//!
//! A part's names and content type are the caller's text, and no problem quotes them.
//! A file name is cleaned with [`format::clean_file_name`](crate::format::clean_file_name)
//! before anything sees it.

use std::path::Path;

use axum::{
    extract::{Multipart, multipart::Field},
    http::StatusCode,
};
use tokio::io::AsyncWriteExt;

use crate::{Error, Result, error::ErrorDetail, format::clean_file_name};

/// The longest field name a part may have.
const MAX_FIELD_NAME_BYTES: usize = 128;

/// How much an upload may hold. Every limit counts bytes as they arrive, so a caller
/// that sends more is refused when it passes the limit, not after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// All the parts together, 20 MB by default. The standard stack's body limit,
    /// `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, applies as well, so raise both for larger
    /// uploads.
    pub total_bytes: u64,
    /// One file, 10 MB by default.
    pub file_bytes: u64,
    /// One text field, 64 KB by default.
    pub field_bytes: usize,
    /// All the text fields together, which are held in memory, 1 MB by default.
    pub memory_bytes: usize,
    /// How many parts, 32 by default.
    pub parts: usize,
    /// How many of them may be files, 8 by default.
    pub files: usize,
    /// The media types a file may declare, as `image/png`, compared without
    /// parameters and case. `None`, the default, takes any. A file that declares none
    /// counts as `application/octet-stream`.
    pub accept: Option<Vec<&'static str>>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            total_bytes: 20_000_000,
            file_bytes: 10_000_000,
            field_bytes: 64_000,
            memory_bytes: 1_000_000,
            parts: 32,
            files: 8,
            accept: None,
        }
    }
}

/// A text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    /// The part's field name.
    pub name: String,
    /// What it held, as UTF-8.
    pub value: String,
}

/// A file, in a temporary file deleted when this drops.
#[derive(Debug)]
pub struct File {
    /// The part's field name.
    pub name: String,
    /// The name the caller gave the file, cleaned, or `download` when it gave none.
    pub file_name: String,
    /// The media type the caller declared, lower case and without parameters, or
    /// `application/octet-stream`. It is the caller's word, not a check of the bytes.
    pub content_type: String,
    /// How many bytes it holds.
    pub size: u64,
    temp: tempfile::NamedTempFile,
}

impl File {
    /// Where the temporary file is. It is deleted when this drops.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.temp.path()
    }

    /// Keep the file at `path`, on the same file system as the temporary directory, so
    /// that it is not deleted when this drops.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be moved there.
    pub fn persist(self, path: impl AsRef<Path>) -> Result<()> {
        self.temp
            .persist(path)
            .map(|_| ())
            .map_err(|error| Error::IO(error.error))
    }
}

/// An upload's parts, in the order they came.
#[derive(Debug, Default)]
pub struct Upload {
    /// The text fields.
    pub fields: Vec<Text>,
    /// The files.
    pub files: Vec<File>,
}

impl Upload {
    /// The value of the first text field named `name`.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.as_str())
    }

    /// The first file in the part named `name`.
    #[must_use]
    pub fn file(&self, name: &str) -> Option<&File> {
        self.files.iter().find(|file| file.name == name)
    }
}

/// A refusal with `status`, whose `detail` names a limit and nothing the caller sent.
fn refuse(status: StatusCode, code: &str, detail: impl Into<String>) -> Error {
    Error::CustomError(status, ErrorDetail::new(code, detail.into()))
}

fn too_large(detail: impl Into<String>) -> Error {
    refuse(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", detail)
}

/// A failure reading the body, which keeps its status, such as `413` from the body
/// limit, and says nothing of what was read.
fn unreadable(error: &axum::extract::multipart::MultipartError) -> Error {
    let status = error.status();
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        return too_large("The request body is too large");
    }
    if status.is_server_error() {
        return Error::InternalServerError;
    }
    refuse(
        StatusCode::BAD_REQUEST,
        "bad_request",
        "The request body is not valid multipart/form-data",
    )
}

/// The declared media type, lower case and without parameters.
fn media_type(field: &Field<'_>) -> String {
    field
        .content_type()
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// How many bytes have arrived so far, against `Limits::total_bytes`.
struct Tally<'a> {
    limits: &'a Limits,
    total: u64,
    memory: usize,
}

impl Tally<'_> {
    fn add(&mut self, bytes: usize) -> Result<()> {
        self.total = self.total.saturating_add(bytes as u64);
        if self.total > self.limits.total_bytes {
            return Err(too_large(format!(
                "The upload is larger than {} bytes",
                self.limits.total_bytes
            )));
        }
        Ok(())
    }
}

/// Read every part of `multipart`, holding each to `limits`.
///
/// # Errors
///
/// Answers `413`, code `payload_too_large`, for an upload past any limit, `415`, code
/// `unsupported_media_type`, for a file whose media type `accept` does not list, and
/// `400`, code `bad_request`, for a body that is not valid multipart, a part with no
/// name or one longer than 128 bytes, or a text field that is not UTF-8. A temporary
/// file that cannot be written is a `500`. What was read so far is dropped, and its
/// temporary files deleted.
pub async fn read(mut multipart: Multipart, limits: &Limits) -> Result<Upload> {
    let mut upload = Upload::default();
    let mut tally = Tally {
        limits,
        total: 0,
        memory: 0,
    };
    let mut parts = 0;
    while let Some(field) = multipart.next_field().await.map_err(|e| unreadable(&e))? {
        parts += 1;
        if parts > limits.parts {
            return Err(too_large(format!(
                "The upload has more than {} parts",
                limits.parts
            )));
        }
        let name = match field.name() {
            Some(name) if !name.is_empty() && name.len() <= MAX_FIELD_NAME_BYTES => {
                name.to_string()
            }
            _ => {
                return Err(refuse(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "Every part needs a name of at most 128 bytes",
                ));
            }
        };
        if field.file_name().is_some() {
            if upload.files.len() >= limits.files {
                return Err(too_large(format!(
                    "The upload has more than {} files",
                    limits.files
                )));
            }
            upload.files.push(read_file(field, name, &mut tally).await?);
        } else {
            upload
                .fields
                .push(read_text(field, name, &mut tally).await?);
        }
    }
    Ok(upload)
}

async fn read_text(mut field: Field<'_>, name: String, tally: &mut Tally<'_>) -> Result<Text> {
    let limits = tally.limits;
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(|e| unreadable(&e))? {
        tally.add(chunk.len())?;
        tally.memory = tally.memory.saturating_add(chunk.len());
        if bytes.len() + chunk.len() > limits.field_bytes {
            return Err(too_large(format!(
                "A text field is larger than {} bytes",
                limits.field_bytes
            )));
        }
        if tally.memory > limits.memory_bytes {
            return Err(too_large(format!(
                "The text fields are larger than {} bytes together",
                limits.memory_bytes
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = String::from_utf8(bytes).map_err(|_| {
        refuse(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "A text field is not UTF-8",
        )
    })?;
    Ok(Text { name, value })
}

async fn read_file(mut field: Field<'_>, name: String, tally: &mut Tally<'_>) -> Result<File> {
    let limits = tally.limits;
    let content_type = media_type(&field);
    if let Some(accept) = &limits.accept
        && !accept
            .iter()
            .any(|accepted| accepted.eq_ignore_ascii_case(&content_type))
    {
        return Err(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            format!("A file must be one of {}", accept.join(", ")),
        ));
    }
    let file_name = clean_file_name(field.file_name().unwrap_or_default());
    let temp = tempfile::NamedTempFile::new().map_err(Error::IO)?;
    let mut writer = tokio::fs::File::from_std(temp.reopen().map_err(Error::IO)?);
    let mut size: u64 = 0;
    while let Some(chunk) = field.chunk().await.map_err(|e| unreadable(&e))? {
        tally.add(chunk.len())?;
        size = size.saturating_add(chunk.len() as u64);
        if size > limits.file_bytes {
            return Err(too_large(format!(
                "A file is larger than {} bytes",
                limits.file_bytes
            )));
        }
        writer.write_all(&chunk).await.map_err(Error::IO)?;
    }
    writer.flush().await.map_err(Error::IO)?;
    Ok(File {
        name,
        file_name,
        content_type,
        size,
        temp,
    })
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, header::CONTENT_TYPE},
        response::IntoResponse,
        routing::post,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    const BOUNDARY: &str = "axa-boundary";

    /// A part: its name, file name, content type, and contents.
    type Part<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a [u8]);

    fn body(parts: &[Part<'_>]) -> Vec<u8> {
        let mut body = Vec::new();
        for (name, file_name, content_type, contents) in parts {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            let disposition = match file_name {
                Some(file_name) => format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\""
                ),
                None => format!("Content-Disposition: form-data; name=\"{name}\""),
            };
            body.extend_from_slice(disposition.as_bytes());
            body.extend_from_slice(b"\r\n");
            if let Some(content_type) = content_type {
                body.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
            }
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(contents);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        body
    }

    /// Send `parts` to a route that reads them under `limits`, and answers with what
    /// it read, checking each file's contents and that the file goes when it drops.
    async fn send(limits: Limits, parts: &[Part<'_>]) -> (StatusCode, Value) {
        let app = Router::new().route(
            "/",
            post(move |multipart: Multipart| async move {
                let upload = match read(multipart, &limits).await {
                    Ok(upload) => upload,
                    Err(error) => return error.into_response(),
                };
                let mut files = Vec::new();
                let mut paths = Vec::new();
                for file in &upload.files {
                    let contents = std::fs::read(file.path()).unwrap();
                    assert_eq!(contents.len() as u64, file.size);
                    paths.push(file.path().to_path_buf());
                    files.push(json!({
                        "name": file.name,
                        "file_name": file.file_name,
                        "content_type": file.content_type,
                        "contents": String::from_utf8_lossy(&contents),
                    }));
                }
                let fields: Vec<Value> = upload
                    .fields
                    .iter()
                    .map(|field| json!([field.name, field.value]))
                    .collect();
                drop(upload);
                for path in paths {
                    assert!(!path.exists(), "{}", path.display());
                }
                axum::Json(json!({ "fields": fields, "files": files })).into_response()
            }),
        );
        let request = Request::post("/")
            .header(
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body(parts)))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn fields_are_kept_and_files_spooled_to_temporary_files() {
        let parts: [Part<'_>; 3] = [
            ("title", None, None, b"Holiday"),
            (
                "photo",
                Some("../../etc/\u{202e}gpj.exe"),
                Some("Image/PNG; charset=x"),
                b"\x89PNG data",
            ),
            ("notes", Some("notes.txt"), None, b"hello"),
        ];
        let (status, body) = send(Limits::default(), &parts).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["fields"], json!([["title", "Holiday"]]));
        assert_eq!(body["files"][0]["name"], "photo");
        assert_eq!(body["files"][0]["file_name"], "gpj.exe");
        assert_eq!(body["files"][0]["content_type"], "image/png");
        assert_eq!(body["files"][1]["file_name"], "notes.txt");
        assert_eq!(body["files"][1]["content_type"], "application/octet-stream");
        assert_eq!(body["files"][1]["contents"], "hello");
    }

    #[tokio::test]
    async fn each_limit_is_a_413_that_names_the_limit_and_quotes_nothing() {
        let secret = b"secret".repeat(4);
        let cases: Vec<(Limits, Vec<Part<'_>>, &str)> = vec![
            (
                Limits {
                    file_bytes: 10,
                    ..Limits::default()
                },
                vec![("f", Some("secret.txt"), None, &secret)],
                "A file is larger than 10 bytes",
            ),
            (
                Limits {
                    field_bytes: 10,
                    ..Limits::default()
                },
                vec![("secret", None, None, &secret)],
                "A text field is larger than 10 bytes",
            ),
            (
                Limits {
                    memory_bytes: 30,
                    ..Limits::default()
                },
                vec![("a", None, None, &secret), ("b", None, None, &secret)],
                "The text fields are larger than 30 bytes together",
            ),
            (
                Limits {
                    total_bytes: 30,
                    ..Limits::default()
                },
                vec![("a", None, None, &secret), ("f", Some("f"), None, &secret)],
                "The upload is larger than 30 bytes",
            ),
            (
                Limits {
                    parts: 1,
                    ..Limits::default()
                },
                vec![("a", None, None, b"1"), ("b", None, None, b"2")],
                "The upload has more than 1 parts",
            ),
            (
                Limits {
                    files: 1,
                    ..Limits::default()
                },
                vec![("a", Some("a"), None, b"1"), ("b", Some("b"), None, b"2")],
                "The upload has more than 1 files",
            ),
        ];
        for (limits, parts, detail) in cases {
            let (status, body) = send(limits, &parts).await;
            assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
            assert_eq!(body["code"], "payload_too_large");
            assert_eq!(body["detail"], detail);
            assert!(!body.to_string().contains("secret"), "{body}");
        }
    }

    #[tokio::test]
    async fn a_file_of_a_type_not_accepted_is_a_415() {
        let limits = Limits {
            accept: Some(vec!["image/png"]),
            ..Limits::default()
        };
        let (status, body) = send(
            limits.clone(),
            &[("f", Some("a.png"), Some("IMAGE/png"), b"x")],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        for content_type in [Some("text/html; secret"), None] {
            let (status, body) =
                send(limits.clone(), &[("f", Some("a.png"), content_type, b"x")]).await;
            assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
            assert_eq!(body["code"], "unsupported_media_type");
            assert_eq!(body["detail"], "A file must be one of image/png");
            assert!(!body.to_string().contains("secret"), "{body}");
        }
    }

    #[tokio::test]
    async fn a_part_with_no_usable_name_or_text_that_is_not_utf8_is_a_400() {
        let long = "n".repeat(129);
        for parts in [
            vec![("", None, None, &b"x"[..])],
            vec![(long.as_str(), None, None, &b"x"[..])],
            vec![("a", None, None, &b"\xff\xfe"[..])],
        ] {
            let (status, body) = send(Limits::default(), &parts).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["code"], "bad_request");
            assert!(!body.to_string().contains(&long), "{body}");
        }
    }

    #[tokio::test]
    async fn the_body_limit_still_applies_and_a_broken_body_is_a_400() {
        let app = |limit| {
            Router::new()
                .route(
                    "/",
                    post(|multipart: Multipart| async move {
                        read(multipart, &Limits::default())
                            .await
                            .map(|_| "ok")
                            .into_response()
                    }),
                )
                .layer(axum::extract::DefaultBodyLimit::max(limit))
        };
        let request = |body: Vec<u8>| {
            Request::post("/")
                .header(
                    CONTENT_TYPE,
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(body))
                .unwrap()
        };
        let big = body(&[("f", Some("f"), None, &[b'x'; 2_000])]);
        let response = app(1_000).oneshot(request(big)).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let broken =
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nno end");
        let response = app(1_000)
            .oneshot(request(broken.into_bytes()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
