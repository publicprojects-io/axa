# axa-opentelemetry

OTLP export of traces and request metrics for Axum services.

It sends a service's `tracing` spans to an `OpenTelemetry` collector over OTLP/gRPC,
records `http.server.request.duration`, and puts a `traceparent` header on each
response.

Export is off until `OTLP_ENDPOINT` is set. Plain `http` is refused for anything but
loopback unless `OTLP_ALLOW_INSECURE` is set, because spans carry what logs carry. A
collector that is down never holds up a request.

Metric attributes are the method, status, and route template, so a caller cannot grow
the number of series, and an identifier in a path does not reach the metrics.

Use it alone, or through `axa` with the `opentelemetry` feature, as
`axa::opentelemetry`.

## License

MIT, Apache-2.0, BSD-3-Clause, GPL-2.0, or GPL-3.0, at your option.
