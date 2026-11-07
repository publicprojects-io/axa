//! Finding what is wrong with asynchronous code while it runs.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use tokio::{runtime::Handle, task::JoinHandle};

/// A future being watched. See [`Watch::watch`].
#[must_use = "a future does nothing unless it is awaited"]
pub struct Watched<F> {
    future: Pin<Box<F>>,
    name: &'static str,
    threshold: Duration,
    started: Option<Instant>,
}

impl<F: Future> Future for Watched<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let started = *this.started.get_or_insert_with(Instant::now);
        let polled = Instant::now();
        let result = this.future.as_mut().poll(context);
        let poll_time = polled.elapsed();
        if poll_time >= this.threshold {
            tracing::warn!(
                name = this.name,
                poll_ms = millis(poll_time),
                "future_blocked_runtime",
            );
        }
        if result.is_ready() {
            let total = started.elapsed();
            if total >= this.threshold {
                tracing::warn!(name = this.name, total_ms = millis(total), "future_slow",);
            }
        }
        result
    }
}

/// Watches a future for the two ways it can be slow.
pub trait Watch: Future + Sized {
    /// Logs a warning if this future takes `threshold` or longer.
    ///
    /// `future_blocked_runtime` means one poll took that long: it held a runtime
    /// thread, and other tasks waited behind it, which usually means blocking code
    /// that belongs in `spawn_blocking`. `future_slow` means it took that long in
    /// all, which is often only a slow peer.
    ///
    /// The future gets a little slower, by two clock reads each poll, and one
    /// allocation. `name` is fixed text, so that no value reaches the logs by way
    /// of it.
    fn watch(self, name: &'static str, threshold: Duration) -> Watched<Self> {
        Watched {
            future: Box::pin(self),
            name,
            threshold,
            started: None,
        }
    }
}

impl<F: Future> Watch for F {}

/// What the Tokio runtime is doing at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    /// Threads running tasks.
    pub workers: usize,
    /// Tasks not yet finished.
    pub alive_tasks: usize,
    /// Tasks waiting in the shared queue. A number that keeps growing means the
    /// runtime is falling behind.
    pub queue_depth: usize,
}

/// A snapshot of the current Tokio runtime.
///
/// # Panics
///
/// Outside a Tokio runtime.
#[must_use]
pub fn runtime_snapshot() -> RuntimeSnapshot {
    snapshot_of(&Handle::current())
}

fn snapshot_of(handle: &Handle) -> RuntimeSnapshot {
    let metrics = handle.metrics();
    RuntimeSnapshot {
        workers: metrics.num_workers(),
        alive_tasks: metrics.num_alive_tasks(),
        queue_depth: metrics.global_queue_depth(),
    }
}

/// Settings for [`monitor_runtime`].
#[derive(Debug, Clone, Copy)]
pub struct RuntimeMonitor {
    /// How often to check.
    pub interval: Duration,
    /// How late a check may wake before it counts as a stall.
    pub max_lag: Duration,
}

impl Default for RuntimeMonitor {
    /// Check each second, and allow a check to wake 100 ms late.
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            max_lag: Duration::from_millis(100),
        }
    }
}

/// Starts a task that notices when the runtime stalls.
///
/// The task sleeps, and measures how late it wakes. A task that wakes late could not
/// get a thread, so neither could the service's other tasks. It then logs a warning,
/// `runtime_lagging`, with a [`RuntimeSnapshot`].
///
/// Dropping the handle leaves it running. Call `abort` on the handle to stop it, and
/// it stops with the runtime in any case.
///
/// # Panics
///
/// Outside a Tokio runtime.
#[must_use = "keep the handle to be able to stop the monitor"]
pub fn monitor_runtime(monitor: RuntimeMonitor) -> JoinHandle<()> {
    let handle = Handle::current();
    tokio::spawn(async move {
        loop {
            let expected = Instant::now() + monitor.interval;
            tokio::time::sleep(monitor.interval).await;
            let lag = Instant::now().saturating_duration_since(expected);
            if lag >= monitor.max_lag {
                let snapshot = snapshot_of(&handle);
                tracing::warn!(
                    lag_ms = millis(lag),
                    workers = snapshot.workers,
                    alive_tasks = snapshot.alive_tasks,
                    queue_depth = snapshot.queue_depth,
                    "runtime_lagging",
                );
            }
        }
    })
}

/// Logs panics as errors, replacing the panic hook that writes to stderr.
///
/// A panic in a spawned task otherwise reaches only stderr, outside the service's
/// log format and without its request id. The line names the source location and the
/// thread, and carries the span that panicked. The panic's own text is left out, as
/// it can quote anything, and so is a backtrace.
///
/// Call it once, from `main`. It replaces any earlier hook. Under `axa`, a panic in a
/// handler then leaves two lines: this one, which says where, and `handler_panicked`.
pub fn log_panics() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()));
        tracing::error!(
            location = location.as_deref().unwrap_or("unknown"),
            thread = std::thread::current().name().unwrap_or("unnamed"),
            request_id = crate::current_request_id().as_deref().unwrap_or(""),
            "panicked",
        );
    }));
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::test_support::Lines;

    fn capture() -> (Lines, tracing::subscriber::DefaultGuard) {
        let lines = Lines::default();
        let subscriber = tracing_subscriber::registry().with(lines.layer());
        let guard = tracing::subscriber::set_default(subscriber);
        (lines, guard)
    }

    /// A panic hook is process-wide, so its test must not race another panic.
    fn panic_hook_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[tokio::test]
    async fn a_blocking_future_is_reported() {
        let (lines, _guard) = capture();
        let value = async {
            std::thread::sleep(Duration::from_millis(10));
            7
        }
        .watch("blocker", Duration::from_millis(5))
        .await;
        let text = lines.text();
        assert_eq!(value, 7);
        assert!(text.contains("future_blocked_runtime"), "{text}");
        assert!(text.contains("blocker"), "{text}");
    }

    #[tokio::test]
    async fn a_future_that_only_waits_is_slow_but_not_blocking() {
        let (lines, _guard) = capture();
        tokio::time::sleep(Duration::from_millis(30))
            .watch("waiter", Duration::from_millis(10))
            .await;
        let text = lines.text();
        assert!(text.contains("future_slow"), "{text}");
        assert!(!text.contains("future_blocked_runtime"), "{text}");
    }

    #[tokio::test]
    async fn a_quick_future_is_not_reported() {
        let (lines, _guard) = capture();
        async {}.watch("quick", Duration::from_hours(1)).await;
        assert_eq!(lines.text(), "");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_snapshot_describes_the_runtime() {
        let snapshot = runtime_snapshot();
        assert_eq!(snapshot.workers, 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_stalled_runtime_is_reported() {
        let (lines, _guard) = capture();
        let monitor = monitor_runtime(RuntimeMonitor {
            interval: Duration::from_millis(5),
            max_lag: Duration::from_millis(10),
        });
        // Let the monitor start its sleep, then hold the only thread past it.
        tokio::time::sleep(Duration::from_millis(1)).await;
        std::thread::sleep(Duration::from_millis(30));
        // The monitor and this test wake together, in either order, so wait for the
        // warning rather than for a fixed time.
        let until = Instant::now() + Duration::from_secs(5);
        while !lines.text().contains("runtime_lagging") && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        monitor.abort();
        let text = lines.text();
        assert!(text.contains("runtime_lagging"), "{text}");
        assert!(text.contains("workers=1"), "{text}");
    }

    #[test]
    fn a_panic_is_logged_without_its_text() {
        let _lock = panic_hook_lock();
        let previous = std::panic::take_hook();
        log_panics();
        let (lines, guard) = capture();
        let result = std::panic::catch_unwind(|| panic!("the secret is hunter2"));
        drop(guard);
        std::panic::set_hook(previous);

        let text = lines.text();
        assert!(result.is_err());
        assert!(text.contains("panicked"), "{text}");
        assert!(text.contains("diagnostics.rs"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }
}
