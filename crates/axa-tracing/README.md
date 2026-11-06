# axa-tracing

Redaction, request tracking, and asynchronous diagnostics over `tracing`, for Axum
services.

[`axa`](https://crates.io/crates/axa) already logs each request. This crate adds what
that span cannot reach by itself:

- **Structured logging:** `Redacted` keeps a value out of a log field, and `SlowSpans`
  reports spans that took too long, split into busy and idle time.
- **Request tracking:** `track` and `current_request_id` give the request id to any
  code serving the request, `propagate` sends it to a peer service, and `spawn` keeps
  it, and the span, in a spawned task.
- **Asynchronous diagnostics:** `Watch::watch` catches a future that blocks the
  runtime, `monitor_runtime` notices a stalled runtime, and `log_panics` brings panics
  into the logs.

None of it needs `tokio_unstable`, so a service can run it in production.

Use it alone, or through `axa` with the `tracing` feature, as `axa::tracing`.

## License

MIT, Apache-2.0, BSD-3-Clause, GPL-2.0, or GPL-3.0, at your option.
