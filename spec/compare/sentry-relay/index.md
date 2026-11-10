# Sentry Relay

Relay is Sentry's ingestion proxy. SDKs send it events, envelopes, attachments,
minidumps, and security reports, and it validates, rate-limits, buffers, and forwards
them upstream to Sentry or Kafka. Its repository is
[github.com/getsentry/relay](https://github.com/getsentry/relay). I read the `master`
branch at commit
[`0d47301`](https://github.com/getsentry/relay/tree/0d47301d73ae3f7a1441d058b7786a24eb3130c0)
of 10 November 2026. It has about 400 stars, is licensed FSL-1.1-Apache-2.0, and is a Cargo workspace
of many crates. This study looks only at `relay-server`, about 71,000 lines of Rust.
It builds an Axum 0.8 `Router` and serves it with `axum-server` 0.8 rather than
`axum::serve`, with its own acceptor and a connection limit wrapped around the make-service.
It uses `tower-http` for compression, request decompression, body limits, CORS, and panic
catching. Its work behind the handlers runs as actor-style services from its own
`relay-system` crate, not inside the request. This study looks at backpressure, load
shedding, request body handling, health, and metrics.

> **Since this was written**, axa 0.11 added the body limit binds a handler that reads the body itself; `RequestMetrics` measures requests by route template. Those rows below are marked "Done in 0.11", and the rest stand as written.

Links below are to files at that commit, under
`https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/`.

[server]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/services/server/mod.rs
[acceptor]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/services/server/acceptor.rs
[climit]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/services/server/concurrency_limit.rs
[endpoints]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/endpoints/mod.rs
[common]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/endpoints/common.rs
[envelope]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/endpoints/envelope.rs
[attachments]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/endpoints/attachments.rs
[minidump]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/endpoints/minidump.rs
[forward]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/endpoints/forward.rs
[decomp]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/middlewares/decompression.rs
[decompmetrics]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/utils/stream/decompression_metrics.rs
[zipbomb]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/tests/integration/test_basic.py
[metrics]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/middlewares/metrics.rs
[clen]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/middlewares/content_length.rs
[panic]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/middlewares/handle_panic.rs
[normalize]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/middlewares/normalize_path.rs
[api]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/utils/api.rs
[bounded]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/utils/stream/bounded.rs
[memory]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/utils/memory.rs
[buffer]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/services/buffer/mod.rs
[health]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/services/health_check.rs
[healthep]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/endpoints/health_check.rs
[autoscaling]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/endpoints/autoscaling.rs
[fwdfor]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-server/src/extractors/forwarded_for.rs
[config]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-config/src/config.rs
[cli]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay/src/cli.rs
[controller]: https://github.com/getsentry/relay/blob/0d47301d73ae3f7a1441d058b7786a24eb3130c0/relay-system/src/controller.rs

## How it is built

### Server and connections

[`services/server/mod.rs`][server] binds the socket itself with `SO_REUSEPORT` and a
configured listen backlog (1,024 by default, in [`relay-config/src/config.rs`][config]).
It sets a fixed 5-second header read timeout, HTTP/1 half-close and `writev`, and an
HTTP/2 keep-alive timeout. [`acceptor.rs`][acceptor] sets `TCP_NODELAY`, TCP keep-alive
probes (five retries), and an optional idle timeout on every accepted stream.
[`concurrency_limit.rs`][climit] is a semaphore around the make-service, so it caps open
connections, and a connection over the cap waits for a permit rather than being refused.
The cap is off by default (`max_connections: None`). A periodic task reports the
number of open connections as a gauge.

When `listen_addr_internal` is set, Relay serves two listeners: public routes on one,
and health and autoscaling routes on the other ([`endpoints/mod.rs`][endpoints]). Without
it, both sets share one port.

### Middleware

The stack in [`make_app`][server], outside in: request metrics, panic catching, a fixed
`server` header and `cross-origin-resource-policy: cross-origin`, Sentry's own Tower
layers for error capture and transactions, a `tower-http` trace layer, a map that drops
an empty or `UTF-8` `content-encoding` (sent by old Ruby and Android SDKs), and response
compression above 128 bytes. A `NormalizePath` wrapper outside the router folds repeated
slashes, so `//api///` routes as `/api/` ([`normalize_path.rs`][normalize]). CORS is a
route layer on the ingestion routes only. I found no request id layer.

### Request bodies

Each route group sets its own limit with `DefaultBodyLimit`: 256 KiB for the Sentry web
API, 50 MB for internal metric batches, and `max_envelope_size` for envelopes
([`endpoints/mod.rs`][endpoints], [`envelope.rs`][envelope]). Routes that stream their body,
such as attachments, minidumps, and the forwarding fallback, turn `DefaultBodyLimit`
off and use `tower-http`'s `RequestBodyLimitLayer`, with a comment that only the latter
checks a stream ([`attachments.rs`][attachments], [`minidump.rs`][minidump],
[`forward.rs`][forward]).

[`decompression.rs`][decomp] wraps routes in `tower-http`'s `RequestDecompression`, so a
gzip, deflate, Brotli, or Zstd body is decoded before the handler reads it, and the body
limit is counted on decoded bytes. An integration test posts a small gzip file that
expands to 10 GB and expects `413` on four routes (`test_zipbomb_content_encoding` in
[`tests/integration/test_basic.py`][zipbomb]). It also counts bytes before and after
decoding and reports the ratio per route ([`decompression_metrics.rs`][decompmetrics]).
[`utils/stream/bounded.rs`][bounded] wraps an upload stream and fails it if it sends more
than an upper bound or fewer than a lower bound, as when the body is shorter than its
declared length.

### Backpressure and load shedding

Relay sheds load at the hand-off from handler to background work, not at the socket.
[`endpoints/common.rs`][common] does two checks before it queues an envelope:

- If process memory is over its watermark, it refuses with `503`
  (`PushError::OutOfMemory`).
- It calls `try_push` on a partitioned envelope buffer. `try_push` never waits: if the
  buffer has no capacity, or is shutting down, the request gets `503`
  ([`services/buffer/mod.rs`][buffer]).

Capacity is held in atomics that the buffer service updates, so a handler reads it
without sending a message. The buffer also stops dequeuing while memory use is above
`spool_max_backpressure_memory_percent`. Memory is the process RSS from `sysinfo`,
refreshed at most every so many milliseconds and read lock-free through `ArcSwap`
([`utils/memory.rs`][memory]).

A rate-limited request gets `429` with `Retry-After` and Relay's own
`X-Sentry-Rate-Limits` header. Oversized items get `413`. Rejected events get `403`.
Other client errors get `400`. A `503` is logged at warning level, other 5xx at error,
and 4xx at debug ([`endpoints/common.rs`][common]).

### Errors

Errors are JSON `{"detail": ..., "causes": [...]}`, built from the error's `Display`
text and its whole `source()` chain ([`utils/api.rs`][api]). The panic handler sends the
panic message to the client as `detail` ([`handle_panic.rs`][panic]).

### Health

[`services/health_check.rs`][health] runs readiness in a background loop at
`health_refresh_interval`, and handlers read the last answer from a `watch` channel.
Readiness asks four probes at once, each under `health_probe_timeout`: memory below its
watermarks (percent and bytes), authenticated with the upstream (when auth is
required), the metrics aggregator accepting, and the envelope buffer having capacity.
An answer older than the interval plus the probe timeout, times 1.1, counts as
unhealthy. Liveness always answers healthy. On shutdown the loop publishes unhealthy.
The routes are `/api/relay/healthcheck/{live|ready}/`, answering `200` or `503` with
`{"is_healthy": bool}` ([`endpoints/health_check.rs`][healthep]). There is no startup
probe.

### Metrics

Metrics go to StatsD, not Prometheus. [`middlewares/metrics.rs`][metrics] counts each
request by route template, method, and `content-encoding`, times it, counts responses
by status, and measures CPU time per request for cost attribution.
[`content_length.rs`][clen] records the declared `content-length` as a distribution on
some routes. `/api/relay/autoscaling/` serves memory use, buffer size, per-service
utilisation, and worker pool and runtime utilisation as Prometheus text for an
autoscaler ([`endpoints/autoscaling.rs`][autoscaling]).

### Configuration and shutdown

Configuration is a YAML `config.yml` in a config folder, with some values overridden by
command-line flags or `RELAY_*` variables ([`relay/src/cli.rs`][cli],
[`relay-config/src/config.rs`][config]). Shutdown is coordinated by a `Controller` that
waits up to `shutdown_timeout` (10 seconds by default) and then forces it
([`relay-system/src/controller.rs`][controller]). The HTTP server then calls
`graceful_shutdown` with that timeout.

### Client address

[`extractors/forwarded_for.rs`][fwdfor] takes the first non-empty value from
`CF-Connecting-IP`, `X-Vercel-Forwarded-For`, `X-Sentry-Forwarded-For`, or
`X-Forwarded-For`, in that order, with no check of who sent it.

### Testing

Most behaviour is tested from Python: `tests/integration` holds about 70 pytest files
that start a real Relay binary against a mock Sentry. I did not study the Rust unit
tests.

## Compared with axa

| Area | Relay | axa |
|---|---|---|
| Server | `axum-server`, own acceptor, `SO_REUSEPORT`, backlog, TCP keep-alive, idle timeout | Own hyper loop with header read, keep-alive, and send-stall timeouts |
| Connection cap | Semaphore on the make-service, off by default, extra connections wait | `MAX_CONNECTIONS`, 10,000 by default, extra connections wait |
| Header read timeout | Fixed at 5 seconds | `HTTP_REQUEST_HEADER_READ_TIMEOUT_SECONDS`, 30 by default |
| Request timeout | None found | `REQUEST_TIMEOUT_SECONDS`, per route and per group |
| Body limit for extractors | `DefaultBodyLimit` per route group | `DefaultBodyLimit` in the stack, `middleware::body_limit` per route |
| Body limit for streams | `RequestBodyLimitLayer` on streaming routes | None: a handler reading `Body` directly is not limited |
| Request decompression | gzip, deflate, Brotli, Zstd, limit on decoded size, zip bomb test | None |
| Load shedding | `503` when memory is high or the work queue is full | None beyond the connection cap and rate limits |
| Rate limits | Per project, from upstream config, `429` with `Retry-After` | `axa-request-rate-limit`, in-process, with `Retry-After` |
| Errors | `detail` plus the whole cause chain, panic text sent | RFC 9457 problems, internal text never sent |
| Health | Live and ready, readiness computed in the background, stale answer fails | Live, ready, and startup, answers reused for a few seconds, one round at a time |
| Health ports | Optional internal listener | Same port as the API |
| Request metrics | StatsD: count, duration, status by route template, content length, decompression ratio, open connections | OTLP request metric in `axa-opentelemetry`. `axa-prometheus-metrics` renders a registry but records no request metrics |
| Client address | Trusts four headers from anyone | `TRUSTED_PROXIES` only, never trusts the first `X-Forwarded-For` entry |
| Request id | None found | Kept or minted, always returned |
| Path normalisation | Folds repeated slashes | None |
| Configuration | YAML file plus some `RELAY_*` overrides | Environment only, production refuses unsafe values |
| Shutdown | Controller with timeout, then forced | `SHUTDOWN_TIMEOUT_SECONDS`, requests and jobs finish first |
| Testing | Python integration tests against a binary | In-process request helpers in `axa-test` |

## What axa could add

| Capability | Why it matters | Effort | Priority |
|---|---|---|---|
| Request body decompression with the limit on decoded bytes | Clients and SDKs send gzip bodies, and a naive decoder is a zip bomb | S | High |
| Body limit that also covers streamed bodies | `DefaultBodyLimit` binds extractors only, so a handler that reads `Body` has no limit | S | Done in 0.11 (T-4, MID-7a) |
| Prometheus request metrics layer | Count and duration by route template, method, and status is what every service graphs first | S | Done in 0.11 (T-10, PRM-2) |
| Overload refusal: a bounded hand-off and a standard `503` | A service that queues background work should refuse fast when full, not grow without bound | S | Medium |
| Memory watermark check and refusal | Lets a pod shed load and leave the Service before the kernel kills it | M | Medium |
| Optional internal listener for health and metrics | Keeps probes and scrapes off the public port, so neither can be reached or loaded from outside | M | Medium |
| Open connections gauge | Shows when `MAX_CONNECTIONS` is near | S | Low |
| TCP keep-alive, `TCP_NODELAY`, listen backlog settings | Detects dead peers behind some load balancers, tunes accept queues | S | Low |
| Repeated-slash path normalisation | Saves a `404` for `//api/x` from hand-written clients | S | Low |
| Exact-length upload stream | Catches truncated uploads before they are stored | S | Low |

**Request body decompression.** Add it to `axa::middleware::apply` behind
`HTTP_REQUEST_DECOMPRESSION`, off by default, using `tower-http`'s `RequestDecompression`
(the `decompression-full` feature of a crate axa already depends on). Put it outside the
body limit layer, so that `DefaultBodyLimit`, and the streamed limit below, count
decoded bytes and a small bomb still gets `413`. An unknown `content-encoding` should
get `415` as a problem. Port Relay's zip bomb test as a requirement, with a small fixture
generated in the test rather than a checked-in file.

**Body limit for streams.** Add `tower-http`'s `RequestBodyLimitLayer` (the `limit`
feature) next to `DefaultBodyLimit` in `apply`, at the same
`HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, and make `middleware::body_limit` set both, so a
route raised for uploads is raised for both kinds of reader. A declared `content-length`
over the limit is then refused before the body is read. The `413` should go through
`shape_refusals` so it is a problem like the rest.

**Prometheus request metrics.** Add `axa_prometheus_metrics::RequestMetrics`, which
registers `http_requests_total` and `http_request_duration_seconds` in the service's
`Registry`, and a `from_fn_with_state` middleware that labels them by method,
`MatchedPath` template, and status. An unmatched request takes the label `unmatched`,
never the raw path, so labels stay bounded and hold no personal data. It needs only the
`prometheus` crate the crate already uses.

**Overload refusal.** Add an `overloaded` problem to `axa-problem` (or `axa::Error`),
`503` with `Retry-After`, and a small `axa::admission::Bounded<T>` around a Tokio
`mpsc` sender whose `try_send` maps `Full` and `Closed` to that error. This is Relay's
`try_push` shape without its envelope types. The outbox and job queue write to a
database and do not need it. It is for services that hand work to in-process workers.

**Memory watermark.** A `health::memory_below(percent, bytes)` named check for
readiness, and a middleware that answers the `overloaded` problem while the same check
fails. It needs `sysinfo`, or reading cgroup files on Linux, so it is a new dependency to
raise first (P-6). The reading should be cached, as Relay does, so a request costs an
atomic load.

**Internal listener.** An `INTERNAL_PORT` setting. When set, `server` serves the health
routes and the metrics path on that port only, and the public router stops serving them.
Both listeners share one shutdown signal. This changes HLT-10's premise, that probes are
on the public port, so the health spec would need a row for it.

## What axa should not copy

- **Sending error text to clients.** Relay returns the whole cause chain and panic
  messages. axa's P-4 forbids this, and axa is right for services that hold data.
- **Trusting forwarding headers from anyone.** Relay takes four headers from any peer.
  axa's `TRUSTED_PROXIES` rule is safer wherever the address feeds a limit or an audit record.
- **Background-loop readiness.** Relay computes readiness on a timer whether anyone
  asks or not. axa's reused answer with one round at a time gives the same protection
  without a task, and adds startup.
- **The actor runtime and envelope buffer.** `relay-system`, the SQLite spool, and
  project-config caches are Relay's product. A library cannot offer them generically.
- **StatsD and cost attribution.** axa has Prometheus and OTLP. CPU time per request for
  billing is Sentry-specific.
- **`X-Sentry-Rate-Limits`, empty `content-encoding` tolerance, and the autoscaling
  endpoint.** These serve Sentry's SDKs and its own autoscaler.
- **A YAML configuration file.** axa's spec rules out configuration files.
