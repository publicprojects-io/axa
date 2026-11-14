# Todo API

A todo API on Postgres, with sqlx and Axa's `postgres-sqlx` feature.

## Run it

Start Postgres from the repository root, then the service:

```sh
podman-compose up -d postgres
cd examples/todo-api
export DB_URL=postgres://axa:axa@localhost:5432/axa_development
cargo run
```

With no `DB_URL`, a development build uses
`postgres://postgres:postgres@localhost:5432/todo_api_development` instead, for a
Postgres of your own.

Then:

```sh
curl -X POST localhost:1241/api/todos -H 'content-type: application/json' -d '{"title":"Milk"}'
curl localhost:1241/api/todos
curl -X PATCH localhost:1241/api/todos/1 -H 'content-type: application/json' -d '{"completed":true}'
curl -X DELETE localhost:1241/api/todos/1
curl localhost:1241/_health/ready   # asks the database too
```

Axa does not read `.env` itself, so `.env.example` lists the variables for your shell or
a tool such as `direnv`.

## What it shows

- **The pool in the context.** With `postgres-sqlx`, `AppContext::db` is a `PgPool`,
  `axa::db::connect` opens it from `DB_URL`, and `/_health/ready` asks it.
- **Migrations at boot**, from `migrations/`, with `axa::postgres_sqlx::migrate`, only
  when `config.database.auto_migrate` is on. It is on outside production, and off in
  production unless `DB_AUTO_MIGRATE` turns it on.
- **Readiness that waits for them.** `Checks::migrations` holds `/_health/ready` and
  `/_health/startup` at `503`, naming `migrations`, while any is pending, so a new pod
  gets no traffic before the migration its code needs has run.
- **Plain SQL** with `sqlx::query_as` and bound parameters, through the `sqlx` that Axa
  re-exports, so there is no second sqlx to keep in step.
- **Problems for every refusal.** A blank title is `400` with its text, a body with the
  wrong fields is `422`, a missing todo is `404`, and a database that cannot serve is
  `503`, none of them quoting what the caller sent or what the database said.
- **A limit on every list.** `GET /api/todos` returns at most 100. For real paging, see
  the `cursor-pagination` feature.

## Routes

| Method | Path | Body | Answer |
|---|---|---|---|
| `GET` | `/api/todos` | | Up to 100 todos, oldest first |
| `POST` | `/api/todos` | `{"title": "..."}` | The new todo |
| `GET` | `/api/todos/{id}` | | The todo, or `404` |
| `PATCH` | `/api/todos/{id}` | `{"title": "...", "completed": true}`, either or both | The changed todo, or `404` |
| `DELETE` | `/api/todos/{id}` | | `200`, or `404` |

A title is trimmed, and must be 1 to 200 characters. The table checks this too.

## Test it

```sh
cargo test
```

The test that talks to Postgres is ignored unless you name a database it may wipe:

```sh
podman-compose exec postgres createdb -U axa todo_api_test
AXA_TEST_DB_URL=postgres://axa:axa@localhost:5432/todo_api_test cargo test -- --ignored
```

It drops and recreates `todos` and `_sqlx_migrations`, so never point it at a database
that holds anything else.
