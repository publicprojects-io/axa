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
    serve_until(router, listener, shutdown_signal()).await
}

/// Serve `router` on `listener` until `shutdown` resolves, letting in-flight
/// requests finish.
///
/// # Errors
///
/// Fails when the server fails.
pub async fn serve_until(
    router: Router,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::oneshot,
    };

    #[tokio::test]
    async fn shutdown_lets_a_request_in_flight_finish() {
        let (started_tx, started_rx) = oneshot::channel::<()>();
        let started = std::sync::Arc::new(std::sync::Mutex::new(Some(started_tx)));
        let app = Router::new().route(
            "/slow",
            get(move || {
                let started = started.lock().unwrap().take();
                async move {
                    if let Some(started) = started {
                        let _ = started.send(());
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    "finished"
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(serve_until(app, listener, async {
            let _ = stop_rx.await;
        }));

        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        // Ask the server to stop only once the handler is really running.
        started_rx.await.unwrap();
        stop_tx.send(()).unwrap();

        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("200 OK"), "{response}");
        assert!(response.ends_with("finished"), "{response}");
        server.await.unwrap().unwrap();
    }

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
