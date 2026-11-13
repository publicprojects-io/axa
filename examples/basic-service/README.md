# Basic service

A minimal Axa service with no database: two JSON routes under the standard middleware,
the three health routes, and a clean stop on `SIGTERM` or Ctrl-C.

## Run it

```sh
cd examples/basic-service
cargo run
```

Then:

```sh
curl localhost:1241/api/hello
curl localhost:1241/api/greet/World
curl localhost:1241/_health/ready
curl -i localhost:1241/nope   # a 404 problem, application/problem+json
```

## What it shows

- **Settings from the environment** with `Config::from_env`. `Defaults { database: "" }`
  says this service has no database, so `AppContext::db` is `()`.
- **Routes** with `AppRoutes::with_default_routes`, which adds `/_health/live`,
  `/_health/ready`, and `/_health/startup`, and `Routes::new().prefix("/api")`.
- **The standard middleware** from `into_app`: request ids, security headers, body
  limits, compression in production, and RFC 9457 problems for every refusal.
- **Errors** as `axa::Result<Response>`: `bad_request` answers `400` with its text, and
  `axa::extract::Path` answers a bad path segment as a problem too.
- **Tests** that drive the router with `tower::ServiceExt::oneshot`, with no server.

## Routes

| Method | Path | Answer |
|---|---|---|
| `GET` | `/api/hello` | A fixed greeting |
| `GET` | `/api/greet/{name}` | A greeting by name, `400` for a name over 100 characters |
| `GET` | `/_health/live`, `/_health/ready`, `/_health/startup` | The health probes |

## Settings

Every variable Axa reads is in the [configuration guide](../../docs/configuration/index.md).
The ones to know first:

| Variable | Default | Meaning |
|---|---|---|
| `PORT` | `1241` | The port to listen on |
| `APP_ENV` | `development` in a debug build, `production` in a release build | The environment |
| `RUST_LOG` | Axa's own filter | A log filter in place of Axa's |

## Test it

```sh
cargo test
```
