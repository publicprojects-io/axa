//! Serving a router until the process is asked to stop.

use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;

use crate::{Result, config::Server};

/// Resolve when the process receives `SIGTERM` or Ctrl-C.
///
/// # Panics
///
/// If the signal handlers cannot be installed.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

/// Serve `router` until [`shutdown_signal`], letting in-flight requests
/// finish.
///
/// # Errors
///
/// Fails when the address cannot be bound.
pub async fn serve(router: Router, config: &Server) -> Result<()> {
    let address = format!("{}:{}", config.binding, config.port);
    let listener = TcpListener::bind(&address).await?;
    tracing::info!(address, "listening");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serve_reports_an_address_it_cannot_bind() {
        let config = Server {
            port: 1,
            binding: "256.0.0.1".to_string(),
            host: String::new(),
        };
        assert!(serve(Router::new(), &config).await.is_err());
    }
}
