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

/// Whether the service's dependencies answer.
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
