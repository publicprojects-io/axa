//! A service that takes file uploads and serves them back, with Axa's `upload` feature
//! and `format::download`.
//!
//! `POST /api/files` takes one file, of at most 1 MB, of a type it accepts, and answers
//! its id. `GET /api/files/{id}` sends it back, shown in place only when it cannot run
//! script, and downloaded otherwise. Files are kept in memory, so they go when it stops.
//!
//! ```sh
//! cargo run
//! curl -F 'file=@notes.txt;type=text/plain' localhost:1241/api/files
//! curl -i localhost:1241/api/files/1
//! ```

use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use axa::{
    Environment, Result,
    config::{Config, Defaults, MB},
    extract::Path,
    logger,
    middleware::body_limit,
    prelude::*,
    server,
    upload::{self, Limits},
};
use axum::extract::Multipart;

/// The largest file it takes.
const MAX_FILE_BYTES: u64 = 1_000_000;

/// The types it takes. HTML is among them, to show that it is downloaded, never shown.
const ACCEPTED: [&str; 4] = ["text/plain", "image/png", "application/pdf", "text/html"];

/// A file, as it was sent.
struct Stored {
    name: String,
    media_type: String,
    bytes: Vec<u8>,
}

/// The files, kept in `AppContext::shared_store`.
#[derive(Default)]
struct Files {
    next: AtomicU64,
    files: Mutex<HashMap<u64, Stored>>,
}

/// The limits for one upload: one file, and a field or two beside it.
fn limits() -> Limits {
    Limits {
        total_bytes: MAX_FILE_BYTES + 64_000,
        file_bytes: MAX_FILE_BYTES,
        files: 1,
        parts: 4,
        accept: Some(ACCEPTED.to_vec()),
        ..Limits::default()
    }
}

async fn create(State(ctx): State<AppContext>, multipart: Multipart) -> Result<Response> {
    let upload = upload::read(multipart, &limits()).await?;
    let Some(file) = upload.file("file") else {
        return bad_request("send the file in a part named file");
    };
    // The temporary file goes when `upload` drops, so read it, or `persist` it, first.
    let bytes = tokio::fs::read(file.path()).await?;
    let Some(files) = ctx.shared_store.get_ref::<Files>() else {
        return Err(axa::Error::InternalServerError);
    };
    let id = files.next.fetch_add(1, Ordering::Relaxed) + 1;
    let stored = Stored {
        name: file.file_name.clone(),
        media_type: file.content_type.clone(),
        bytes,
    };
    let answer = serde_json::json!({ "id": id, "name": stored.name, "bytes": file.size });
    files
        .files
        .lock()
        .map_err(|_| axa::Error::InternalServerError)?
        .insert(id, stored);
    format::json(answer)
}

async fn show(State(ctx): State<AppContext>, Path(id): Path<u64>) -> Result<Response> {
    let Some(files) = ctx.shared_store.get_ref::<Files>() else {
        return Err(axa::Error::InternalServerError);
    };
    let files = files
        .files
        .lock()
        .map_err(|_| axa::Error::InternalServerError)?;
    let Some(stored) = files.get(&id) else {
        return not_found();
    };
    format::download(stored.bytes.clone(), &stored.media_type, &stored.name)
}

/// The service's routes, with the health routes Axa adds. The upload route's body limit
/// is raised to fit its largest upload, and no other route's.
fn routes() -> AppRoutes {
    let upload_limit = usize::try_from(limits().total_bytes).unwrap_or(MB);
    AppRoutes::with_default_routes().add_route(
        Routes::new()
            .prefix("/api/files")
            .add("/", body_limit(post(create), upload_limit))
            .add("/{id}", get(show)),
    )
}

/// The settings for `environment`, with no database.
fn config(environment: &Environment) -> Result<Config> {
    let defaults = Defaults {
        database: "",
        ..Defaults::default()
    };
    Config::from_env(environment, &defaults)
}

/// The app for `environment`.
fn app(environment: Environment, config: Config) -> Result<axum::Router> {
    let ctx = AppContext::new(environment, (), config);
    ctx.shared_store.insert(Files::default());
    routes().into_app(ctx)
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let config = config(&environment)?;
    logger::init("file_drop", &config.logger);
    let server_config = config.server.clone();
    let router = app(environment, config)?;
    server::serve(router, &server_config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    const BOUNDARY: &str = "file-drop-boundary";

    fn test_app() -> axum::Router {
        app(Environment::Test, config(&Environment::Test).unwrap()).unwrap()
    }

    /// A multipart body with one file part.
    fn upload(name: &str, media_type: &str, contents: &[u8]) -> Request<Body> {
        let mut body = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
             Content-Type: {media_type}\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(contents);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        Request::post("/api/files")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body))
            .unwrap()
    }

    async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, String, String) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let disposition = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .map(|value| value.to_str().unwrap().to_string())
            .unwrap_or_default();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            disposition,
            String::from_utf8_lossy(&bytes).to_string(),
        )
    }

    fn get_file(id: u64) -> Request<Body> {
        Request::get(format!("/api/files/{id}"))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn a_file_is_kept_and_sent_back_under_a_clean_name() {
        let app = test_app();
        let (status, _, answer) =
            send(&app, upload("../../notes.txt", "text/plain", b"milk")).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert!(answer.contains(r#""name":"notes.txt""#), "{answer}");
        let (status, disposition, body) = send(&app, get_file(1)).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "milk"));
        assert_eq!(disposition, r#"inline; filename="notes.txt""#);
    }

    #[tokio::test]
    async fn html_is_downloaded_never_shown() {
        let app = test_app();
        let page = b"<script>alert(1)</script>";
        send(&app, upload("page.html", "text/html", page)).await;
        let (_, disposition, _) = send(&app, get_file(1)).await;
        assert_eq!(disposition, r#"attachment; filename="page.html""#);
    }

    #[tokio::test]
    async fn a_file_too_large_or_of_another_type_is_refused() {
        let app = test_app();
        let big = vec![b'x'; 1_000_001];
        let (status, _, answer) = send(&app, upload("big.txt", "text/plain", &big)).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{answer}");
        let (status, _, answer) =
            send(&app, upload("tool.exe", "application/x-msdownload", b"MZ")).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{answer}");
        assert!(answer.contains("unsupported_media_type"), "{answer}");
        assert_eq!(send(&app, get_file(1)).await.0, StatusCode::NOT_FOUND);
    }
}
