# Upgrading to Axa 0.23

This covers a service on 0.22. A service that builds `Defaults` with
`..Defaults::default()`, as every example does, upgrades by changing the versions. That
holds for a service with a database and for one without.

## What will not compile

| Change | What to do |
|---|---|
| `axa-test` 0.14 is the one for `axa` 0.23 | Set `axa-test = "0.14"` in `[dev-dependencies]` |
| `Defaults` has `request_timeout_seconds`, `cors_origins`, `hsts`, and `compression` | A `Defaults { .. }` that names every field needs `..Defaults::default()`, or the four new fields set to `None` |

## What changes with no compile error

| Change | What to do |
|---|---|
| The standard stack logs a warning the first time it drops a handler's plain-text answer on a route, naming the method, the route template, and the status | Where the caller needs the text, return `Error::with_detail(status, sentence)`. Where it does not, nothing |

## What is new

| Addition | Use |
|---|---|
| `Defaults::request_timeout_seconds`, `cors_origins`, `hsts`, `compression` | State in code what a `Dockerfile` set: `request_timeout_seconds: Some(10)` in place of `ENV REQUEST_TIMEOUT_SECONDS=10`. The variable still wins, and `check` prints what is in effect |
| `AppRoutes::into_app_with(ctx, layers)` | Add a rate limit or an auth guard inside the standard middleware, in place of `into_router`, the layer, and `middleware::apply` by hand |
| `Error::with_detail(status, sentence)` | A problem whose `detail` is the sentence, and whose `code` is the status's reason, in place of a type of the service's own for it |
| `AppContext::for_test(db)` | A `test` context that reads no variables, in place of building a `Config` by hand. A service with no database passes `()` |
| `Config::for_task` and `for_task_from_lookup` | Build a task's context with them, so that a setting only the server reads, such as `CORS_ALLOWED_ORIGINS`, cannot stop the task. `config::LISTENER_ONLY` names those settings |
| [A service with its own store](../a-service-with-its-own-store/index.md) | A guide for a service whose data is in a store of its own, such as `rusqlite` |
