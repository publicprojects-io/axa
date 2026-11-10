# axa-prometheus-metrics

Prometheus text exposition for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family, and `RequestMetrics`, which measures each request by its route template, never its path, with one `unmatched` label for requests that match no route.

Enable it through the `prometheus` feature of `axa`, or depend on this crate alone. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
