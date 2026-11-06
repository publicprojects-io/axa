//! Serving a router until the process is asked to stop.

use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};

use axum::Router;
use tokio::{net::TcpListener, sync::Notify};

#[cfg(feature = "queue")]
use crate::{Error, queue::Queue};
use crate::{Result, config::Server};
#[cfg(feature = "queue")]
use tokio::sync::watch;

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
    bind_and_serve(router, config, shutdown_signal()).await
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

/// Wait out the shutdown time limit, or for ever when it is zero.
async fn grace(config: &Server) {
    if config.shutdown_timeout_seconds == 0 {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep(Duration::from_secs(config.shutdown_timeout_seconds)).await;
}

async fn bind_and_serve(
    router: Router,
    config: &Server,
    stop: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let address = format!("{}:{}", config.binding, config.port);
    let listener = TcpListener::bind(&address).await?;
    tracing::info!(address, "listening");
    let stopping = Arc::new(Notify::new());
    let told = stopping.clone();
    let signal = async move {
        stop.await;
        told.notify_one();
    };
    tokio::select! {
        served = serve_until(router, listener, signal) => served,
        () = async {
            stopping.notified().await;
            grace(config).await;
        } => {
            tracing::warn!("requests were still running at the shutdown time limit");
            Ok(())
        }
    }
}

/// Serve `router`, run `queue`, or both, until [`shutdown_signal`]: what `start`, `start --worker`,
/// and `start --all` do. Requests in flight and jobs in progress finish first.
///
/// # Errors
///
/// Fails when there is nothing to run, when the address cannot be bound, or when the job loops
/// panicked. The job loops are stopped either way.
#[cfg(feature = "queue")]
pub async fn run(router: Option<Router>, queue: Option<Arc<Queue>>, config: &Server) -> Result<()> {
    run_until(router, queue, config, shutdown_signal()).await
}

#[cfg(feature = "queue")]
async fn run_until(
    router: Option<Router>,
    queue: Option<Arc<Queue>>,
    config: &Server,
    stop: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let modes = match (&router, &queue) {
        (Some(_), Some(_)) => "server, worker",
        (Some(_), None) => "server",
        (None, Some(_)) => "worker",
        (None, None) => return Err(Error::string("neither a server nor a worker to run")),
    };
    tracing::info!("modes: {modes}");
    let (stop_jobs, stopped) = watch::channel(false);
    let jobs = queue.map(|queue| tokio::spawn(queue.run(stopped)));
    let served = if let Some(router) = router {
        bind_and_serve(router, config, stop).await
    } else {
        stop.await;
        Ok(())
    };
    let _ = stop_jobs.send(true);
    if let Some(jobs) = jobs {
        tokio::select! {
            finished = jobs => {
                finished.map_err(|error| Error::string(&format!("job loops failed: {error}")))?;
            }
            // A job left running is recovered as lost by the next worker.
            () = grace(config) => {
                tracing::warn!("jobs were still running at the shutdown time limit");
            }
        }
    }
    served
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
            shutdown_timeout_seconds: 0,
        };
        assert!(serve(Router::new(), &config).await.is_err());
    }

    #[cfg(feature = "queue")]
    fn local() -> Server {
        Server {
            port: 0,
            binding: "127.0.0.1".to_string(),
            host: String::new(),
            shutdown_timeout_seconds: 0,
        }
    }

    /// A request that never ends, and a server told to stop once it has started.
    async fn stop_with_a_hung_request(shutdown_timeout_seconds: u64) -> bool {
        let router = Router::new().route("/", get(std::future::pending::<&'static str>));
        let config = Server {
            port: 0,
            binding: "127.0.0.1".to_string(),
            host: String::new(),
            shutdown_timeout_seconds,
        };
        let (reach, reached) = tokio::sync::oneshot::channel::<()>();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let config = Server {
            port: address.port(),
            ..config
        };
        let client = tokio::spawn(async move {
            for _ in 0..200 {
                if let Ok(mut stream) = tokio::net::TcpStream::connect(address).await {
                    stream
                        .write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let _ = reach.send(());
                    let mut held = Vec::new();
                    let _ = stream.read_to_end(&mut held).await;
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let stop = async move {
            let _ = reached.await;
        };
        let served = bind_and_serve(router, &config, stop);
        let ended = tokio::time::timeout(Duration::from_secs(4), served).await;
        client.abort();
        ended.is_ok()
    }

    #[tokio::test]
    async fn a_hung_request_cannot_hold_shutdown_past_its_limit() {
        assert!(stop_with_a_hung_request(1).await);
    }

    #[tokio::test]
    async fn a_zero_shutdown_limit_waits_for_requests() {
        assert!(!stop_with_a_hung_request(0).await);
    }

    #[cfg(feature = "queue")]
    #[tokio::test]
    async fn nothing_to_run_is_an_error() {
        assert!(run_until(None, None, &local(), async {}).await.is_err());
    }

    #[cfg(feature = "queue")]
    #[tokio::test]
    async fn a_server_alone_stops_when_told() {
        run_until(Some(Router::new()), None, &local(), async {})
            .await
            .unwrap();
    }

    #[cfg(feature = "queue")]
    async fn queue() -> Arc<Queue> {
        let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
        let options = crate::queue::QueueOptions::default();
        Arc::new(Queue::connect(&uri, options).await.unwrap())
    }

    #[cfg(feature = "queue")]
    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_hung_job_cannot_hold_shutdown_past_its_limit() {
        use crate::{AppContext, worker::BackgroundWorker};

        struct Hang;

        #[async_trait::async_trait]
        impl BackgroundWorker<()> for Hang {
            fn build(_ctx: &AppContext) -> Self {
                Self
            }

            async fn perform(&self, (): ()) -> Result<()> {
                std::future::pending().await
            }
        }

        let uri = std::env::var("AXA_TEST_DATABASE_URL").expect("AXA_TEST_DATABASE_URL");
        let options = crate::queue::QueueOptions::default();
        let mut queue = Queue::connect(&uri, options).await.unwrap();
        queue.register(Hang);
        let id = queue
            .enqueue("Hang", serde_json::json!(null))
            .await
            .unwrap();
        let config = Server {
            shutdown_timeout_seconds: 1,
            ..local()
        };
        let stop = tokio::time::sleep(Duration::from_millis(1_500));
        let run = run_until(None, Some(Arc::new(queue)), &config, stop);
        let ended = tokio::time::timeout(Duration::from_secs(6), run).await;
        ended.expect("a hung job held shutdown").unwrap();
        let pool = sqlx::PgPool::connect(&uri).await.unwrap();
        sqlx::query("DELETE FROM axa_queue WHERE id = $1")
            .bind(uuid::Uuid::parse_str(&id).unwrap())
            .execute(&pool)
            .await
            .unwrap();
    }

    #[cfg(feature = "queue")]
    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn a_worker_alone_stops_when_told() {
        run_until(None, Some(queue().await), &local(), async {})
            .await
            .unwrap();
    }

    #[cfg(feature = "queue")]
    #[tokio::test]
    #[ignore = "needs AXA_TEST_DATABASE_URL"]
    async fn the_job_loops_stop_when_the_server_cannot_start() {
        let config = Server {
            port: 1,
            binding: "256.0.0.1".to_string(),
            host: String::new(),
            shutdown_timeout_seconds: 0,
        };
        let router = Some(Router::new());
        let run = run_until(router, Some(queue().await), &config, std::future::pending());
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), run).await;
        assert!(outcome.expect("the job loops kept running").is_err());
    }
}
