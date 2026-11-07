//! Serving a router until the process is asked to stop.

#[cfg(feature = "queue")]
use std::sync::OnceLock;
use std::{
    convert::Infallible,
    future::Future,
    io,
    net::SocketAddr,
    pin::{Pin, pin},
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum::{Router, body::Body, extract::ConnectInfo, http::Request};
use hyper::{
    body::{Body as HttpBody, Bytes, Frame, Incoming, SizeHint},
    server::conn::http1,
    service::{Service, service_fn},
};
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{Notify, OwnedSemaphorePermit, Semaphore, watch},
    time::Instant,
};

#[cfg(feature = "queue")]
use crate::{Error, queue::Queue};
use crate::{
    Result,
    config::{
        DEFAULT_HEADER_READ_TIMEOUT_SECONDS, DEFAULT_KEEP_ALIVE_TIMEOUT_SECONDS,
        DEFAULT_SEND_STALL_TIMEOUT_SECONDS, Server,
    },
};

/// The most connections a server holds open at once unless `MAX_CONNECTIONS` says
/// otherwise. More wait to be accepted.
pub const MAX_CONNECTIONS: usize = 10_000;

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
/// requests finish. The limits on connections are the defaults: see [`Server`].
///
/// # Errors
///
/// Fails when the server fails.
pub async fn serve_until(
    router: Router,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let limits = Limits::new(
        DEFAULT_HEADER_READ_TIMEOUT_SECONDS,
        DEFAULT_KEEP_ALIVE_TIMEOUT_SECONDS,
        DEFAULT_SEND_STALL_TIMEOUT_SECONDS,
        MAX_CONNECTIONS,
    );
    accept_until(router, listener, limits, shutdown).await
}

/// What one connection is allowed.
#[derive(Clone, Copy)]
struct Limits {
    /// From the start of a connection or a request, to the end of its headers.
    header_read: Option<Duration>,
    /// From the end of a response, to the start of the next request.
    keep_alive: Option<Duration>,
    /// From a write the client's socket cannot take, to the next that it can.
    send_stall: Option<Duration>,
    /// Connections open at once.
    max_connections: usize,
}

impl Limits {
    /// Limits from settings, where a time of `0` means no limit. There is always a
    /// connection limit, as `Semaphore` with no places would never accept one.
    fn new(
        header_read_seconds: u64,
        keep_alive_seconds: u64,
        send_stall_seconds: u64,
        max_connections: usize,
    ) -> Self {
        let seconds = |seconds| (seconds > 0).then(|| Duration::from_secs(seconds));
        Self {
            header_read: seconds(header_read_seconds),
            keep_alive: seconds(keep_alive_seconds),
            send_stall: seconds(send_stall_seconds),
            max_connections: max_connections.clamp(1, Semaphore::MAX_PERMITS),
        }
    }
}

impl From<&Server> for Limits {
    fn from(config: &Server) -> Self {
        Self::new(
            config.header_read_timeout_seconds,
            config.keep_alive_timeout_seconds,
            config.send_stall_timeout_seconds,
            config.max_connections,
        )
    }
}

/// What a connection is waiting for, and until when.
#[derive(Clone, Copy)]
enum Phase {
    /// The rest of a request's headers.
    Head(Option<Instant>),
    /// A next request. A client or a load balancer may keep this connection for one.
    Idle(Option<Instant>),
    /// The service, which has its own limits, so this one is set only while a client
    /// that has stopped reading holds up the response.
    Serving(Option<Instant>),
}

/// Moves a connection between its phases. `hyper` has one timer, which runs across
/// both a request's headers and the wait between requests, and these need different
/// limits: a short one for a client that stalls, and for an idle connection one longer
/// than the load balancer in front, which would otherwise reuse a connection as it
/// closes. So `hyper`'s timer stays off and this does both.
#[derive(Clone)]
struct Tracker {
    phase: watch::Sender<Phase>,
    limits: Limits,
}

impl Tracker {
    fn new(limits: Limits) -> Self {
        let deadline = limits.header_read.and_then(from_now);
        Self {
            phase: watch::Sender::new(Phase::Head(deadline)),
            limits,
        }
    }

    /// The first bytes of the next request started its header limit.
    fn received(&self) {
        self.phase.send_if_modified(|phase| {
            let idle = matches!(phase, Phase::Idle(_));
            if idle {
                *phase = Phase::Head(self.limits.header_read.and_then(from_now));
            }
            idle
        });
    }

    fn serving(&self) {
        self.phase.send_replace(Phase::Serving(None));
    }

    fn finished(&self) {
        let deadline = self.limits.keep_alive.and_then(from_now);
        self.phase.send_replace(Phase::Idle(deadline));
    }

    /// Bytes reached the socket, so a response is still going out, and the connection
    /// is not yet idle. A client that stops reading stops this, and is dropped.
    fn sent(&self) {
        self.phase.send_if_modified(|phase| match phase {
            Phase::Idle(deadline) => {
                *deadline = self.limits.keep_alive.and_then(from_now);
                true
            }
            Phase::Serving(deadline) => deadline.take().is_some(),
            Phase::Head(_) => false,
        });
    }

    /// What a write to the socket came to.
    fn wrote(&self, polled: &Poll<io::Result<usize>>) {
        match polled {
            Poll::Ready(Ok(written)) if *written > 0 => self.sent(),
            Poll::Pending => self.blocked(),
            Poll::Ready(_) => {}
        }
    }

    /// The socket can take no more, which is the only sign of a client that has stopped
    /// reading. A handler with nothing to send never gets here. Waiting does not move a
    /// deadline already set, so only a write that goes through buys more time.
    fn blocked(&self) {
        let Some(limit) = self.limits.send_stall.and_then(from_now) else {
            return;
        };
        self.phase.send_if_modified(|phase| match phase {
            Phase::Serving(deadline) | Phase::Idle(deadline) => {
                let earliest = deadline.map_or(limit, |deadline| deadline.min(limit));
                deadline.replace(earliest) != Some(earliest)
            }
            Phase::Head(_) => false,
        });
    }
}

/// When a limit ends, or `None` for one too long for the clock to hold, which is no
/// limit: `Instant` panics on adding it, and any such limit is meant as for ever.
fn from_now(limit: Duration) -> Option<Instant> {
    Instant::now().checked_add(limit)
}

/// Resolve once a connection is past its deadline.
async fn overdue(mut phase: watch::Receiver<Phase>) {
    loop {
        let deadline = match *phase.borrow_and_update() {
            Phase::Head(deadline) | Phase::Idle(deadline) | Phase::Serving(deadline) => deadline,
        };
        let expires = async {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = expires => return,
            changed = phase.changed() => {
                if changed.is_err() {
                    return std::future::pending().await;
                }
            }
        }
    }
}

/// A connection that tells its [`Tracker`] when bytes move. It holds the connection's
/// slot, because an upgrade outlives the task that serves the connection, and only the
/// stream lasts as long as the socket.
struct Tracked {
    stream: TcpStream,
    tracker: Tracker,
    _slot: OwnedSemaphorePermit,
}

impl AsyncRead for Tracked {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let polled = Pin::new(&mut this.stream).poll_read(context, buffer);
        if buffer.filled().len() > before {
            this.tracker.received();
        }
        polled
    }
}

impl AsyncWrite for Tracked {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.stream).poll_write(context, bytes);
        this.tracker.wrote(&polled);
        polled
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(context)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.stream).poll_write_vectored(context, buffers);
        this.tracker.wrote(&polled);
        polled
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

/// Tells a [`Tracker`] the service has finished its response. `hyper` drops the body
/// when it holds the last of it, which can be long before that reaches the client, so
/// [`Tracker::sent`] keeps the connection from being taken for idle until then.
struct Finished(Tracker);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.finished();
    }
}

/// A response body that reports its end.
struct TrackedBody {
    body: Body,
    _finished: Finished,
}

impl HttpBody for TrackedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, axum::Error>>> {
        Pin::new(&mut self.get_mut().body).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// Take a slot, then the next connection. Each is safe to cancel, so shutdown can
/// interrupt this at any point.
async fn next_connection(
    listener: &TcpListener,
    slots: &Arc<Semaphore>,
) -> (TcpStream, SocketAddr, OwnedSemaphorePermit) {
    let slot = if let Ok(slot) = slots.clone().try_acquire_owned() {
        slot
    } else {
        tracing::warn!("at the connection limit, so new connections wait");
        slots
            .clone()
            .acquire_owned()
            .await
            .expect("the slots are never closed")
    };
    loop {
        match listener.accept().await {
            Ok((stream, address)) => return (stream, address, slot),
            // Running out of file descriptors, say. It passes, and a tight loop would
            // not help it to.
            Err(error) => {
                tracing::warn!(%error, "could not accept a connection");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// The connection loop. This is `axum::serve` plus the limits in [`Limits`], which
/// `axum::serve` cannot set: without them, a client that sends nothing, or trickles out
/// its headers, holds a connection and a task for as long as it likes, and enough of
/// them use up the process's file descriptors.
///
/// It speaks HTTP/1 only, as this crate has always been built to. The limits count a
/// connection as one request at a time, and detecting the protocol first would wait,
/// with no limit, for a client's first bytes. A proxy in front can offer HTTP/2.
///
/// Past its limit it stops accepting, so a connection waits in the listen queue for a
/// place rather than being refused.
async fn accept_until(
    router: Router,
    listener: TcpListener,
    limits: Limits,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let mut builder = http1::Builder::new();
    // `Tracker` does this, as it says.
    builder.header_read_timeout(None);
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    // Each connection holds a receiver, so the sender sees when the last has gone.
    let (stop, stopping) = watch::channel(false);
    let mut shutdown = pin!(shutdown);
    loop {
        tokio::select! {
            (stream, address, slot) = next_connection(&listener, &slots) => {
                let tracker = Tracker::new(limits);
                let phase = tracker.phase.subscribe();
                let router = TowerToHyperService::new(router.clone());
                let serving = tracker.clone();
                let service = service_fn(move |mut request: Request<Incoming>| {
                    request.extensions_mut().insert(ConnectInfo::<SocketAddr>(address));
                    serving.serving();
                    let tracker = serving.clone();
                    let response = router.call(request);
                    async move {
                        let response = response.await?;
                        Ok::<_, Infallible>(response.map(|body| TrackedBody {
                            body,
                            _finished: Finished(tracker),
                        }))
                    }
                });
                let stream = TokioIo::new(Tracked { stream, tracker, _slot: slot });
                let connection = builder.serve_connection(stream, service).with_upgrades();
                let mut stopping = stopping.clone();
                tokio::spawn(async move {
                    let mut connection = pin!(connection);
                    let stop = async {
                        // A sender dropped is a server gone, and it is told first.
                        let _ = stopping.wait_for(|stop| *stop).await;
                    };
                    tokio::select! {
                        ended = connection.as_mut() => log_end(ended),
                        () = overdue(phase.clone()) => {
                            tracing::debug!("dropped a quiet connection");
                        }
                        () = stop => {
                            connection.as_mut().graceful_shutdown();
                            tokio::select! {
                                ended = connection => log_end(ended),
                                () = overdue(phase) => {}
                            }
                        }
                    }
                });
            }
            () = &mut shutdown => break,
        }
    }
    drop(listener);
    drop(stopping);
    let _ = stop.send(true);
    stop.closed().await;
    Ok(())
}

/// A connection ending badly is the client's business, so it is only worth a trace.
fn log_end(ended: std::result::Result<(), hyper::Error>) {
    if let Err(error) = ended {
        tracing::trace!(%error, "connection ended");
    }
}

/// Wait until the shutdown time limit has passed since `told`, or for ever when
/// it is zero.
async fn grace(config: &Server, told: Instant) {
    if config.shutdown_timeout_seconds == 0 {
        std::future::pending::<()>().await;
    }
    match told.checked_add(Duration::from_secs(config.shutdown_timeout_seconds)) {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
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
        served = accept_until(router, listener, config.into(), signal) => served,
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
            Limits::new(header_read_timeout_seconds, 0, 0, 0),
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

    /// Whether a server closes on a client that sends only `opening`.
    async fn drops_a_client_sending(opening: &[u8]) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(accept_until(
            Router::new(),
            listener,
            Limits::new(1, 0, 0, 0),
            std::future::pending(),
        ));
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(opening).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut response))
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn a_client_that_sends_nothing_is_dropped() {
        assert!(drops_a_client_sending(b"").await);
    }

    #[tokio::test]
    async fn an_http2_client_that_goes_silent_is_dropped() {
        assert!(drops_a_client_sending(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await);
    }

    #[tokio::test]
    async fn a_connection_can_be_upgraded() {
        async fn upgrade(request: axum::extract::Request) -> axum::response::Response {
            tokio::spawn(async move {
                let upgraded = hyper::upgrade::on(request).await.unwrap();
                let mut upgraded = TokioIo::new(upgraded);
                upgraded.write_all(b"upgraded").await.unwrap();
            });
            axum::response::Response::builder()
                .status(axum::http::StatusCode::SWITCHING_PROTOCOLS)
                .header("connection", "upgrade")
                .header("upgrade", "test")
                .body(axum::body::Body::empty())
                .unwrap()
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(accept_until(
            Router::new().route("/", get(upgrade)),
            listener,
            Limits::new(0, 0, 0, 0),
            std::future::pending(),
        ));
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: upgrade\r\nUpgrade: test\r\n\r\n")
            .await
            .unwrap();
        let mut received = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut received));
        let _ = read.await;
        let received = String::from_utf8_lossy(&received);
        assert!(received.starts_with("HTTP/1.1 101"), "{received}");
        assert!(received.ends_with("upgraded"), "{received}");
    }

    /// Read `size` bytes in small steps, and give back how many arrived.
    async fn read_slowly(stream: &mut TcpStream, size: usize) -> usize {
        let mut buffer = vec![0; 256 * 1024];
        let mut total = 0;
        while total < size {
            tokio::time::sleep(Duration::from_millis(20)).await;
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => total += read,
            }
        }
        total
    }

    const LONG_BODY: usize = 16 * 1024 * 1024;

    fn long_body() -> Router {
        Router::new().route("/", get(|| async { vec![b'x'; LONG_BODY] }))
    }

    #[tokio::test]
    async fn a_long_response_outlasts_the_keep_alive_limit() {
        let address = limited(long_body(), millis(60_000, 200, 0)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert!(read_slowly(&mut stream, LONG_BODY).await >= LONG_BODY);
    }

    #[tokio::test]
    async fn a_response_going_out_at_shutdown_is_not_cut_off() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let shutdown = async {
            let _ = stopped.await;
        };
        tokio::spawn(accept_until(
            long_body(),
            listener,
            millis(60_000, 200, 0),
            shutdown,
        ));
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.send(()).unwrap();
        assert!(read_slowly(&mut stream, LONG_BODY).await >= LONG_BODY);
    }

    #[tokio::test]
    async fn a_client_that_stops_reading_a_finished_response_is_dropped() {
        let address = limited(long_body(), millis(60_000, 200, 0)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let read =
            tokio::time::timeout(Duration::from_secs(5), read_slowly(&mut stream, usize::MAX))
                .await;
        assert!(read.unwrap() < LONG_BODY);
    }

    /// Limits where only the send stall can end a connection a client has stopped
    /// reading from.
    fn stall(limit: u64, max_connections: usize) -> Limits {
        Limits {
            send_stall: Some(Duration::from_millis(limit)),
            ..millis(60_000, 60_000, max_connections)
        }
    }

    /// A body that sends for as long as anyone takes it.
    struct Endless;

    impl HttpBody for Endless {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<std::result::Result<Frame<Bytes>, Infallible>>> {
            Poll::Ready(Some(Ok(Frame::data(Bytes::from(vec![b'x'; 65_536])))))
        }
    }

    /// A route that never ends, beside one that answers.
    fn endless() -> Router {
        Router::new()
            .route("/", get(|| async { "hello" }))
            .route("/stream", get(|| async { Body::new(Endless) }))
    }

    const STREAM: &[u8] = b"GET /stream HTTP/1.1\r\nHost: x\r\n\r\n";

    #[tokio::test]
    async fn a_client_that_stops_reading_a_stream_loses_its_slot() {
        let address = limited(endless(), stall(200, 1)).await;
        let mut stuck = TcpStream::connect(address).await.unwrap();
        stuck.write_all(STREAM).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut next = TcpStream::connect(address).await.unwrap();
        next.write_all(REQUEST).await.unwrap();
        assert!(response(&mut next).await.contains("hello"));
    }

    #[tokio::test]
    async fn a_stream_with_no_send_stall_limit_keeps_its_slot() {
        let limits = Limits {
            send_stall: None,
            ..stall(200, 1)
        };
        let address = limited(endless(), limits).await;
        let mut stuck = TcpStream::connect(address).await.unwrap();
        stuck.write_all(STREAM).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut next = TcpStream::connect(address).await.unwrap();
        next.write_all(REQUEST).await.unwrap();
        let mut answer = [0; 16];
        let waited = tokio::time::timeout(Duration::from_millis(600), next.read(&mut answer));
        assert!(waited.await.is_err());
    }

    #[tokio::test]
    async fn a_reader_that_keeps_up_outlasts_the_send_stall_limit() {
        let address = limited(long_body(), stall(200, 0)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert!(read_slowly(&mut stream, LONG_BODY).await >= LONG_BODY);
    }

    #[tokio::test]
    async fn a_reader_that_keeps_up_with_a_stream_outlasts_the_send_stall_limit() {
        let address = limited(endless(), stall(200, 0)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(STREAM).await.unwrap();
        assert!(read_slowly(&mut stream, LONG_BODY).await >= LONG_BODY);
    }

    #[tokio::test]
    async fn waiting_on_a_blocked_write_does_not_move_its_deadline() {
        let tracker = Tracker::new(stall(10_000, 0));
        tracker.serving();
        tracker.blocked();
        let deadline = |tracker: &Tracker| match *tracker.phase.borrow() {
            Phase::Serving(deadline) => deadline,
            Phase::Head(_) | Phase::Idle(_) => None,
        };
        let first = deadline(&tracker);
        assert!(first.is_some());
        tokio::time::sleep(Duration::from_millis(5)).await;
        tracker.blocked();
        assert_eq!(deadline(&tracker), first);
    }

    #[tokio::test]
    async fn a_stalled_response_is_dropped_with_no_keep_alive_limit() {
        let limits = Limits {
            keep_alive: None,
            ..stall(200, 0)
        };
        let address = limited(long_body(), limits).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(REQUEST).await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let read =
            tokio::time::timeout(Duration::from_secs(5), read_slowly(&mut stream, usize::MAX))
                .await;
        assert!(read.unwrap() < LONG_BODY);
    }

    #[tokio::test]
    async fn a_stalled_client_cannot_hold_shutdown() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let shutdown = async {
            let _ = stopped.await;
        };
        let server = tokio::spawn(accept_until(endless(), listener, stall(200, 0), shutdown));
        let mut stuck = TcpStream::connect(address).await.unwrap();
        stuck.write_all(STREAM).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.send(()).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), server).await;
        assert!(ended.is_ok());
    }

    #[tokio::test]
    async fn an_upgraded_connection_keeps_its_slot() {
        let app = Router::new().route(
            "/",
            get(|request: axum::extract::Request| async move {
                let upgrade = hyper::upgrade::on(request);
                tokio::spawn(async move {
                    let mut upgraded = TokioIo::new(upgrade.await.unwrap());
                    let mut byte = [0; 1];
                    while upgraded.read_exact(&mut byte).await.is_ok() {}
                });
                axum::http::Response::builder()
                    .status(axum::http::StatusCode::SWITCHING_PROTOCOLS)
                    .header("connection", "upgrade")
                    .header("upgrade", "echo")
                    .body(Body::empty())
                    .unwrap()
            })
            .post(|| async { "answered" }),
        );
        let address = limited(app, millis(60_000, 60_000, 1)).await;
        let mut first = TcpStream::connect(address).await.unwrap();
        first
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: upgrade\r\nUpgrade: echo\r\n\r\n")
            .await
            .unwrap();
        let mut buffer = vec![0; 1024];
        let read = first.read(&mut buffer).await.unwrap();
        assert!(String::from_utf8_lossy(&buffer[..read]).contains("101"));

        let post = b"POST / HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
        let mut second = TcpStream::connect(address).await.unwrap();
        second.write_all(post).await.unwrap();
        let waiting = Duration::from_millis(300);
        assert!(
            tokio::time::timeout(waiting, second.read(&mut buffer))
                .await
                .is_err()
        );

        drop(first);
        let read = tokio::time::timeout(Duration::from_secs(5), second.read(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&buffer[..read]).contains("200"));
    }

    #[tokio::test]
    async fn a_limit_too_long_for_the_clock_means_none() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let limits = Limits::new(u64::MAX, u64::MAX, u64::MAX, 10);
        let server = tokio::spawn(accept_until(
            hello(),
            listener,
            limits,
            std::future::pending(),
        ));
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        assert!(response(&mut stream).await.contains("hello"));
        assert!(!server.is_finished());
    }

    #[tokio::test]
    async fn connections_past_the_limit_wait_their_turn() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(accept_until(
            Router::new().route("/", get(|| async { "ok" })),
            listener,
            Limits::new(0, 0, 0, 1),
            std::future::pending(),
        ));
        let request = b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
        let first = TcpStream::connect(address).await.unwrap();

        // The first connection holds the only place, so the second gets no answer.
        let mut second = TcpStream::connect(address).await.unwrap();
        second.write_all(request).await.unwrap();
        let mut answer = [0; 16];
        let waited = tokio::time::timeout(Duration::from_millis(200), second.read(&mut answer));
        assert!(waited.await.is_err());

        // Letting go of it lets the second through.
        drop(first);
        let answered = tokio::time::timeout(Duration::from_secs(3), second.read(&mut answer));
        assert!(answered.await.unwrap().unwrap() > 0);
    }

    #[tokio::test]
    async fn a_header_limit_of_zero_waits_for_ever() {
        assert!(!drops_a_stalled_client(0).await);
    }

    /// Limits short enough to test, and a server under them.
    async fn limited(app: Router, limits: Limits) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(accept_until(app, listener, limits, std::future::pending()));
        address
    }

    fn millis(header_read: u64, keep_alive: u64, max_connections: usize) -> Limits {
        Limits {
            header_read: Some(Duration::from_millis(header_read)),
            keep_alive: Some(Duration::from_millis(keep_alive)),
            send_stall: None,
            max_connections: if max_connections == 0 {
                Semaphore::MAX_PERMITS
            } else {
                max_connections
            },
        }
    }

    /// As [`millis`], with a send stall limit as short as the keep-alive limit.
    fn stalling(header_read: u64, keep_alive: u64) -> Limits {
        Limits {
            send_stall: Some(Duration::from_millis(keep_alive)),
            ..millis(header_read, keep_alive, 0)
        }
    }

    fn hello() -> Router {
        Router::new().route("/", get(|| async { "hello" }))
    }

    const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";

    /// Read until a whole response has arrived.
    async fn response(stream: &mut TcpStream) -> String {
        let mut bytes = vec![0; 1024];
        let read = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8_lossy(&bytes[..read]).into_owned()
    }

    /// Whether the server closes `stream` within `limit`.
    async fn closes_within(stream: &mut TcpStream, limit: Duration) -> bool {
        let mut rest = Vec::new();
        tokio::time::timeout(limit, stream.read_to_end(&mut rest))
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn an_idle_connection_outlasts_the_header_limit() {
        let address = limited(hello(), millis(100, 60_000, 0)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(REQUEST).await.unwrap();
        assert!(response(&mut stream).await.ends_with("hello"));
        tokio::time::sleep(Duration::from_millis(400)).await;
        stream.write_all(REQUEST).await.unwrap();
        assert!(response(&mut stream).await.ends_with("hello"));
    }

    #[tokio::test]
    async fn an_idle_connection_is_dropped_at_the_keep_alive_limit() {
        let address = limited(hello(), millis(60_000, 100, 0)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(REQUEST).await.unwrap();
        assert!(response(&mut stream).await.ends_with("hello"));
        assert!(closes_within(&mut stream, Duration::from_secs(2)).await);
    }

    #[tokio::test]
    async fn a_stall_in_a_second_requests_headers_is_dropped() {
        let address = limited(hello(), millis(100, 60_000, 0)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(REQUEST).await.unwrap();
        assert!(response(&mut stream).await.ends_with("hello"));
        stream.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
        assert!(closes_within(&mut stream, Duration::from_secs(2)).await);
    }

    #[tokio::test]
    async fn neither_limit_cuts_off_a_slow_handler() {
        let app = Router::new().route(
            "/",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(400)).await;
                "late"
            }),
        );
        let address = limited(app, stalling(100, 100)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(REQUEST).await.unwrap();
        assert!(response(&mut stream).await.ends_with("late"));
    }

    #[tokio::test]
    async fn neither_limit_cuts_off_a_slow_response_body() {
        let app = Router::new().route(
            "/",
            get(|| async {
                Body::new(SlowBody {
                    pause: Box::pin(tokio::time::sleep(Duration::from_millis(400))),
                    sent: 0,
                })
            }),
        );
        let address = limited(app, stalling(100, 100)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut whole = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut whole))
            .await
            .unwrap()
            .unwrap();
        let whole = String::from_utf8_lossy(&whole);
        assert!(
            whole.contains("first") && whole.contains("second"),
            "{whole}"
        );
    }

    /// Two chunks, with a pause between them.
    struct SlowBody {
        pause: Pin<Box<tokio::time::Sleep>>,
        sent: u8,
    }

    impl HttpBody for SlowBody {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<Option<std::result::Result<Frame<Bytes>, Infallible>>> {
            self.sent += 1;
            match self.sent {
                1 => Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"first"))))),
                2 => match self.pause.as_mut().poll(context) {
                    Poll::Ready(()) => {
                        Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"second")))))
                    }
                    Poll::Pending => {
                        self.sent = 1;
                        Poll::Pending
                    }
                },
                _ => Poll::Ready(None),
            }
        }
    }

    #[tokio::test]
    async fn an_upgraded_connection_is_left_alone() {
        let app = Router::new().route(
            "/",
            get(|request: axum::extract::Request| async move {
                let upgrade = hyper::upgrade::on(request);
                tokio::spawn(async move {
                    let mut upgraded = TokioIo::new(upgrade.await.unwrap());
                    let mut byte = [0; 1];
                    while upgraded.read_exact(&mut byte).await.is_ok() {
                        upgraded.write_all(&byte).await.unwrap();
                    }
                });
                axum::http::Response::builder()
                    .status(axum::http::StatusCode::SWITCHING_PROTOCOLS)
                    .header("connection", "upgrade")
                    .header("upgrade", "echo")
                    .body(Body::empty())
                    .unwrap()
            }),
        );
        let address = limited(app, stalling(100, 100)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: upgrade\r\nUpgrade: echo\r\n\r\n")
            .await
            .unwrap();
        assert!(response(&mut stream).await.contains("101"));
        tokio::time::sleep(Duration::from_millis(400)).await;
        stream.write_all(b"x").await.unwrap();
        assert_eq!(response(&mut stream).await, "x");
    }

    #[test]
    fn a_limit_of_zero_means_none() {
        let limits = Limits::new(0, 0, 0, 0);
        assert!(limits.send_stall.is_none());
        let limits = Limits::new(0, 0, 7, 0);
        assert_eq!(limits.send_stall, Some(Duration::from_secs(7)));
        let limits = Limits::new(0, 0, 0, 0);
        assert_eq!(limits.header_read, None);
        assert_eq!(limits.keep_alive, None);
        assert_eq!(limits.max_connections, 1);
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
            keep_alive_timeout_seconds: 0,
            send_stall_timeout_seconds: 0,
            max_connections: 1,
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
            keep_alive_timeout_seconds: 0,
            send_stall_timeout_seconds: 0,
            max_connections: 1,
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
            keep_alive_timeout_seconds: 0,
            send_stall_timeout_seconds: 0,
            max_connections: 1,
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
