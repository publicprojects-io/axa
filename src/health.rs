//! The `/_ping`, `/_health`, and `/_readiness` routes.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::{AppContext, routes::Routes};

#[derive(Serialize)]
struct Health {
    ok: bool,
}

async fn ok() -> Json<Health> {
    Json(Health { ok: true })
}

/// Whether the service's dependencies answer. With no database or queue
/// feature there is nothing to check, so this is simply true.
#[cfg_attr(
    not(any(feature = "sea-orm", feature = "queue")),
    allow(clippy::unused_async)
)]
async fn ready(ctx: &AppContext) -> bool {
    #[cfg(feature = "sea-orm")]
    if let Err(error) = ctx.db.ping().await {
        tracing::error!(%error, "readiness_db_ping_error");
        return false;
    }
    #[cfg(feature = "queue")]
    if let Some(queue) = &ctx.queue_provider
        && let Err(error) = queue.ping().await
    {
        tracing::error!(%error, "readiness_queue_ping_error");
        return false;
    }
    let _ = ctx;
    true
}

async fn readiness(State(ctx): State<AppContext>) -> Response {
    if ready(&ctx).await {
        Json(Health { ok: true }).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(Health { ok: false })).into_response()
    }
}

/// The health routes.
#[must_use]
pub fn routes() -> Routes {
    Routes::new()
        .add("/_ping", get(ok))
        .add("/_health", get(ok))
        .add("/_readiness", get(readiness))
}

#[cfg(all(test, not(feature = "sea-orm")))]
mod tests {
    use super::*;
    use crate::{
        Environment,
        config::{Config, Defaults},
        routes::AppRoutes,
    };
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn every_health_route_answers_ok() {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        let ctx = AppContext::new(Environment::Test, (), config);
        let router = AppRoutes::with_default_routes().into_router(ctx);
        for path in ["/_ping", "/_health", "/_readiness"] {
            let request = Request::builder().uri(path).body(Body::empty()).unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
    }
}
