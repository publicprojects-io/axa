//! Tracking a request through the code it runs, the tasks it spawns, and its peers.

use std::{future::Future, sync::Arc};

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use tokio::task::JoinHandle;
use tracing::Instrument;
use uuid::Uuid;

/// The header carrying a request's id.
pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The longest id kept from a caller, as `axa` keeps.
const MAX_REQUEST_ID_LEN: usize = 255;

tokio::task_local! {
    static REQUEST_ID: Arc<str>;
}

/// Keep a caller's id only after removing unexpected characters, as `axa` does, or
/// mint one when nothing usable remains. Behind `axa`'s own middleware this changes
/// nothing, and on a router without it the header is still only caller input.
fn sanitize(headers: &HeaderMap) -> Arc<str> {
    headers
        .get(&X_REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '@'))
                .take(MAX_REQUEST_ID_LEN)
                .collect::<String>()
        })
        .filter(|id| !id.is_empty())
        .map_or_else(|| Uuid::new_v4().to_string().into(), Into::into)
}

/// Middleware that makes the request id readable anywhere the request runs, through
/// [`current_request_id`], without passing it down. Layer it inside `axa`'s own, by
/// adding it to the router before `axa::middleware::apply`.
pub async fn track(request: Request, next: Next) -> Response {
    let id = sanitize(request.headers());
    REQUEST_ID.scope(id, next.run(request)).await
}

/// The id of the request this code is serving. `None` outside [`track`], and in a
/// task from `tokio::spawn`. Use [`spawn`] to keep it there.
#[must_use]
pub fn current_request_id() -> Option<String> {
    REQUEST_ID
        .try_with(|id| Some(id.to_string()))
        .ok()
        .flatten()
}

/// Add the current request's id to a call to a peer service, so that both services'
/// logs can be followed from one id. Outside a request this changes nothing, and a
/// value already there stays.
pub fn propagate(headers: &mut HeaderMap) {
    if headers.contains_key(&X_REQUEST_ID) {
        return;
    }
    if let Some(value) = current_request_id().and_then(|id| HeaderValue::from_str(&id).ok()) {
        headers.insert(X_REQUEST_ID, value);
    }
}

/// `tokio::spawn`, keeping the current span and request id in the new task.
///
/// A plain `tokio::spawn` loses both, so what the task logs cannot be tied to the
/// request that started it.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let future = future.in_current_span();
    match REQUEST_ID.try_with(Arc::clone) {
        Ok(id) => tokio::spawn(REQUEST_ID.scope(id, future)),
        Err(_) => tokio::spawn(future),
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router, body::Body, http::Request as HttpRequest, middleware::from_fn, routing::get,
    };
    use tower::ServiceExt;

    use super::*;

    async fn id_seen_by_a_handler(header: Option<&str>) -> String {
        async fn handler() -> String {
            current_request_id().unwrap_or_default()
        }
        let app = Router::new().route("/", get(handler)).layer(from_fn(track));
        let mut request = HttpRequest::builder().uri("/");
        if let Some(header) = header {
            request = request.header("x-request-id", header);
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn a_handler_can_read_the_request_id() {
        assert_eq!(id_seen_by_a_handler(Some("abc-123")).await, "abc-123");
    }

    #[tokio::test]
    async fn a_callers_id_is_cleaned_as_axa_cleans_it() {
        assert_eq!(
            id_seen_by_a_handler(Some("abc<>123_x@y")).await,
            "abc123_x@y"
        );
        let long = "a".repeat(400);
        assert_eq!(id_seen_by_a_handler(Some(&long)).await.len(), 255);
    }

    #[tokio::test]
    async fn an_id_is_minted_when_nothing_usable_remains() {
        assert_eq!(id_seen_by_a_handler(None).await.len(), 36);
        assert_eq!(id_seen_by_a_handler(Some("<>")).await.len(), 36);
    }

    #[test]
    fn there_is_no_request_id_outside_a_request() {
        assert_eq!(current_request_id(), None);
        let mut headers = HeaderMap::new();
        propagate(&mut headers);
        assert!(headers.is_empty());
    }

    #[tokio::test]
    async fn propagate_sends_the_id_and_keeps_one_already_set() {
        let id: Arc<str> = "abc".into();
        REQUEST_ID
            .scope(id, async {
                let mut headers = HeaderMap::new();
                propagate(&mut headers);
                assert_eq!(headers.get(&X_REQUEST_ID).unwrap(), "abc");

                let mut headers = HeaderMap::new();
                headers.insert(X_REQUEST_ID, HeaderValue::from_static("mine"));
                propagate(&mut headers);
                assert_eq!(headers.get(&X_REQUEST_ID).unwrap(), "mine");
            })
            .await;
    }

    #[tokio::test]
    async fn a_spawned_task_keeps_the_request_id() {
        let id: Arc<str> = "abc".into();
        let (kept, lost) = REQUEST_ID
            .scope(id, async {
                let kept = spawn(async { current_request_id() }).await.unwrap();
                let lost = tokio::spawn(async { current_request_id() }).await.unwrap();
                (kept, lost)
            })
            .await;
        assert_eq!(kept.as_deref(), Some("abc"));
        assert_eq!(lost, None);
    }

    #[tokio::test]
    async fn spawn_works_outside_a_request() {
        assert_eq!(spawn(async { 1 }).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn a_spawned_task_keeps_the_span() {
        use tracing_subscriber::layer::SubscriberExt;

        let subscriber =
            tracing_subscriber::registry().with(crate::test_support::Lines::default().layer());
        let _guard = tracing::subscriber::set_default(subscriber);
        let span = tracing::info_span!("parent");
        let id = span.id();
        let seen = async {
            spawn(async { tracing::Span::current().id() })
                .await
                .unwrap()
        }
        .instrument(span)
        .await;
        assert!(id.is_some());
        assert_eq!(seen, id);
    }
}
