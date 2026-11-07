//! Extractors that answer a bad request as every other error is answered.
//!
//! Axum's own `Json`, `Path`, and `Query` reject a bad request with plain text, and
//! that text can quote what the caller sent. These take their place, and reject with
//! [`Error`], so a caller gets a problem and the text quotes nothing.
//!
//! ```
//! use axa::{Result, extract::{Json, Path}};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize, Serialize)]
//! struct Folder {
//!     name: String,
//! }
//!
//! async fn rename(Path(id): Path<u32>, Json(folder): Json<Folder>) -> Result<Json<Folder>> {
//!     # let _ = id;
//!     Ok(Json(folder))
//! }
//! ```
//!
//! The prelude keeps Axum's, because changing a name it exports would change what
//! existing handlers mean. A service opts in by importing these.

use axum::{
    extract::{FromRequest, FromRequestParts},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::Error;

/// A JSON request or response body.
#[derive(Debug, Clone, Copy, Default, FromRequest)]
#[from_request(via(axum::Json), rejection(Error))]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Values from the URL's path.
#[derive(Debug, Clone, Copy, Default, FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(Error))]
pub struct Path<T>(pub T);

/// Values from the URL's query string.
#[derive(Debug, Clone, Copy, Default, FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(Error))]
pub struct Query<T>(pub T);

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
        routing::{get, post},
    };
    use serde::Deserialize;
    use tower::ServiceExt;

    use super::*;

    #[derive(Deserialize, Serialize)]
    struct Folder {
        name: String,
    }

    #[derive(Deserialize)]
    struct Page {
        size: u8,
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/folders",
                post(|Json(folder): Json<Folder>| async move { Json(folder) }),
            )
            .route(
                "/folders/{id}",
                get(|Path(id): Path<u32>| async move { id.to_string() }),
            )
            .route(
                "/list",
                get(|Query(page): Query<Page>| async move { page.size.to_string() }),
            )
    }

    async fn call(request: Request<Body>) -> (StatusCode, String, String) {
        let response = app().oneshot(request).await.unwrap();
        let status = response.status();
        let kind = response.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_string();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, kind, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn post_json(body: &str) -> Request<Body> {
        Request::post("/folders")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get_uri(uri: &str) -> Request<Body> {
        Request::get(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn a_good_request_passes_through() {
        let (status, _, body) = call(post_json(r#"{"name":"a"}"#)).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, r#"{"name":"a"}"#));
        assert_eq!(call(get_uri("/folders/7")).await.2, "7");
        assert_eq!(call(get_uri("/list?size=3")).await.2, "3");
    }

    #[tokio::test]
    async fn every_rejection_is_a_problem_and_quotes_nothing() {
        let cases = [
            (post_json("{secret"), StatusCode::BAD_REQUEST),
            (
                post_json(r#"{"name":["secret"]}"#),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                Request::post("/folders")
                    .body(Body::from("secret"))
                    .unwrap(),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (get_uri("/folders/secret"), StatusCode::BAD_REQUEST),
            (get_uri("/list?size=secret"), StatusCode::BAD_REQUEST),
        ];
        for (request, expected) in cases {
            let (status, kind, body) = call(request).await;
            assert_eq!(status, expected, "{body}");
            assert_eq!(kind, crate::problem::PROBLEM_JSON, "{body}");
            assert!(body.contains("detail"), "{body}");
            assert!(!body.contains("secret"), "{body}");
        }
    }
}
