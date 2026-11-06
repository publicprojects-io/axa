//! Serving a router until the process is asked to stop.

#[cfg(feature = "queue")]
use std::sync::OnceLock;
use std::{future::Future, net::SocketAddr, pin::pin, sync::Arc, time::Duration};

use axum::{Router, extract::ConnectInfo, http::Request};
use hyper::{
    body::Incoming,
    service::{Service, service_fn},
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::{conn::auto::Builder, graceful::GracefulShutdown},
    service::TowerToHyperService,
};
use tokio::{net::TcpListener, sync::Notify, time::Instant};

#[cfg(feature = "queue")]
use crate::{Error, queue::Queue};
use crate::{
    Result,
    config::{DEFAULT_HEADER_READ_TIMEOUT_SECONDS, Server},
};
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
/// requests finish. A client has
/// [`DEFAULT_HEADER_READ_TIMEOUT_SECONDS`] to send a request's headers.
///
/// # Errors
///
/// Fails when the server fails.
pub async fn serve_until(
    router: Router,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    accept_until(
        router,
        listener,
        DEFAULT_HEADER_READ_TIMEOUT_SECONDS,
        shutdown,
    )
    .await
}

/// The connection loop. This is `axum::serve` plus a limit on reading headers, which
/// `axum::serve` cannot set: without one, a client that trickles out its headers holds
/// a connection and a task for as long as it likes.
async fn accept_until(
    router: Router,
    listener: TcpListener,
    header_read_timeout_seconds: u64,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(
            (header_read_timeout_seconds > 0)
                .then(|| Duration::from_secs(header_read_timeout_seconds)),
        );
    let connections = GracefulShutdown::new();
    let mut shutdown = pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, address) = match accepted {
                    Ok(connection) => connection,
                    // Running out of file descriptors, say. It passes, and a tight
                    // loop would not help it to.
                    Err(error) => {
                        tracing::warn!(%error, "could not accept a connection");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };
                let router = TowerToHyperService::new(router.clone());
                let service = service_fn(move |mut request: Request<Incoming>| {
                    request.extensions_mut().insert(ConnectInfo::<SocketAddr>(address));
                    router.call(request)
                });
                let connection = builder
                    .serve_connection_with_upgrades(TokioIo::new(stream), service)
                    .into_owned();
                let connection = connections.watch(connection);
                tokio::spawn(async move {
                    if let Err(error) = connection.await {
                        tracing::trace!(%error, "connection ended");
                    }
                });
            }
            () = &mut shutdown => break,
        }
    }
    drop(listener);
    connections.shutdown().await;
    Ok(())
}

/// Wait until the shutdown time limit has passed since `told`, or for ever when
/// it is zero.
async fn grace(config: &Server, told: Instant) {
    if config.shutdown_timeout_seconds == 0 {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep_until(told + Duration::from_secs(config.shutdown_timeout_seconds)).await;
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
        served = accept_until(router, listener, config.header_read_timeout_seconds, signal) => served,
        () = async {
            stopping.notified().await;
            grace(config, Instant::now()).await;
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
pub(crate) async fn run_until(
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
    // Requests and jobs hear at once, and share one time limit, so that no job
    // is claimed during the wait and the whole of shutdown stays under it.
    let stop_jobs = Arc::new(stop_jobs);
    let told = Arc::new(OnceLock::new());
    let stop = {
        let (stop_jobs, told) = (stop_jobs.clone(), told.clone());
        async move {
            stop.await;
            told.get_or_init(Instant::now);
            let _ = stop_jobs.send(true);
        }
    };
    let served = if let Some(router) = router {
        bind_and_serve(router, config, stop).await
    } else {
        stop.await;
        Ok(())
    };
    // A server that could not start was never told to stop.
    let told = *told.get_or_init(Instant::now);
    let _ = stop_jobs.send(true);
    if let Some(jobs) = jobs {
        tokio::select! {
            finished = jobs => {
                finished.map_err(|error| Error::string(&format!("job loops failed: {error}")))?;
            }
            // A job left running is recovered as lost by the next worker.
            () = grace(config, told) => {
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

    /// Whether a server closes on a client that starts its headers and stalls.
    async fn drops_a_stalled_client(header_read_timeout_seconds: u64) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(accept_until(
            Router::new(),
            listener,
            header_read_timeout_seconds,
            std::future::pending(),
        ));
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut response))
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn a_client_that_stalls_in_its_headers_is_dropped() {
        assert!(drops_a_stalled_client(1).await);
    }

    #[tokio::test]
    async fn a_header_limit_of_zero_waits_for_ever() {
        assert!(!drops_a_stalled_client(0).await);
    }

    #[tokio::test]
    async fn a_handler_still_sees_the_clients_address() {
        let app = Router::new().route(
            "/",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.ip().to_string() }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(serve_until(app, listener, std::future::pending()));
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.ends_with("127.0.0.1"), "{response}");
    }

    #[tokio::test]
    async fn serve_reports_an_address_it_cannot_bind() {
        let config = Server {
            port: 1,
            binding: "256.0.0.1".to_string(),
            host: String::new(),
            shutdown_timeout_seconds: 0,
            header_read_timeout_seconds: 0,
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
            header_read_timeout_seconds: 0,
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
            header_read_timeout_seconds: 0,
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
}
