# API

What this service answers. Keep this page in step with the routes in `src/routes.rs`.

## Base URL

Locally, `http://localhost:1241`. `PORT` sets the port.

## Routes

| Method | Path | Answer |
|---|---|---|
| `GET` | `/api/v1/status` | `{"version": "..."}`, the running version |
| `GET` | `/_health/live` | `200` while the process runs |
| `GET` | `/_health/ready` | `200` when the service can take traffic, `503` otherwise, including while it shuts down |
| `GET` | `/_health/startup` | `200` once the service has started |

Add each new route to this table, with its body and its answers.

## Errors

Every error is an [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) problem, with
`content-type: application/problem+json`:

```json
{
  "type": "about:blank",
  "title": "Not Found",
  "status": 404,
  "detail": "Resource was not found",
  "code": "not_found"
}
```

A problem never quotes what the caller sent, or what went wrong inside the service. A
`5xx` is logged with the request's `x-request-id`, which every response carries, so a
caller can quote that id when reporting a failure.

## Authentication

None yet. With Axa's `paseto` or `keycloak` feature, describe here the token a caller
sends and the `401` and `403` they can get.
