//! Tokio runtime and task metrics, behind the `tokio` feature.
//!
//! [`TokioMetrics`] reads the runtime at each scrape, and counts the polls of each task a
//! service instruments by name:
//!
//! ```
//! use axa_prometheus_metrics::{Registry, render, runtime::TokioMetrics};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let registry = Registry::new();
//! let tokio_metrics = TokioMetrics::register(&registry, &tokio::runtime::Handle::current()).unwrap();
//! let queue = tokio_metrics.task("queue");
//! tokio::spawn(queue.instrument(async { /* the queue's loop */ })).await.unwrap();
//! assert!(render(&registry).contains(r#"tokio_task_polls_total{task="queue"}"#));
//! # }
//! ```
//!
//! Only the runtime metrics that stable Tokio offers are read: a build with
//! `--cfg tokio_unstable` reports the same ones.

use prometheus::{
    Counter, CounterVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    core::{Collector, Desc},
    proto::MetricFamily,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
pub use tokio_metrics::TaskMonitor;
use tokio_metrics::{RuntimeIntervals, RuntimeMonitor, TaskMetrics};

/// Tokio runtime metrics, and the metrics of each task instrumented through
/// [`TokioMetrics::task`]. It is one collector in the registry, read at each scrape.
///
/// | Metric | Type | What it is |
/// |---|---|---|
/// | `tokio_runtime_workers` | gauge | Worker threads |
/// | `tokio_runtime_alive_tasks` | gauge | Tasks spawned and not yet finished |
/// | `tokio_runtime_global_queue_depth` | gauge | Tasks waiting in the shared queue |
/// | `tokio_runtime_worker_parks_total` | counter | Times a worker parked, finding no work |
/// | `tokio_runtime_worker_busy_seconds_total` | counter | Time workers spent running tasks |
/// | `tokio_task_instrumented_total{task}` | counter | Tasks instrumented under the name |
/// | `tokio_task_dropped_total{task}` | counter | Those tasks finished or dropped |
/// | `tokio_task_polls_total{task}` | counter | Polls of those tasks |
/// | `tokio_task_slow_polls_total{task}` | counter | Polls that took 50 microseconds or more |
/// | `tokio_task_poll_seconds_total{task}` | counter | Time spent polling them |
/// | `tokio_task_scheduled_seconds_total{task}` | counter | Time they waited to run once woken |
/// | `tokio_task_idle_seconds_total{task}` | counter | Time they waited to be woken |
///
/// Time scheduled but not running is the sign of a starved runtime: a blocking call in
/// one task holds up every other task on its worker.
#[derive(Clone)]
pub struct TokioMetrics {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for TokioMetrics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tasks = lock(&self.inner.tasks);
        formatter
            .debug_struct("TokioMetrics")
            .field("tasks", &tasks.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

struct Inner {
    runtime: Mutex<RuntimeIntervals>,
    tasks: Mutex<BTreeMap<&'static str, Task>>,
    workers: IntGauge,
    alive_tasks: IntGauge,
    global_queue_depth: IntGauge,
    parks: IntCounter,
    busy: Counter,
    instrumented: IntCounterVec,
    dropped: IntCounterVec,
    polls: IntCounterVec,
    slow_polls: IntCounterVec,
    poll_seconds: CounterVec,
    scheduled_seconds: CounterVec,
    idle_seconds: CounterVec,
}

/// A task's monitor, and the totals already reported, since a counter only grows by
/// what changed.
struct Task {
    monitor: TaskMonitor,
    reported: TaskMetrics,
}

impl TokioMetrics {
    /// Make the collector for `runtime`, and register it in `registry`.
    ///
    /// # Errors
    ///
    /// Fails when `registry` already holds a metric of one of these names.
    pub fn register(
        registry: &Registry,
        runtime: &tokio::runtime::Handle,
    ) -> prometheus::Result<Self> {
        let task = |name: &str, help: &str| IntCounterVec::new(Opts::new(name, help), &["task"]);
        let task_seconds =
            |name: &str, help: &str| CounterVec::new(Opts::new(name, help), &["task"]);
        let inner = Inner {
            runtime: Mutex::new(RuntimeMonitor::new(runtime).intervals()),
            tasks: Mutex::new(BTreeMap::new()),
            workers: IntGauge::new("tokio_runtime_workers", "Tokio worker threads.")?,
            alive_tasks: IntGauge::new(
                "tokio_runtime_alive_tasks",
                "Tokio tasks spawned and not yet finished.",
            )?,
            global_queue_depth: IntGauge::new(
                "tokio_runtime_global_queue_depth",
                "Tokio tasks waiting in the shared queue.",
            )?,
            parks: IntCounter::new(
                "tokio_runtime_worker_parks_total",
                "Times a Tokio worker parked, finding no work.",
            )?,
            busy: Counter::new(
                "tokio_runtime_worker_busy_seconds_total",
                "Time Tokio workers spent running tasks.",
            )?,
            instrumented: task("tokio_task_instrumented_total", "Tasks instrumented.")?,
            dropped: task(
                "tokio_task_dropped_total",
                "Instrumented tasks finished or dropped.",
            )?,
            polls: task("tokio_task_polls_total", "Polls of instrumented tasks.")?,
            slow_polls: task(
                "tokio_task_slow_polls_total",
                "Polls of instrumented tasks that took 50 microseconds or more.",
            )?,
            poll_seconds: task_seconds(
                "tokio_task_poll_seconds_total",
                "Time spent polling instrumented tasks.",
            )?,
            scheduled_seconds: task_seconds(
                "tokio_task_scheduled_seconds_total",
                "Time instrumented tasks waited to run once woken.",
            )?,
            idle_seconds: task_seconds(
                "tokio_task_idle_seconds_total",
                "Time instrumented tasks waited to be woken.",
            )?,
        };
        let metrics = Self {
            inner: Arc::new(inner),
        };
        registry.register(Box::new(metrics.clone()))?;
        Ok(metrics)
    }

    /// The monitor for tasks named `name`: wrap a future in [`TaskMonitor::instrument`]
    /// before spawning it. The same name gives the same monitor, so its tasks add up.
    ///
    /// The name is a label, so it is `'static`, written in the code: never a value from a
    /// request, which would let a client make labels without end.
    #[must_use]
    pub fn task(&self, name: &'static str) -> TaskMonitor {
        let mut tasks = lock(&self.inner.tasks);
        tasks
            .entry(name)
            .or_insert_with(|| Task {
                monitor: TaskMonitor::new(),
                reported: TaskMetrics::default(),
            })
            .monitor
            .clone()
    }

    /// Bring every metric up to date.
    fn update(&self) {
        let inner = &self.inner;
        if let Some(runtime) = lock(&inner.runtime).next() {
            inner.workers.set(gauge(runtime.workers_count));
            inner.alive_tasks.set(gauge(runtime.live_tasks_count));
            inner
                .global_queue_depth
                .set(gauge(runtime.global_queue_depth));
            inner.parks.inc_by(runtime.total_park_count);
            inner.busy.inc_by(runtime.total_busy_duration.as_secs_f64());
        }
        for (name, task) in lock(&inner.tasks).iter_mut() {
            let now = task.monitor.cumulative();
            let was = &task.reported;
            let count = |metric: &IntCounterVec, now: u64, was: u64| {
                // A counter that went backwards is unknown, so it adds nothing.
                if let Some(added) = now.checked_sub(was) {
                    metric.with_label_values(&[name]).inc_by(added);
                }
            };
            let seconds = |metric: &CounterVec, now: Duration, was: Duration| {
                if let Some(added) = now.checked_sub(was) {
                    metric
                        .with_label_values(&[name])
                        .inc_by(added.as_secs_f64());
                }
            };
            count(
                &inner.instrumented,
                now.instrumented_count,
                was.instrumented_count,
            );
            count(&inner.dropped, now.dropped_count, was.dropped_count);
            count(&inner.polls, now.total_poll_count, was.total_poll_count);
            count(
                &inner.slow_polls,
                now.total_slow_poll_count,
                was.total_slow_poll_count,
            );
            seconds(
                &inner.poll_seconds,
                now.total_poll_duration,
                was.total_poll_duration,
            );
            seconds(
                &inner.scheduled_seconds,
                now.total_scheduled_duration,
                was.total_scheduled_duration,
            );
            seconds(
                &inner.idle_seconds,
                now.total_idle_duration,
                was.total_idle_duration,
            );
            task.reported = now;
        }
    }

    fn parts(&self) -> [&dyn Collector; 12] {
        let inner = &self.inner;
        [
            &inner.workers,
            &inner.alive_tasks,
            &inner.global_queue_depth,
            &inner.parks,
            &inner.busy,
            &inner.instrumented,
            &inner.dropped,
            &inner.polls,
            &inner.slow_polls,
            &inner.poll_seconds,
            &inner.scheduled_seconds,
            &inner.idle_seconds,
        ]
    }
}

impl Collector for TokioMetrics {
    fn desc(&self) -> Vec<&Desc> {
        self.parts().into_iter().flat_map(Collector::desc).collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        self.update();
        self.parts()
            .into_iter()
            .flat_map(Collector::collect)
            .collect()
    }
}

/// A count as a gauge's value, saturating rather than wrapping.
fn gauge(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Lock, carrying on after a panic elsewhere: these are counts, and a scrape must not
/// fail for one.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render;

    /// The sample on the line that starts `line`.
    fn sample<'a>(text: &'a str, line: &str) -> &'a str {
        text.lines()
            .find(|row| row.starts_with(&format!("{line} ")))
            .unwrap_or_else(|| panic!("no {line} in\n{text}"))
            .rsplit(' ')
            .next()
            .unwrap()
    }

    fn seconds(text: &str, line: &str) -> f64 {
        sample(text, line).parse().unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_runtime_is_read_at_each_scrape() {
        let registry = Registry::new();
        let metrics =
            TokioMetrics::register(&registry, &tokio::runtime::Handle::current()).unwrap();
        // Each family a scrape gathers is one the collector describes.
        let described: Vec<String> = metrics
            .desc()
            .iter()
            .map(|desc| desc.fq_name.clone())
            .collect();
        for family in registry.gather() {
            assert!(
                described.contains(&family.name().to_string()),
                "{described:?}"
            );
        }
        let text = render(&registry);
        assert_eq!(sample(&text, "tokio_runtime_workers"), "2");
        assert!(
            sample(&text, "tokio_runtime_alive_tasks")
                .parse::<u64>()
                .is_ok()
        );
        assert!(
            sample(&text, "tokio_runtime_global_queue_depth")
                .parse::<u64>()
                .is_ok()
        );
        assert!(
            text.contains("# TYPE tokio_runtime_worker_parks_total counter"),
            "{text}"
        );
        assert!(text.contains("# TYPE tokio_runtime_worker_busy_seconds_total counter"));
        let busy = seconds(&text, "tokio_runtime_worker_busy_seconds_total");
        for _ in 0_i32..10_i32 {
            tokio::spawn(async {
                std::hint::black_box((0..10_000_u64).filter(|n| n % 3 == 0).count())
            })
            .await
            .unwrap();
        }
        let later = render(&registry);
        assert!(seconds(&later, "tokio_runtime_worker_busy_seconds_total") >= busy);
    }

    #[tokio::test]
    async fn tasks_of_one_name_add_up_under_one_label() {
        let registry = Registry::new();
        let metrics =
            TokioMetrics::register(&registry, &tokio::runtime::Handle::current()).unwrap();
        for _ in 0_i32..3_i32 {
            let monitor = metrics.task("queue");
            tokio::spawn(monitor.instrument(async { tokio::task::yield_now().await }))
                .await
                .unwrap();
        }
        let text = render(&registry);
        let queue = |text, name| sample(text, &format!(r#"tokio_task_{name}{{task="queue"}}"#));
        assert_eq!(queue(&text, "instrumented_total"), "3");
        assert_eq!(queue(&text, "dropped_total"), "3");
        let polls = queue(&text, "polls_total");
        assert!(polls.parse::<u64>().unwrap() >= 6, "{text}");
        // A second scrape reports the same totals, not twice them.
        let again = render(&registry);
        assert_eq!(queue(&again, "polls_total"), polls);
        assert_eq!(queue(&again, "instrumented_total"), "3");
    }

    #[tokio::test]
    async fn registering_twice_in_one_registry_fails() {
        let registry = Registry::new();
        let runtime = tokio::runtime::Handle::current();
        assert!(TokioMetrics::register(&registry, &runtime).is_ok());
        assert!(TokioMetrics::register(&registry, &runtime).is_err());
    }
}
