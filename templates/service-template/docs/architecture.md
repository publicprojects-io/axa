# Architecture

How this service is built, and the decisions behind it. Add to the decision log as the
service grows.

## Overview

The service is an ordinary [Axum](https://docs.rs/axum) application. It uses
[Axa](https://github.com/publicprojects-io/axa) for what every service needs, so its own
code is its routes:

| Concern | Where |
|---|---|
| Settings from the environment, refusing unsafe ones in production | `axa::config::Config::from_env`, in `src/main.rs` |
| Logging | `axa::logger::init`, in `src/main.rs` |
| Routes | `src/routes.rs` |
| Health probes | `AppRoutes::with_default_routes`, from Axa |
| Middleware: request ids, security headers, CORS, body limits, timeouts, compression | `into_app`, from Axa |
| Errors as RFC 9457 problems | `axa::Result<Response>` from each handler |
| Graceful shutdown on `SIGTERM` | `axa::server::serve` |

## Directory structure

```text
service-template/
├── Cargo.toml
├── src/
│   ├── main.rs          # Settings, logger, and server
│   └── routes.rs        # The service's routes
├── docs/
│   ├── api.md           # What the service answers
│   └── architecture.md  # This page
├── .github/workflows/   # CI
├── .devcontainer/       # Dev container
└── podman-compose.yml   # Local Postgres and Redis
```

## Settings

Every variable Axa reads is in Axa's
[configuration guide](https://github.com/publicprojects-io/axa/blob/main/docs/configuration/index.md).
`APP_ENV` names the environment, and a release build is `production` unless it says
otherwise. Production refuses a setting that is unsafe or cannot be read, so the service
fails at boot rather than running with something nobody chose.

## Deployment

Point each Kubernetes probe at its own route:

```yaml
ports:
  - containerPort: 1241
livenessProbe:
  httpGet: { path: /_health/live, port: 1241 }
readinessProbe:
  httpGet: { path: /_health/ready, port: 1241 }
startupProbe:
  httpGet: { path: /_health/startup, port: 1241 }
```

Readiness fails from the moment shutdown begins, so a draining pod leaves the Service
before its connections close.

## Decision log

| Date | Decision | Why |
|---|---|---|
| | Start from the Axa service template | Shared health probes, middleware, and errors |
