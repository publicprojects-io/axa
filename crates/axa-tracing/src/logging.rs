//! Log fields that hide their value, and a layer that reports slow spans.

use std::{
    fmt,
    time::{Duration, Instant},
};

use tracing::{Subscriber, span};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// A log field that never prints its value, in `Display` or `Debug`.
///
/// Use it for a token, a key, or personal data, so that a later change to the log
/// format, or a derived `Debug` on the struct around it, cannot print the value.
///
/// ```
/// use axa_tracing::Redacted;
///
/// assert_eq!(format!("{}", Redacted("secret")), "[redacted]");
/// assert_eq!(format!("{:?}", Redacted("secret")), "[redacted]");
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Redacted<T>(pub T);

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// How one span spent its time so far.
struct Timing {
    created: Instant,
    entered: Option<Instant>,
    busy: Duration,
}

/// Logs a warning, `slow_span`, for each span that lasts `threshold` or longer.
///
/// The warning names the span and how long it took, split into `busy_ms`, while its
/// code was running, and `idle_ms`, while it was waiting. It is written inside the
/// closed span, so it carries that span's fields, including a request id.
///
/// A large `idle_ms` means something it awaited was slow. A large `busy_ms` in an
/// async span means it held a runtime thread for that long.
#[derive(Debug, Clone, Copy)]
pub struct SlowSpans {
    threshold: Duration,
}

impl SlowSpans {
    /// Report spans lasting `threshold` or longer.
    #[must_use]
    pub const fn new(threshold: Duration) -> Self {
        Self { threshold }
    }
}

impl<S> Layer<S> for SlowSpans
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, _attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(Timing {
                created: Instant::now(),
                entered: None,
                busy: Duration::ZERO,
            });
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(timing) = span.extensions_mut().get_mut::<Timing>()
        {
            // A span entered twice at once counts from its first entry.
            timing.entered.get_or_insert_with(Instant::now);
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(timing) = span.extensions_mut().get_mut::<Timing>()
            && let Some(entered) = timing.entered.take()
        {
            timing.busy += entered.elapsed();
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let report = span.extensions().get::<Timing>().and_then(|timing| {
            let total = timing.created.elapsed();
            (total >= self.threshold).then_some((total, timing.busy))
        });
        if let Some((total, busy)) = report {
            tracing::warn!(
                parent: &id,
                span = span.name(),
                busy_ms = millis(busy),
                idle_ms = millis(total.saturating_sub(busy)),
                "slow_span",
            );
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::test_support::Lines;

    fn logged(threshold: Duration, body: impl FnOnce()) -> String {
        let lines = Lines::default();
        let subscriber = tracing_subscriber::registry()
            .with(SlowSpans::new(threshold))
            .with(lines.layer());
        tracing::subscriber::with_default(subscriber, body);
        lines.text()
    }

    #[test]
    fn redacted_hides_its_value_in_a_log_line() {
        let text = logged(Duration::from_hours(1), || {
            tracing::info!(token = %Redacted("hunter2"), other = ?Redacted("hunter3"), "signing in");
        });
        assert!(text.contains("[redacted]"), "{text}");
        assert!(!text.contains("hunter"), "{text}");
    }

    #[test]
    fn a_slow_span_is_reported_with_its_fields() {
        let text = logged(Duration::from_millis(5), || {
            let span = tracing::info_span!("load", request_id = "abc");
            let _entered = span.enter();
            sleep(Duration::from_millis(10));
        });
        assert!(text.contains("slow_span"), "{text}");
        assert!(text.contains("span=\"load\""), "{text}");
        assert!(text.contains("request_id"), "{text}");
    }

    #[test]
    fn a_quick_span_is_not_reported() {
        let text = logged(Duration::from_hours(1), || {
            let span = tracing::info_span!("load");
            let _entered = span.enter();
        });
        assert!(!text.contains("slow_span"), "{text}");
    }

    #[test]
    fn busy_and_idle_time_are_told_apart() {
        let text = logged(Duration::from_millis(30), || {
            let span = tracing::info_span!("load");
            {
                let _entered = span.enter();
                sleep(Duration::from_millis(5));
            }
            sleep(Duration::from_millis(40));
        });
        let number = |name: &str| -> u64 {
            let at = text.find(name).expect("field is logged") + name.len() + 1;
            let digits: String = text[at..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().unwrap()
        };
        assert!(number("idle_ms") >= 30, "{text}");
        assert!(number("busy_ms") < number("idle_ms"), "{text}");
    }
}
