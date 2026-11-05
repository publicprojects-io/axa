# axa specification

This directory is the single source of truth for `axa`. Code conforms to the
spec, not the other way round. A change in behaviour is one change carrying
all three parts: spec edit, code edit, test edit.

Each requirement below has an id and names the test that proves it. A
requirement with no test is marked **unverified**, and that is a gap to close.

## 1. Purpose

`axa` is a small shared library for services built directly on Axum and sqlx.
It exists so that a service can be audited by reading it: the SQL, the
middleware, and the startup path are all visible, with no framework between
the reader and the behaviour.

It holds the plumbing that is the same in every service, so that a defect is
fixed once and an auditor reads it once.

## 2. Scope

### In scope

- Route tables that become an ordinary Axum `Router`
- The standard HTTP middleware stack
- Health routes
- The error type and its HTTP mapping
- Configuration from the process environment
- Logging setup and graceful shutdown
- The shared command line, tasks, and background jobs
- A Postgres job queue
- Request-level test helpers
- Transitional SeaORM support

### Out of scope

- Domain logic, handlers, models, and SQL. These stay in each service.
- Authentication and authorization. These stay in `authentication-verifier`
  and in each service.

### Must not be added

- Lifecycle hooks or traits a service implements to be driven by this crate.
  `boot::TestApp` is the one exception, and it exists only for tests.
- Route macros or code generation
- A dependency-injection container or plugin system
- A configuration language or configuration files
- A database abstraction
- A generic CRUD controller

## 3. Principles

| Id | Principle |
|---|---|
| P-1 | **Library, not framework.** A service's `main.rs` stays ordinary Axum and calls in. The control flow is readable from the service's own code. |
| P-2 | **Two consumers first.** Code is added only once two services need it. |
| P-3 | **Fail closed.** A missing or unsafe production setting stops boot. It never falls back to a development value. |
| P-4 | **Generic errors.** Internal failure text is logged and never sent to a client. |
| P-5 | **Written afresh.** Loco is Apache-2.0 only and this crate offers five licences, so its source is not copied. |
| P-6 | **New dependencies are raised first.** They are a cost every consumer pays. |

## 4. Requirements

### 4.1 Environment

| Id | Requirement | Test |
|---|---|---|
| ENV-1 | `APP_ENV` names the environment, then `LOCO_ENV`. When both are unset it is development. | **unverified** (reads the process environment) |
| ENV-2 | `dev` and `prod` are accepted as short forms, in any case. | `parses_names_and_short_forms` |
| ENV-3 | Any name other than development and test is production-like. | `unknown_environment_is_production_like` |

### 4.2 Configuration

| Id | Requirement | Test |
|---|---|---|
| CFG-1 | The default port is 1241. | `default_port_is_1241` |
| CFG-2 | A service may set its own port, and a separate production port. | `production_port_applies_only_in_production` |
| CFG-3 | Production with a database and no `DATABASE_URL` refuses to boot. | `production_refuses_to_boot_without_a_database_url` |
| CFG-4 | A service with an empty `database` default needs no `DATABASE_URL`. | `a_service_with_no_database_boots_in_production_without_one` |
| CFG-5 | Development and test get a local database named `<database>_<environment>`. | `development_and_test_get_a_local_database_named_for_the_environment` |
| CFG-6 | Only the `test` environment sets `dangerously_truncate`. | `only_tests_ever_truncate` |
| CFG-7 | Production does not migrate at boot by default. | `production_does_not_migrate_at_boot_by_default` |
| CFG-8 | Production refuses `*` as a CORS origin. | `production_refuses_a_wildcard_cors_origin` |
| CFG-9 | Production sends no CORS headers until origins are listed. | `production_sends_no_cors_headers_until_origins_are_listed` |
| CFG-10 | An unparsable number falls back to its default. | `an_unparsable_number_falls_back_to_the_default` |
| CFG-11 | `1`, `true`, `yes`, and `on` mean true, and any other set value means false. | `flags_accept_the_usual_spellings` |
| CFG-12 | The `test` environment runs jobs in the foreground unless `WORKER_MODE` says otherwise. | `tests_run_jobs_in_the_foreground` |

The variables are listed in the `config` module's rustdoc.

### 4.3 Routes

| Id | Requirement | Test |
|---|---|---|
| RTE-1 | A group's `"/"` route is served at the prefix, with no trailing slash. | `a_trailing_slash_route_is_served_at_the_prefix`, `join_normalises_slashes` |
| RTE-2 | Two methods added on one path merge. | `two_methods_on_one_path_merge` |
| RTE-3 | Path parameters route. | `path_parameters_route` |
| RTE-4 | An unknown path is a `404`. | `an_unknown_path_is_404` |
| RTE-5 | `Routes` fields are public, so a service's tests can check what is mounted without booting it. | `groups_expose_what_they_mount` |
| RTE-6 | `AppRoutes::into_app` applies the standard middleware. | `into_app_applies_the_standard_middleware` |

### 4.4 Middleware

From the outside in: request id, tracing, panic catching, security headers,
CORS, timeout, compression, body limit.

| Id | Requirement | Test |
|---|---|---|
| MID-1 | Every response carries `x-request-id`. | `responses_carry_request_id_and_security_headers` |
| MID-2 | A caller's request id keeps only alphanumerics, `-`, `_`, and `@`, up to 255 characters. | `request_id_keeps_safe_text_and_strips_the_rest` |
| MID-3 | A UUID is minted when nothing usable remains. | `request_id_is_minted_when_nothing_usable_remains` |
| MID-4 | Responses carry `x-content-type-options: nosniff`, `x-frame-options: DENY`, and `referrer-policy: no-referrer`, without replacing a handler's own value. | `responses_carry_request_id_and_security_headers` (values for the first two; `referrer-policy` and the no-replace rule are **unverified**) |
| MID-5 | `strict-transport-security` is sent when `hsts` is set. | `responses_carry_request_id_and_security_headers` |
| MID-6 | A panic in a handler becomes a `500`. | `a_panic_becomes_a_500` |
| MID-7 | A body over the limit is a `413`. | `an_oversized_body_is_refused` |
| MID-8 | An empty origin list adds no CORS layer. Listed origins are validated. | `an_empty_origin_list_means_no_cors_layer`, `listed_origins_are_validated` |
| MID-9 | A request over `timeout_seconds` is a `408`, and `0` turns the timeout off. | **unverified** |

### 4.5 Errors

| Id | Requirement | Test |
|---|---|---|
| ERR-1 | Internal failures become a generic `500` that contains none of the original text. | `internal_errors_do_not_leak_their_text` |
| ERR-2 | Unauthorized text is logged and not returned. | `unauthorized_text_is_logged_but_not_returned` |
| ERR-3 | Bad-request text is returned to the caller. | `bad_request_text_is_returned` |
| ERR-4 | A custom error keeps its status and body. | `custom_errors_keep_their_status_and_body` |
| ERR-5 | `not_found`, `unauthorized`, and `bad_request` build the matching variants. | `helpers_build_the_matching_variants`, `statuses_match_their_variants` |
| ERR-6 | The `IntoResponse` match has no wildcard arm, so a new variant must choose its status. | Enforced by the compiler |

The body is currently `{error, description, errors}`. See §6 for the move to
RFC 9457.

### 4.6 Responses

| Id | Requirement | Test |
|---|---|---|
| FMT-1 | `format::json` serializes with a JSON content type. | `json_serializes_with_a_json_content_type` |
| FMT-2 | `empty_json` is `{}` and `empty` has no body. | `empty_json_is_an_empty_object`, `empty_has_no_body` |
| FMT-3 | `text` and `html` set their content types. | `text_and_html_set_their_content_types` |
| FMT-4 | `redirect` is a `303` to the target. | `redirect_points_at_the_target` |

### 4.7 Health

| Id | Requirement | Test |
|---|---|---|
| HLT-1 | `/_ping`, `/_health`, and `/_readiness` answer `200` and `{"ok":true}` when healthy. | `every_health_route_answers_ok` |
| HLT-2 | `/_readiness` answers `503` when the database or queue does not. | **unverified** |

### 4.8 Command line and tasks

| Id | Requirement | Test |
|---|---|---|
| CLI-1 | `start` takes `--worker`, `--all`, `--binding`, and `--port`. | `start_takes_its_options` |
| CLI-2 | `--worker` and `--all` conflict. | `worker_and_all_conflict` |
| CLI-3 | `db down` defaults to one step. | `database_commands_parse` |
| CLI-4 | `task` takes a name and `key:value` parameters, and values may contain colons. | `task_takes_a_name_and_parameters`, `task_arguments_keep_colons_in_values` |
| CLI-5 | A parameter with no colon is refused. | `task_argument_without_a_colon_is_refused` |
| CLI-6 | `--environment` works after the subcommand. | `the_environment_flag_works_after_the_subcommand` |
| TSK-1 | A registered task runs with its arguments. | `a_registered_task_runs_with_its_arguments` |
| TSK-2 | An unknown task, or a missing argument, is an error. | `an_unknown_task_is_an_error`, `a_missing_argument_is_an_error` |

### 4.9 Background jobs

| Id | Requirement | Test |
|---|---|---|
| JOB-1 | `ForegroundBlocking` runs the job before returning and reports its failure. | `foreground_jobs_run_before_returning_and_report_failure` |
| JOB-2 | `BackgroundQueue` with no queue is an error, never a silent drop. | `queued_jobs_fail_without_a_queue` |
| JOB-3 | `BackgroundQueue` with a queue enqueues the job. | `perform_later_enqueues_in_queue_mode` |

### 4.10 Queue

Delivery is at least once: a stale job runs again, so a job's work must be
safe to repeat. These tests need `AXA_TEST_DATABASE_URL`.

| Id | Requirement | Test |
|---|---|---|
| QUE-1 | Each job is claimed by exactly one worker loop, using `FOR UPDATE SKIP LOCKED`. | `queue_behaviour_against_postgres` |
| QUE-2 | A failed job keeps its error text on the row and does not stop the loop. | `queue_behaviour_against_postgres` |
| QUE-3 | Arguments that do not parse fail the job and do not panic. | `queue_behaviour_against_postgres` |
| QUE-4 | A job with no handler stays queued for a worker that has one. | `queue_behaviour_against_postgres` |
| QUE-5 | A job running longer than `stale_minutes` is queued again. | `queue_behaviour_against_postgres` |
| QUE-6 | Services booting together all connect. Table setup runs under an advisory lock, because `CREATE ... IF NOT EXISTS` is not safe against itself. | `services_booting_together_all_connect` |
| QUE-7 | Jobs in progress finish before shutdown. | **unverified** directly; `queue_behaviour_against_postgres` shows the loops stop with nothing left running |

### 4.11 Store, logging, and server

| Id | Requirement | Test |
|---|---|---|
| STR-1 | The store holds one value per type. | `stores_one_value_per_type` |
| LOG-1 | `RUST_LOG` wins, then the override filter, then the default modules plus the service. | `default_filter_names_the_service_and_shared_modules`, `an_override_replaces_the_default_filter` (the `RUST_LOG` case is **unverified**) |
| LOG-2 | Level and format names parse, with safe defaults. | `levels_parse_and_print`, `formats_parse` |
| SRV-1 | An address that cannot be bound is an error. | `serve_reports_an_address_it_cannot_bind` |
| SRV-2 | `SIGTERM` and Ctrl-C stop the server and let in-flight requests finish. | **unverified** in this crate; seen on `case-folder`'s real binary |

### 4.12 Transitional SeaORM support

The `sea-orm` feature exists only until the last service has moved to sqlx,
and goes with it.

| Id | Requirement | Test |
|---|---|---|
| SEA-1 | Table names are pluralised snake case. | `table_names_are_pluralised_snake_case` |
| SEA-2 | `create_table` adds `created_at` and `updated_at` with a `now()` default. | **unverified** in this crate; exercised by consuming services' migrations |
| SEA-3 | Foreign keys are named `fk-<table>-<column>-to-<parent>`, and a `?` suffix makes one optional. | **unverified** in this crate |

### 4.13 Subcrates

Each subcrate is optional, is enabled by a feature of `axa`, and is re-exported
under that feature's name. Each can also be used alone. Their requirements are
stated here and proved by their own tests.

| Id | Crate | Requirement | Test |
|---|---|---|---|
| SQX-1 | `axa-sqlx` | Pool settings reach sqlx. | `pool_settings_reach_the_options` |
| SQX-2 | `axa-sqlx` | Migrations apply, and reverting takes back exactly the requested steps. | `pooling_and_migrations_against_postgres` |
| SQX-3 | `axa-sqlx` | Services booting together all migrate. | `services_booting_together_all_migrate` |
| SQX-4 | `axa-sqlx` | A table or type name reaches SQL only through `quote_identifier`, which refuses anything but lowercase ASCII letters, digits, and underscores. | `unsafe_identifiers_are_refused`, `quote_all_fails_on_any_bad_name` |
| SQX-5 | `axa-sqlx` | Recreating a test database keeps its extensions. | `recreating_keeps_extensions` |
| SQX-6 | `axa-sqlx` | Status works on a database that was never migrated. | `status_on_a_fresh_database_shows_nothing_applied` |
| PAG-1 | `axa-pagination` | A large limit is clamped, not rejected. | `a_large_limit_is_clamped_not_rejected` |
| PAG-2 | `axa-pagination` | Zero and unparsable values use the defaults. | `zero_and_unparsable_values_use_the_defaults` |
| PAG-3 | `axa-pagination` | An offset past the maximum is refused. | `an_offset_past_the_maximum_is_refused` |
| PAG-4 | `axa-pagination` | `LIKE` wildcards are escaped, backslash first. | `like_wildcards_are_neutralised`, `backslash_is_escaped_first` |
| PRB-1 | `axa-problem` | Responses follow RFC 9457 and use `application/problem+json`. | `defaults_follow_the_rfc`, `responses_use_the_problem_media_type` |
| PRB-2 | `axa-problem` | An extension cannot overwrite a standard member. | `reserved_names_cannot_be_overwritten_by_extensions` |
| PRM-1 | `axa-prometheus` | Metrics render in the text exposition format, with its media type. | `counters_render_in_the_exposition_format`, `the_response_uses_the_exposition_media_type` |
| VER-1 | `axa-versioning` | No header means the current version, and a bare major is an alias. | `no_header_means_the_current_version`, `a_bare_major_is_an_alias` |
| VER-2 | `axa-versioning` | An unsupported version on an `/api` path is a `406`. | `an_unsupported_version_is_a_406` |
| VER-3 | `axa-versioning` | Paths outside `/api` ignore the header. | `paths_outside_api_ignore_the_header` |

## 5. Verification

CI runs all of this on every push, in `.github/workflows/ci.yml`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --all-targets --features queue,testing -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --features queue,testing
cargo test --workspace --all-features
cargo doc --workspace --no-deps --all-features
AXA_TEST_DATABASE_URL=postgres://... cargo test --features queue,testing -- --ignored
AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-sqlx -- --ignored --test-threads=1
```

All must pass before a commit is pushed or a version is published. The feature
sets are not interchangeable. The health, task, and queue tests compile
only without `sea-orm`, and the worker tests only without `sea-orm` and `queue`.
The rest of the suite runs under every set.

## 6. Decided, not yet built

- **RFC 9457 errors.** Errors will become `application/problem+json`, with
  bare JSON for successes. This changes the older services' `ApiResponse`
  envelope and their front-ends, so it lands as its own change. `axa-problem`
  is built, and no service uses it yet.
- **`axa-opentelemetry`**, replacing the services' copies of OTLP export.
- **`AppContext` on a sqlx pool**, so a service can leave SeaORM.
- **Removing the `sea-orm` feature**, once services have moved to sqlx.

## 7. Open questions

- Whether the shared PASETO and ABAC guard belongs here or in
  `authentication-verifier` behind an `axum` feature.
