# axa specification

This directory is the single source of truth for `axa`. Code conforms to the
spec, not the other way round. A change in behaviour is one change carrying
all three parts: spec edit, code edit, test edit.

Each requirement below has an id and names the test that proves it. A
requirement with no test is marked **unverified**, and that is a gap to close.

## 1. Purpose

`axa` is a small shared library for services built directly on Axum. sqlx is the
preferred way to reach a database, and `axa-seaorm` supports projects that keep
SeaORM. It exists so that a service can be audited by reading it: the SQL, the
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
- SeaORM support, for projects leaving Loco

### Out of scope

- Domain logic, handlers, models, and SQL. These stay in each service.
- Verifying tokens and evaluating policy. These stay in `authentication-verifier`,
  and a service's own rules stay in the service. `axa-paseto` is only the Axum layer
  that calls them. `axa-keycloak` is the exception: it carries its own copy of what
  it needs, so that it can be run and audited alone.

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
| P-2 | **Two consumers first.** Code is added only once two services need it. A subcrate built ahead of that is marked in the README as having no service yet, and that mark comes off with its first service. |
| P-3 | **Fail closed.** A missing or unsafe production setting stops boot. It never falls back to a development value. |
| P-4 | **Generic errors.** Internal failure text is logged and never sent to a client. |
| P-5 | **Written afresh.** Loco is Apache-2.0 only and this crate offers five licences, so its source is not copied. |
| P-6 | **New dependencies are raised first.** They are a cost every consumer pays. |

## 4. Requirements

### 4.1 Environment

| Id | Requirement | Test |
|---|---|---|
| ENV-1 | `APP_ENV` names the environment, then `LOCO_ENV`. A blank value counts as unset. | `app_env_is_read_before_loco_env`, `a_blank_name_counts_as_unset` |
| ENV-2 | `dev` and `prod` are accepted as short forms, in any case. | `parses_names_and_short_forms` |
| ENV-3 | Any name other than development and test is production-like. | `unknown_environment_is_production_like` |
| ENV-4 | With no name, a debug build is development and a release build is production, so a release binary started with no variables fails closed. The build is read from `cfg!(debug_assertions)`. | `a_debug_build_with_no_name_is_development`, `a_release_build_with_no_name_is_production`, `parsing_a_blank_name_follows_the_build`. The whole binary is **unverified** in CI: this was seen once by hand |
| ENV-5 | A name means the same in every build. | `a_name_means_the_same_in_every_build` |
| ENV-6 | The lookup lives in `axa-environment`, which has no dependencies, and `axa` re-exports it, so a migration's guard cannot read the environment in a different order from its service. It is the one subcrate that is not optional. | `this_crate_has_no_dependencies`, `axa_reads_the_environment_through_the_lighter_crate` |

### 4.2 Configuration

| Id | Requirement | Test |
|---|---|---|
| CFG-1 | The default port is 1241. | `default_port_is_1241` |
| CFG-2 | A service may set its own port, and a separate production port. | `production_port_applies_only_in_production` |
| CFG-3 | Production with a database and no `DATABASE_URL` refuses to boot. | `production_refuses_to_boot_without_a_database_url` |
| CFG-4 | A service with an empty `database` default needs no `DATABASE_URL`. | `a_service_with_no_database_boots_in_production_without_one` |
| CFG-5 | Development and test get a local database named `<database>_<environment>`, as the user `postgres`. | `development_and_test_get_a_local_database_named_for_the_environment`, `the_local_database_user_names_no_framework` |
| CFG-6 | Only the `test` environment sets `dangerously_truncate`. | `only_tests_ever_truncate` |
| CFG-7 | Production does not migrate at boot by default. | `production_does_not_migrate_at_boot_by_default` |
| CFG-8 | Production refuses `*` as a CORS origin. | `production_refuses_a_wildcard_cors_origin` |
| CFG-9 | Production sends no CORS headers until origins are listed. | `production_sends_no_cors_headers_until_origins_are_listed` |
| CFG-10 | Outside production-like environments, an unparsable number keeps its default and raises a warning. | `an_unparsable_number_falls_back_to_the_default_with_a_warning` |
| CFG-11 | `1`, `true`, `yes`, and `on` mean true, and `0`, `false`, `no`, and `off` mean false. Outside production-like environments, anything else keeps the default and raises a warning. | `flags_accept_the_usual_spellings`, `a_flag_it_does_not_know_keeps_its_default_with_a_warning` |
| CFG-12 | The `test` environment runs jobs in the foreground unless `WORKER_MODE` says otherwise. | `tests_run_jobs_in_the_foreground` |
| CFG-13 | A production-like environment refuses a number, flag, level, format, worker mode, or log filter that is set but invalid. | `production_refuses_an_unparsable_number`, `a_number_out_of_range_is_refused`, `production_refuses_a_flag_it_does_not_know`, `production_refuses_a_name_it_does_not_know`, `a_bad_log_filter_is_refused_and_never_reaches_the_logger` |
| CFG-14 | The refusal names every invalid variable and its value, and never the value under a secret-shaped name. | `every_problem_is_named_at_once`, `an_invalid_value_under_a_secret_name_is_not_printed` |
| CFG-15 | A blank value counts as unset for the settings of CFG-13 and for `DATABASE_URL`. `CORS_ALLOWED_ORIGINS`, `BINDING`, and `HOST` keep a blank value. | `a_blank_value_counts_as_unset`, `production_refuses_a_blank_database_url` |
| CFG-16 | Warnings are kept on `Logger::warnings`, and `logger::init` writes them to stderr. Valid settings raise none. | `valid_settings_raise_no_warnings`, `check_shows_warnings_outside_production` (writing them to stderr was seen once by hand, and is **unverified** by a test) |
| CFG-17 | `redact` hides the whole value of a secret-shaped name, the password in a URL, and any secret-shaped `key=value` pair. It errs towards hiding. | `redact_hides_secret_shaped_names`, `redact_hides_the_password_in_a_url`, `redact_hides_secret_pairs` |
| CFG-18 | `Config::apply_cli` applies `start --binding` and `--port`, and nothing else. | `the_command_line_overrides_only_what_start_gives` |
| CFG-19 | Production sends a strict `content-security-policy` unless `HTTP_CSP` names one, and an empty `HTTP_CSP` sends none. Elsewhere there is none by default. | `production_sends_a_strict_policy_by_default`, `development_sends_no_policy_by_default`, `a_service_can_name_its_own_policy_or_none` |
| CFG-20 | `HSTS_MAX_AGE_SECONDS` sets the HSTS lifetime, one year by default. | `the_hsts_max_age_can_be_set`, `production_sends_a_strict_policy_by_default` |
| CFG-21 | The database connect timeout is 5 seconds and the idle timeout 10 minutes, unless a service's `Defaults` or a variable says otherwise. | `the_database_timeouts_suit_a_real_database`, `a_service_can_state_its_own_database_defaults` |
| CFG-22 | Outside production a service migrates at boot unless `Defaults::auto_migrate` is off, and a variable beats that. | `development_migrates_at_boot_by_default`, `a_service_can_state_its_own_database_defaults` |
| CFG-23 | An unset smallest pool size follows a smaller largest one. A largest size of 0, or a smallest above the largest, is invalid as in CFG-13. | `a_small_largest_pool_size_carries_the_smallest_with_it`, `a_pool_that_cannot_exist_is_refused` |

The variables are listed in `README.md`.

### 4.3 Routes

| Id | Requirement | Test |
|---|---|---|
| RTE-1 | A group's `"/"` route is served at the prefix, with no trailing slash. | `a_trailing_slash_route_is_served_at_the_prefix`, `join_normalises_slashes` |
| RTE-2 | Two methods added on one path merge. | `two_methods_on_one_path_merge` |
| RTE-3 | Path parameters route. | `path_parameters_route` |
| RTE-4 | An unknown path is a `404`. | `an_unknown_path_is_404` |
| RTE-5 | `Routes` fields are public, so a service's tests can check what is mounted without booting it. | `groups_expose_what_they_mount` |
| RTE-6 | `AppRoutes::into_app` applies the standard middleware. | `into_app_applies_the_standard_middleware` |
| RTE-7 | `AppRoutes::mounted`, `Routes::mounted`, and `Handler::methods` list every route as `(method, full path)`. `HEAD` is left out beside `GET`, and a handler for every method shows as `ANY`, so a census cannot miss it. | `mounted_lists_every_method_and_full_path`, `mounted_includes_the_health_routes_and_a_handler_for_any_method`, `head_is_left_out_beside_get`, `methods_survive_a_layer` |
| RTE-8 | Methods come from `MethodRouter`'s debug text, the one place Axum says. A change in that text fails a test by name. | `the_debug_text_still_says_what_is_mounted` |

### 4.4 Middleware

From the outside in: request id, tracing, panic catching, security headers,
CORS, timeout, compression, body limit.

| Id | Requirement | Test |
|---|---|---|
| MID-1 | Every response carries `x-request-id`. | `responses_carry_request_id_and_security_headers` |
| MID-2 | A caller's request id keeps only alphanumerics, `-`, `_`, and `@`, up to 255 characters. | `request_id_keeps_safe_text_and_strips_the_rest` |
| MID-3 | A UUID is minted when nothing usable remains. | `request_id_is_minted_when_nothing_usable_remains` |
| MID-4 | Responses carry `x-content-type-options: nosniff`, `x-frame-options: DENY`, and `referrer-policy: no-referrer`, without replacing a handler's own value. | `responses_carry_request_id_and_security_headers`, `security_headers_do_not_replace_a_handlers_own` |
| MID-5 | `strict-transport-security` is sent when `hsts` is set, for `hsts_max_age` seconds. | `responses_carry_request_id_and_security_headers`, `the_hsts_max_age_is_configurable` |
| MID-6 | A panic in a handler becomes the `500` of any internal failure, logged as an error with no panic text. | `a_panic_becomes_a_500`, `a_panic_answers_like_any_other_internal_failure`. The log line is **unverified** by a test |
| MID-7 | A body over the limit is a `413`. | `an_oversized_body_is_refused` |
| MID-8 | An empty origin list adds no CORS layer. Listed origins are validated. | `an_empty_origin_list_means_no_cors_layer`, `listed_origins_are_validated` |
| MID-9 | A request over `timeout_seconds` is a `408`, and `0` turns the timeout off. | `a_request_past_its_timeout_is_a_408`, `a_zero_timeout_means_none` |
| MID-10 | Every log line written while serving a request carries its request id, method, and route template, and never the path itself or its query string. An unmatched request logs as `unmatched`. | `log_lines_carry_the_request_id_and_not_the_query` |
| MID-11 | One route can have its own timeout. | `a_route_can_have_a_longer_timeout`, `an_override_can_turn_the_timeout_off`, `a_trailing_slash_still_matches_an_override` |
| MID-12 | One route can accept a larger body than the service-wide limit. | `one_route_can_accept_a_larger_body` |
| MID-13 | A route or group can state its own timeout where it is declared, with `Routes::add_with_timeout` or `Routes::timeout`. It replaces the limit in force, longer or shorter, and beats `timeout_overrides`. A route's limit beats its group's. Other routes are still cut off. | `a_route_with_a_longer_limit_outlives_the_stack_and_others_do_not`, `a_route_is_still_cut_off_at_its_own_limit`, `a_group_limit_applies_and_a_routes_own_limit_beats_it`, `a_routes_own_limit_beats_a_timeout_override`, `a_route_limit_covers_one_method_on_a_shared_path` |
| MID-14 | A route's limit applies even where `REQUEST_TIMEOUT_SECONDS` is `0`, and a route limit of zero means no limit. | `a_shorter_route_limit_applies_with_the_stack_timeout_off`, `a_zero_route_limit_means_no_limit` |
| MID-15 | `content-security-policy` is sent when a policy is set, without replacing a handler's own value. | `responses_carry_request_id_and_security_headers`, `no_policy_means_no_csp_header`, `a_handler_may_set_its_own_policy` |
| MID-16 | A policy that is not a valid header value refuses to boot. | `a_policy_that_is_not_a_header_value_refuses_to_boot` |
| MID-17 | The request's own `x-request-id` header carries the cleaned or minted id, so that an inner layer reads what the response will carry. | `the_request_header_carries_the_id_the_response_does`, `the_tracked_id_is_the_id_the_response_carries` |

### 4.5 Errors

| Id | Requirement | Test |
|---|---|---|
| ERR-1 | Internal failures become a generic `500` that contains none of the original text. | `internal_errors_do_not_leak_their_text` |
| ERR-2 | Unauthorized text is logged and not returned. | `unauthorized_text_is_logged_but_not_returned` |
| ERR-3 | Bad-request text is returned to the caller. | `bad_request_text_is_returned` |
| ERR-4 | A custom error keeps its status and body. | `custom_errors_keep_their_status_and_body` |
| ERR-5 | `not_found`, `unauthorized`, and `bad_request` build the matching variants. | `helpers_build_the_matching_variants`, `statuses_match_their_variants` |
| ERR-6 | The `IntoResponse` match has no wildcard arm, so a new variant must choose its status. | Enforced by the compiler |
| ERR-8 | With the `sqlx` or `queue` feature, a sqlx error converts to `Error::Sqlx`, a generic `500`. | `a_sqlx_error_is_a_generic_500` |
| ERR-7 | Only a server fault logs as an error. A client's mistake logs no text, because it can quote personal data, and `Unauthorized` logs a warning. | `only_a_server_fault_is_an_error_and_a_clients_text_is_not_logged` |

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
| HLT-2 | `/_readiness` answers `503` when the database or queue does not answer, naming it, while `/_ping` still passes. | `readiness_fails_when_the_database_does_not_answer`, `readiness_names_the_database`, `readiness_names_a_queue_that_does_not_answer` |
| HLT-3 | A service can register named async checks in `AppContext::health_checks`. All passing gives `{"ok":true}`. Any failing gives `503` and `{"ok":false,"failed":[...]}`, in the order added. | `readiness_with_every_check_passing_is_just_ok`, `readiness_names_what_failed_in_the_order_added` |
| HLT-4 | A check's error text is logged and never sent. | `readiness_never_sends_a_checks_error_text` |
| HLT-5 | A check past its time limit, or one that panics, has failed. | `a_check_past_its_time_limit_fails`, `a_check_that_panics_fails` |
| HLT-6 | One answer, passing or failing, is reused for a few seconds, and only one round runs at a time, so a public route cannot drive load at a dependency. A caller that hangs up does not cancel the round. | `one_answer_is_reused_until_it_goes_stale`, `a_failed_answer_is_reused_too`, `callers_during_a_round_share_it`, `a_caller_that_hangs_up_does_not_waste_the_round` |
| HLT-7 | With the `sqlx` feature, `/_readiness` asks `AppContext::db` as `database`. A pool held anywhere else is a named check. | `the_context_pool_is_checked_without_being_named`, `a_sqlx_pool_is_checked_by_name` |

### 4.8 Command line and tasks

| Id | Requirement | Test |
|---|---|---|
| CLI-1 | `start` takes `--worker`, `--all`, `--binding`, and `--port`. | `start_takes_its_options` |
| CLI-2 | `--worker` and `--all` conflict. | `worker_and_all_conflict` |
| CLI-10 | `--server-and-worker` is accepted for `--all`, so deployment scripts written for Loco keep working. | `the_older_spelling_of_all_still_works` |
| CLI-3 | `db down` defaults to one step. | `database_commands_parse` |
| CLI-4 | `task` takes a name and `key:value` parameters, and values may contain colons. | `task_takes_a_name_and_parameters`, `task_arguments_keep_colons_in_values` |
| CLI-5 | A parameter with no colon is refused. | `task_argument_without_a_colon_is_refused` |
| CLI-6 | `--environment` works after the subcommand. | `the_environment_flag_works_after_the_subcommand` |
| CLI-7 | `check` takes no arguments, and prints the effective settings, then any warnings. It connects to nothing. A service's own settings are its to check. | `check_takes_no_arguments`, `check_prints_the_effective_settings`, `check_shows_warnings_outside_production` |
| CLI-8 | `check` never prints a password or other secret. | `check_never_prints_a_password`, `every_setting_passes_through_redaction` |
| CLI-9 | `check` refuses what a start would refuse, so the process exits non-zero. | `check_refuses_what_a_start_would_refuse`. The exit status was seen once by hand, and is **unverified** by a test |
| CLI-11 | `cli::parse` answers `--help` and `--version` for the service. Nothing presents this crate as the service. | `help_and_version_speak_for_the_service`, `nothing_says_this_crate_is_the_service` |
| TSK-1 | A registered task runs with its arguments. | `a_registered_task_runs_with_its_arguments` |
| TSK-2 | An unknown task, or a missing argument, is an error. | `an_unknown_task_is_an_error`, `a_missing_argument_is_an_error` |

### 4.9 Background jobs

| Id | Requirement | Test |
|---|---|---|
| JOB-1 | `ForegroundBlocking` runs the job before returning and reports its failure. | `foreground_jobs_run_before_returning_and_report_failure` |
| JOB-2 | `BackgroundQueue` with no queue is an error, never a silent drop. | `queued_jobs_fail_without_a_queue` |
| JOB-3 | `BackgroundQueue` with a queue enqueues the job. | `perform_later_enqueues_in_queue_mode` |

### 4.10 Queue

A job gets one attempt by default, because a retry repeats its work. Raising
`max_attempts` makes delivery at least once, so that job's work must be safe to
repeat. These tests need `AXA_TEST_DATABASE_URL`.

| Id | Requirement | Test |
|---|---|---|
| QUE-1 | Each job is claimed by exactly one worker loop, using `FOR UPDATE SKIP LOCKED`. | `queue_behaviour_against_postgres` |
| QUE-2 | A failed job keeps its error text on the row and does not stop the loop. | `queue_behaviour_against_postgres` |
| QUE-3 | Arguments that do not parse fail the job and do not panic. | `queue_behaviour_against_postgres` |
| QUE-4 | A job with no handler stays queued for a worker that has one. | `queue_behaviour_against_postgres` |
| QUE-5 | A job not heard from for `stale_minutes` is queued again if it has attempts left, and otherwise fails, so a crash cannot loop forever. | `a_stale_job_with_attempts_left_runs_again`, `a_job_that_keeps_killing_workers_ends_as_failed` |
| QUE-6 | Services booting together all connect. Table setup runs under an advisory lock, because `CREATE ... IF NOT EXISTS` is not safe against itself. | `services_booting_together_all_connect` |
| QUE-7 | Jobs in progress finish before shutdown. | `a_job_in_progress_finishes_before_shutdown` |
| QUE-8 | A failed job gets `max_attempts` tries, waiting longer each time. The default is one. A worker may set its own limit, which also applies to a lost job. | `a_failing_job_retries_up_to_its_limit`, `a_retry_waits_for_its_backoff`, `by_default_a_failed_job_is_not_repeated`, `each_retry_waits_twice_as_long_up_to_a_limit`, `a_worker_can_allow_more_tries_than_the_queue`, `a_lost_job_uses_its_workers_limit_too` |
| QUE-9 | Finished rows are deleted after `retention_days`, and nothing else is. | `old_finished_rows_are_deleted_and_nothing_else` |
| QUE-10 | A service with a queue and no worker is warned at boot. | `a_service_with_a_queue_and_no_worker_is_warned`, `a_service_without_a_queue_gets_no_worker_warning` |
| QUE-11 | A table created by 0.1.0 gains the `attempts` and `max_attempts` columns. | `the_attempts_column_is_added_to_an_older_table`, `the_max_attempts_column_is_added_to_an_older_table` |
| QUE-12 | A job that panics counts as failed, and its loop carries on. | `a_job_that_panics_fails_and_its_loop_carries_on` |
| QUE-13 | A running job reports in, so it can run past `stale_minutes` without being taken for lost. | `a_long_job_that_reports_in_is_not_taken_for_lost` |
| QUE-14 | A run that was given up on cannot overwrite the job's later state. | `a_job_given_up_on_cannot_overwrite_its_replacement` |
| QUE-15 | A job queued on a transaction commits or rolls back with it. | `a_job_queued_in_a_transaction_rolls_back_with_it` |

### 4.11 Store, logging, and server

| Id | Requirement | Test |
|---|---|---|
| STR-1 | The store holds one value per type. | `stores_one_value_per_type` |
| LOG-1 | `RUST_LOG` wins, then the override filter, then the default modules plus the service. | `default_filter_names_the_service_and_shared_modules`, `an_override_replaces_the_default_filter`, `rust_log_wins_over_the_override_and_the_level`, `a_blank_or_invalid_rust_log_is_passed_over` |
| LOG-2 | Level and format names parse, with safe defaults. | `levels_parse_and_print`, `formats_parse` |
| LOG-3 | The default filter names the service as its log target reads, with `_` for `-`. | `a_package_name_with_a_hyphen_still_shows_its_logs` |
| LOG-4 | Colour goes to a terminal only. | **Unverified** by a test |
| LOG-5 | `logger::init_with` adds layers, which see what the filter lets through, and which keep running when logging is off. | `an_extra_layer_sees_what_the_filter_lets_through`, `extra_layers_keep_running_with_logging_off` |
| LOG-6 | Extra directives widen the default filter only, and never `RUST_LOG` or an override. | `extra_directives_widen_the_default_filter_only`, `rust_log_wins_over_the_override_and_the_level` |
| SRV-1 | An address that cannot be bound is an error. | `serve_reports_an_address_it_cannot_bind` |
| SRV-2 | Shutdown lets in-flight requests finish. | `shutdown_lets_a_request_in_flight_finish` (the signal handling itself was seen on `case-folder`'s real binary) |
| SRV-3 | `server::run` runs a server, a job queue, or both, and logs which. With neither it is an error. | `nothing_to_run_is_an_error`, `a_server_alone_stops_when_told`, `a_worker_alone_stops_when_told` |
| SRV-4 | The job loops stop when the server cannot start. | `the_job_loops_stop_when_the_server_cannot_start` |
| SRV-5 | `SHUTDOWN_TIMEOUT_SECONDS`, 25 by default, limits how long shutdown waits for requests and jobs. `0` waits for as long as they take. | `a_hung_request_cannot_hold_shutdown_past_its_limit`, `a_zero_shutdown_limit_waits_for_requests`, `a_hung_job_cannot_hold_shutdown_past_its_limit` |
| SRV-6 | Requests and jobs hear of shutdown at once and share one limit, and no job starts after that. | `requests_and_jobs_share_one_limit_and_no_job_starts_during_it` |

### 4.12 SeaORM support

`axa-seaorm` lets a project keep SeaORM while it leaves Loco. It stays available
after the Main X Index services move to sqlx, for other projects making the same
move.

| Id | Requirement | Test |
|---|---|---|
| SEA-1 | Table names are pluralised snake case. | `table_names_are_pluralised_snake_case` |
| SEA-2 | `create_table` adds `created_at` and `updated_at`. | `schema_helpers_against_postgres` |
| SEA-3 | Foreign keys are named `fk-<table>-<column>-to-<parent>`, and a `?` suffix makes one optional. | `schema_helpers_against_postgres` |

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
| SQX-7 | `axa-sqlx` | `Db` lets one function take the pool or an open transaction, in a future that can be spawned. | `one_function_serves_the_pool_and_a_transaction` |
| ENV-D | `axa-environment` | A command that destroys data may run only in development or test, and only when confirmed. | `data_is_destroyed_only_in_development_or_test_and_only_when_confirmed` |
| PAG-1 | `axa-pagination` | A large limit is clamped, not rejected. | `a_large_limit_is_clamped_not_rejected` |
| PAG-2 | `axa-pagination` | Zero and unparsable values use the defaults. | `zero_and_unparsable_values_use_the_defaults` |
| PAG-3 | `axa-pagination` | An offset past the maximum is refused. | `an_offset_past_the_maximum_is_refused` |
| PAG-4 | `axa-pagination` | `LIKE` wildcards are escaped, backslash first. | `like_wildcards_are_neutralised`, `backslash_is_escaped_first` |
| PST-1 | `axa-paseto` | Every path is protected unless listed as public. | `an_unlisted_path_is_protected_too` |
| PST-8 | `axa-paseto` | Enforcement is on unless a service turns it off, and turning it off logs a warning. | `enforcement_is_on_unless_turned_off` (the warning is **unverified**) |
| PST-2 | `axa-paseto` | A public prefix matches whole path segments only. | `prefixes_match_whole_segments_only` |
| PST-3 | `axa-paseto` | A missing, forged, or expired token is a `401`. | `a_missing_token_is_a_401`, `a_token_signed_by_another_key_is_a_401`, `an_expired_token_is_a_401` |
| PST-4 | `axa-paseto` | Under the default policy any valid token reads, and every mutation needs an explicit allow. | `any_valid_token_may_read`, `no_attributes_means_no_mutation`, `write_access_writes_but_cannot_delete_or_merge`, `admin_access_may_delete_and_merge` |
| PST-5 | `axa-paseto` | A trailing slash cannot downgrade a destructive action. | `a_trailing_slash_cannot_downgrade_a_destructive_post`, `a_trailing_slash_does_not_reach_a_destructive_route_with_write_access` |
| PST-6 | `axa-paseto` | `AuthUser` verifies even with enforcement off, and fails closed without the middleware. | `auth_user_verifies_even_with_enforcement_off`, `auth_user_fails_closed_without_the_middleware` |
| PST-7 | `axa-paseto` | An edited policy applies to the next request. | `an_edited_policy_applies_to_the_next_request` |
| DSL-1 | `axa-diesel` | No C database library is linked. | `cargo tree` shows no `pq-sys`, `libpq`, or `openssl`, checked in CI |
| DSL-2 | `axa-diesel` | Migrations apply, and reverting takes back exactly the requested steps. | `pooling_and_migrations_against_postgres` |
| DSL-3 | `axa-diesel` | A name reaches SQL only through `quote_identifier`. | `unsafe_identifiers_are_refused`, `quote_all_fails_on_any_bad_name` |
| DSL-4 | `axa-diesel` | Recreating a test database keeps its extensions. | `recreating_keeps_extensions` |
| TRA-1 | `axa-tera` | HTML templates escape values unless a template says `safe`. | `html_templates_escape_values`, `safe_turns_escaping_off` |
| TRA-2 | `axa-tera` | A missing variable or template is an error, not blank output. | `a_missing_variable_is_an_error_rather_than_blank`, `a_missing_template_is_an_error` |
| TRA-3 | `axa-tera` | A failed render shows the visitor nothing from the template. | `a_failed_render_shows_nothing_to_the_visitor` |
| TRA-4 | `axa-tera` | A template syntax error is found when templates load. | `a_syntax_error_is_found_at_boot` |
| TRA-5 | `axa-tera` | A page sends its own `content-security-policy`, which beats `axa`'s production default, so its stylesheets and images load. | `a_page_can_load_its_own_files_in_production`, `json_keeps_the_strict_policy` |
| KCK-1 | `axa-keycloak` | A missing, expired, forged, or unsigned token, or one with the wrong issuer or audience, is a `401`. | `bad_tokens_are_401s`, `a_changed_payload_fails_the_signature_check`, `an_unsigned_token_is_a_401` |
| KCK-2 | `axa-keycloak` | A token with an unverified email is a `401`. | `bad_tokens_are_401s` |
| KCK-3 | `axa-keycloak` | A role grants nothing unless the role map names it. | `mapped_roles_decide_what_a_caller_may_do`, `an_unmapped_role_grants_nothing` |
| KCK-4 | `axa-keycloak` | A refused caller learns nothing about why. | `a_refusal_tells_the_caller_nothing_about_why` |
| KCK-5 | `axa-keycloak` | An unreachable realm refuses every token and never hangs a request. | `an_unreachable_realm_refuses_every_token` |
| KCK-6 | `axa-keycloak` | A PASETO is accepted only through a verifier the service supplies, and then under the same policy. | `a_paseto_is_refused_unless_the_guard_accepts_them`, `with_paseto_both_kinds_of_token_work_under_one_policy` |
| KCK-7 | `axa-keycloak` | A half-finished configuration is an error, never Keycloak silently off. | `a_half_configuration_is_an_error_rather_than_off`, `no_url_means_keycloak_is_off` |
| KCK-8 | `axa-keycloak` | `AuthUser` verifies even with enforcement off, and fails closed without the middleware. | `with_enforcement_off_no_token_is_needed_except_by_auth_user`, `auth_user_fails_closed_without_the_middleware` |
| KCK-9 | `axa-keycloak` | It depends on neither `authentication-verifier` nor any other `axa` crate, and states the source of each copy. | `no_dependency_on_what_this_crate_was_copied_from` |
| KCK-10 | `axa-keycloak` | Only an access token passes: one with no audience, a `typ` other than `Bearer`, or an `nbf` or `iat` in the future is a `401`. | `only_a_current_access_token_for_this_audience_passes` |
| RDS-1 | `axa-redis` | Every entry expires. A TTL below one millisecond is refused. | `a_zero_ttl_is_refused`, `a_zero_ttl_stores_nothing`, `entries_expire`, `the_default_ttl_applies` |
| RDS-2 | `axa-redis` | Prefixes keep services apart. | `prefixes_keep_services_apart` |
| RDS-3 | `axa-redis` | An unreadable entry is a miss, not a failure. | `an_unreadable_value_is_a_miss`, `a_changed_type_reads_as_a_miss` |
| RDS-4 | `axa-redis` | `get_or_load` loads once, and a failed load stores nothing. | `get_or_load_loads_once`, `a_failed_load_stores_nothing` |
| RDS-5 | `axa-redis` | Neither an outage nor a frozen Redis fails `get_or_load`. Every call has a time limit, and an outage costs a request one limit, not the client's own retries. | `an_outage_does_not_fail_get_or_load`, `a_frozen_redis_does_not_hold_a_request_up`, `every_call_is_bounded_while_the_client_reconnects` |
| RDS-6 | `axa-redis` | `Cache::memory` behaves like Redis in one process, and holds at most 32 MB. | `the_memory_cache_stores_like_redis`, `clones_of_a_memory_cache_share_it`, `the_memory_cache_refuses_a_zero_ttl`, `the_memory_cache_loads_on_a_miss_only`, `an_expired_entry_is_gone_and_its_bytes_are_returned`, `replacing_an_entry_counts_only_the_new_one`, `the_memory_cache_never_holds_more_than_its_limit`, `stale_entries_go_before_live_ones` |
| OAP-1 | `axa-openapi` | The document and the page are served at their paths. | `the_document_is_served_as_json`, `the_page_is_served_as_html` |
| OAP-2 | `axa-openapi` | Swagger UI is one exact version, and every remote file carries an integrity hash. | `the_page_pins_an_exact_version_with_integrity_hashes` |
| OAP-3 | `axa-openapi` | A title or URL cannot inject markup. | `the_title_cannot_inject_markup`, `the_spec_url_cannot_break_out_of_the_script` |
| OAP-4 | `axa-openapi` | Bearer authentication is declared and required, keeping other components. | `bearer_auth_is_declared_and_required`, `bearer_auth_keeps_other_components` |
| OAP-5 | `axa-openapi` | Parameter names do not distinguish routes. | `parameter_names_do_not_change_a_shape`, `agreement_is_empty_whatever_the_parameters_are_called` |
| OAP-6 | `axa-openapi` | `drift` reports a mounted route the document lacks, and a documented operation with no route. | `a_mounted_route_missing_from_the_document_is_reported`, `a_documented_operation_with_no_route_is_reported`, `a_document_with_no_paths_reports_every_route` |
| OAP-7 | `axa-openapi` | A path kept out on purpose, and a path item's other members, are not reported. | `a_path_kept_out_on_purpose_is_not_reported`, `a_path_items_other_members_are_not_operations` |
| OAP-8 | `axa-openapi` | `dangling_refs` reports a local `$ref` to nothing, and leaves references to other files alone. | `a_reference_that_resolves_is_not_reported`, `a_reference_to_nothing_is_reported_wherever_it_is`, `a_reference_to_another_file_is_left_alone` |
| OAP-9 | `axa-openapi` | The Swagger UI page sets its own `content-security-policy`, so the production default does not blank it. | `the_page_sets_a_policy_that_lets_it_load` |
| SOR-1 | `axa-seaorm` | Pool settings reach `SeaORM`. | `pool_settings_reach_the_options` |
| SOR-2 | `axa-seaorm` | `query::condition()` builds the same conditions as Loco's, all of which must hold. | `eq_and_ne_compare_values`, `several_conditions_must_all_hold` |
| SOR-3 | `axa-seaorm` | Validation failures keep Loco's compact JSON text, so snapshots do not change. | `the_database_error_is_compact_json` |
| SES-1 | `axa-session` | `production` refuses a key under 64 bytes, rather than letting the session crate panic on it, and refuses two identical keys. | `a_short_key_is_refused_not_a_panic`, `the_two_keys_must_differ` |
| SES-2 | `axa-session` | Production cookies are `Secure`, `HttpOnly`, `SameSite=Lax`, and signed. Only `development` is unsigned. | `a_production_cookie_is_secure_and_signed`, `a_development_cookie_is_neither_secure_nor_signed` |
| SES-3 | `axa-session` | A live session's identifier without its signature, or with a changed one, reads nothing. | `a_real_id_with_no_signature_reads_nothing`, `a_changed_signature_reads_nothing`, `an_unsigned_cookie_can_be_forged_in_development` |
| SES-4 | `axa-session` | A session survives between requests, `renew` changes its identifier and keeps its data, and `sign_out` ends it. | `a_session_survives_between_requests`, `renew_changes_the_id_and_keeps_the_data`, `destroy_ends_the_session` |
| SES-5 | `axa-session` | Keys are never printed. | `debug_output_hides_the_keys` |
| SES-6 | `axa-session` | A session is not tied to the client's IP, so a phone changing networks keeps it. | `a_session_survives_a_new_ip_address` |
| SES-7 | `axa-session` | An anonymous visitor keeps no cookie, as they are sent only a deletion, and has no stored data and no row, unless a service asks for every visitor to be stored. | `an_anonymous_visitor_keeps_no_cookie_and_nothing_is_stored`, `anonymous_visitors_add_no_rows`, `every_visitor_can_be_stored_when_a_service_asks` |
| SES-8 | `axa-session` | In Postgres, session data is encrypted at rest and survives a restart of the store. | `postgres_sessions_are_encrypted_and_survive_a_restart` |
| SES-9 | `axa-session` | `sign_in` makes an identifier from before it worthless, keeps the session stored, and replaces the form token. | `sign_in_makes_an_identifier_from_before_it_worthless`, `sign_in_replaces_the_form_token` |
| SES-10 | `axa-session` | A form token is stable within a session, passes only for that session, and a session with none refuses every post. | `a_form_token_is_stable_and_only_it_passes`, `a_form_token_belongs_to_one_session`, `a_session_with_no_token_refuses_every_post`, `only_the_same_text_is_the_same` |
| PRB-1 | `axa-problem` | Responses follow RFC 9457 and use `application/problem+json`. | `defaults_follow_the_rfc`, `responses_use_the_problem_media_type` |
| PRB-2 | `axa-problem` | An extension cannot overwrite a standard member. | `reserved_names_cannot_be_overwritten_by_extensions` |
| PRM-1 | `axa-prometheus` | Metrics render in the text exposition format, with its media type. | `counters_render_in_the_exposition_format`, `the_response_uses_the_exposition_media_type` |
| VER-1 | `axa-versioning` | No header means the current version, and a bare major is an alias. | `no_header_means_the_current_version`, `a_bare_major_is_an_alias` |
| VER-2 | `axa-versioning` | An unsupported version on an `/api` path is a `406`. | `an_unsupported_version_is_a_406` |
| VER-3 | `axa-versioning` | Paths outside `/api` ignore the header. | `paths_outside_api_ignore_the_header` |
| OTL-1 | `axa-opentelemetry` | Export is off until an endpoint is set, and off leaves requests working with no header. | `export_is_off_until_an_endpoint_is_set`, `off_means_no_layers_no_header_and_the_request_still_works` |
| OTL-2 | `axa-opentelemetry` | A span and the request metric reach a real collector, with the service's name and version. | `a_request_reaches_the_collector_as_a_span_and_a_metric` |
| OTL-3 | `axa-opentelemetry` | Plain `http` is for loopback, unless a clear yes allows it. An invalid endpoint is refused, and an error never quotes the endpoint. | `plain_http_is_for_loopback`, `plain_http_to_another_host_is_refused`, `plain_http_to_another_host_can_be_allowed`, `anything_but_a_clear_yes_keeps_insecure_refused`, `a_bad_endpoint_is_refused`, `an_error_never_quotes_the_endpoint`, `start_refuses_cleartext_to_another_host` |
| OTL-4 | `axa-opentelemetry` | An `https` endpoint trusts the system's roots. | `an_https_endpoint_builds_with_roots_to_trust`. The handshake itself is **unverified** by a test |
| OTL-5 | `axa-opentelemetry` | A collector that is down never holds up a request. | `a_collector_that_is_down_stops_nothing` |
| OTL-6 | `axa-opentelemetry` | A metric's attributes are the method, status, and route template. An unknown method is `_OTHER`, and a path's value never appears. | `an_unknown_method_cannot_create_a_series`, `a_request_reaches_the_collector_as_a_span_and_a_metric` |
| OTL-7 | `axa-opentelemetry` | The middleware adds to the request span `axa` opens, and puts its `traceparent` on the response. A server error marks the span. | `the_middleware_joins_the_request_span_axa_opens`, `a_server_error_marks_the_span` |
| OTL-8 | `axa-opentelemetry` | `DIAGNOSTIC_DIRECTIVES` are valid filter directives. | `the_diagnostic_directives_are_valid` |
| TRC-1 | `axa-tracing` | `Redacted` hides its value in `Display` and `Debug`, and so in a log line. | `redacted_hides_its_value_in_a_log_line` |
| TRC-2 | `axa-tracing` | `SlowSpans` reports a span at or past its threshold, inside that span so that it carries its fields, and tells busy time from idle time. A quick span is not reported. | `a_slow_span_is_reported_with_its_fields`, `a_quick_span_is_not_reported`, `busy_and_idle_time_are_told_apart` |
| TRC-3 | `axa-tracing` | `track` makes the request id readable anywhere the request runs, cleaned by `axa`'s rule, or minted when nothing usable remains. Outside a request there is none. | `a_handler_can_read_the_request_id`, `a_callers_id_is_cleaned_as_axa_cleans_it`, `an_id_is_minted_when_nothing_usable_remains`, `there_is_no_request_id_outside_a_request` |
| TRC-4 | `axa-tracing` | `propagate` adds the id to a call to a peer, and keeps a value already there. | `propagate_sends_the_id_and_keeps_one_already_set` |
| TRC-5 | `axa-tracing` | `spawn` keeps the request id and the span in the new task, and works outside a request. | `a_spawned_task_keeps_the_request_id`, `a_spawned_task_keeps_the_span`, `spawn_works_outside_a_request` |
| TRC-6 | `axa-tracing` | `Watch::watch` tells a future that blocked a runtime thread from one that only waited. | `a_blocking_future_is_reported`, `a_future_that_only_waits_is_slow_but_not_blocking`, `a_quick_future_is_not_reported` |
| TRC-7 | `axa-tracing` | A task that wakes late is reported with a snapshot of the runtime, and none of this needs `tokio_unstable`. | `a_stalled_runtime_is_reported`, `a_snapshot_describes_the_runtime` |
| TRC-8 | `axa-tracing` | `log_panics` logs where a panic happened, and never its text. | `a_panic_is_logged_without_its_text` |

### 4.14 Databases

`AppContext::db` is a SeaORM connection with the `sea-orm` feature, a `PgPool`
with the `sqlx` feature, and `()` with neither. With both, it is the SeaORM
connection. `db::connect` opens either from `config::Database`.

## 5. Verification

CI runs all of this on every push, in `.github/workflows/ci.yml`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --all-targets --features queue,testing -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --features queue,testing
cargo test --workspace --all-features
cargo doc --workspace --no-deps --all-features
cargo tree -p axa-diesel --all-features -e normal   # must show no C database library
AXA_TEST_DATABASE_URL=postgres://... cargo test --features queue,testing -- --ignored
AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-sqlx -- --ignored --test-threads=1
AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-seaorm -- --ignored --test-threads=1
AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-diesel -- --ignored --test-threads=1
AXA_TEST_DATABASE_URL=postgres://... cargo test -p axa-session --all-features -- --ignored --test-threads=1
AXA_TEST_REDIS_URL=redis://... cargo test -p axa-redis -- --ignored
```

Separate jobs check what the code is built from. Actions are pinned to a commit
and images to a digest, and these must pass too:

```sh
cargo audit
cargo deny --all-features check advisories bans licenses sources
zizmor --offline .github
gitleaks detect --source . --redact --no-banner
```


Subcrates publish before `axa`, because it depends on them by version.
`axa-environment` has no dependencies, so it can go first.

All must pass before a commit is pushed or a version is published. The feature
sets are not interchangeable. The health, task, and queue tests compile
only without `sea-orm`, and the worker tests only without `sea-orm` and `queue`.
The rest of the suite runs under every set.

### Releasing

A version breaks when a public struct gains a field, because services build these
by hand. Publish only from a commit where everything above has passed.

## 6. Decided, not yet built

- **RFC 9457 errors.** Errors will become `application/problem+json`, with
  bare JSON for successes. This changes the older services' `ApiResponse`
  envelope and their front-ends, so it lands as its own change. `axa-problem`
  is built, and no service uses it yet.

## 7. Open questions

None.
