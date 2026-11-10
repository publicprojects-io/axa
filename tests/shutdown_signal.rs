//! `server::run` and a real `SIGTERM`.
//!
//! Each test is run twice. Run by `cargo test` it is the parent: it starts this same
//! binary again, with [`ROLE`] set, as the child that serves. The child is a separate
//! process because the signal has to reach a real one, whose exit a test can then read.
//! The job tests need a real Postgres, so they are ignored by default.

#![cfg(all(unix, feature = "queue"))]

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::process::ExitStatusExt,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axa::{
    AppContext, Result,
    config::Server,
    queue::{Queue, QueueOptions},
    server,
    worker::BackgroundWorker,
};
use axum::{Router, routing::get};

/// Selects the child's part, and being set is what makes a process the child.
const ROLE: &str = "AXA_SHUTDOWN_TEST_ROLE";
/// The port the child serves on.
const PORT: &str = "AXA_SHUTDOWN_TEST_PORT";
/// What the child prints once a request is inside its handler.
const IN_HANDLER: &str = "in handler";
/// How long a child has to do anything, after which the test fails.
const DEADLINE: Duration = Duration::from_secs(20);
/// `shutdown_signal` installs its handlers when first polled, and until then a
/// `SIGTERM` takes the default action. A child that is ready has reached that point
/// or is about to, so this is only for the moment in between.
const SETTLE: Duration = Duration::from_millis(200);
/// How long the slow handler and the slow job run, well past [`SETTLE`].
const WORK: Duration = Duration::from_millis(1500);

/// The child's process, killed if the test ends first.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Child {
    /// Start this test again as the child with `role`, serving on `port`.
    fn start(test: &str, role: &str, port: u16) -> Self {
        let binary = std::env::current_exe().unwrap();
        Self(
            Command::new(binary)
                .args([test, "--exact", "--nocapture", "--include-ignored"])
                .env(ROLE, role)
                .env(PORT, port.to_string())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    /// Send a real `SIGTERM`, as an orchestrator does.
    fn terminate(&self) {
        self.signal("-TERM");
    }

    /// Send a real signal, by `kill`'s name for it.
    fn signal(&self, name: &str) {
        let sent = Command::new("kill")
            .args([name, &self.0.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
    }

    /// How the child ended, failing the test if it has not by [`DEADLINE`].
    fn exit(&mut self) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(start.elapsed() < DEADLINE, "the child ignored SIGTERM");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Assert that the child stopped itself, rather than being killed by the signal.
    fn assert_exits_cleanly(&mut self) {
        let status = self.exit();
        assert_eq!(status.signal(), None, "killed by a signal: {status}");
        assert!(status.success(), "{status}");
    }

    /// The child's output, a line at a time, without blocking the test on it.
    fn lines(&mut self) -> mpsc::Receiver<String> {
        let stdout = self.0.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        lines
    }
}

/// A port that was free a moment ago, from below the range the system hands out for
/// port 0. The child binds it later, and in that gap another process asking for any
/// port could be given this one, which this test would then connect to and wait on.
fn free_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let base = 20_000 + u16::try_from(std::process::id() % 10_000).unwrap();
    loop {
        let port = base + NEXT.fetch_add(1, Ordering::Relaxed);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

/// Connect once the child is listening.
fn connect(port: u16) -> TcpStream {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let start = Instant::now();
    loop {
        match TcpStream::connect(address) {
            Ok(stream) => return stream,
            Err(error) => {
                assert!(start.elapsed() < DEADLINE, "never listened: {error}");
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn config() -> Server {
    Server {
        port: std::env::var(PORT).unwrap().parse().unwrap(),
        binding: "127.0.0.1".to_string(),
        host: String::new(),
        shutdown_timeout_seconds: 10,
        shutdown_drain_seconds: 0,
        header_read_timeout_seconds: 10,
        keep_alive_timeout_seconds: 0,
        send_stall_timeout_seconds: 0,
        max_connections: 10,
    }
}

/// Takes a while, and says when it has started.
async fn slow() -> &'static str {
    println!("{IN_HANDLER}");
    tokio::time::sleep(WORK).await;
    "finished"
}

fn router() -> Router {
    Router::new().route("/slow", get(slow))
}

/// The parent's side of a request test: a request must be running when `signal` arrives,
/// and must finish before the child exits cleanly.
fn request_outlives(test: &str, signal: &str) {
    let port = free_port();
    let mut child = Child::start(test, "server", port);
    let lines = child.lines();
    let mut stream = connect(port);
    stream.set_read_timeout(Some(DEADLINE)).unwrap();
    stream
        .write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    while lines.recv_timeout(DEADLINE).unwrap() != IN_HANDLER {}
    thread::sleep(SETTLE);

    child.signal(signal);

    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with("finished"), "{response}");
    child.assert_exits_cleanly();
}

#[tokio::test(flavor = "multi_thread")]
async fn sigterm_lets_a_request_in_flight_finish_and_exits_cleanly() {
    if std::env::var_os(ROLE).is_some() {
        server::run(Some(router()), None, &config()).await.unwrap();
        return;
    }
    request_outlives(
        "sigterm_lets_a_request_in_flight_finish_and_exits_cleanly",
        "-TERM",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_lets_a_request_in_flight_finish_and_exits_cleanly() {
    if std::env::var_os(ROLE).is_some() {
        server::run(Some(router()), None, &config()).await.unwrap();
        return;
    }
    request_outlives(
        "ctrl_c_lets_a_request_in_flight_finish_and_exits_cleanly",
        "-INT",
    );
}

/// Works for as long as its argument says.
struct SlowWorker;

#[async_trait]
impl BackgroundWorker<u64> for SlowWorker {
    fn build(_ctx: &AppContext) -> Self {
        Self
    }

    async fn perform(&self, millis: u64) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(millis)).await;
        Ok(())
    }
}

fn database() -> String {
    std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL")
}

/// Held for a whole test, because the job tests share one table.
static TABLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A queue that notices a job quickly.
async fn queue() -> Queue {
    let options = QueueOptions {
        poll_interval: Duration::from_millis(20),
        ..QueueOptions::default()
    };
    let mut queue = Queue::connect(&database(), options).await.unwrap();
    queue.register(SlowWorker);
    queue
}

async fn status_of(pool: &sqlx::PgPool, id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM axa_queue WHERE id::text = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The parent's side of a job test: a job must be running when `SIGTERM` arrives, and
/// must have completed by the time the child has exited.
async fn job_outlives_sigterm(test: &str, role: &str) {
    let _table = TABLE.lock().await;
    let pool = sqlx::PgPool::connect(&database()).await.unwrap();
    let queue = queue().await;
    sqlx::query("DELETE FROM axa_queue")
        .execute(&pool)
        .await
        .unwrap();
    let millis = u64::try_from(WORK.as_millis()).unwrap();
    let id = queue
        .enqueue("SlowWorker", serde_json::json!(millis))
        .await
        .unwrap();

    let mut child = Child::start(test, role, free_port());
    let start = Instant::now();
    while status_of(&pool, &id).await != "running" {
        assert!(start.elapsed() < DEADLINE, "the job never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(SETTLE).await;
    assert_eq!(status_of(&pool, &id).await, "running");

    child.terminate();

    child.assert_exits_cleanly();
    assert_eq!(status_of(&pool, &id).await, "completed");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn sigterm_lets_a_job_in_progress_finish_in_a_worker() {
    const TEST: &str = "sigterm_lets_a_job_in_progress_finish_in_a_worker";
    if std::env::var_os(ROLE).is_some() {
        let queue = Arc::new(queue().await);
        server::run(None, Some(queue), &config()).await.unwrap();
        return;
    }
    job_outlives_sigterm(TEST, "worker").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs AXA_TEST_DB_URL"]
async fn sigterm_lets_a_job_in_progress_finish_beside_a_server() {
    const TEST: &str = "sigterm_lets_a_job_in_progress_finish_beside_a_server";
    if std::env::var_os(ROLE).is_some() {
        let queue = Arc::new(queue().await);
        server::run(Some(router()), Some(queue), &config())
            .await
            .unwrap();
        return;
    }
    job_outlives_sigterm(TEST, "server and worker").await;
}
