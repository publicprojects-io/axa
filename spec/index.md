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
- Conditional requests
- Health routes
- The error type and its HTTP mapping
- Configuration from the process environment
- Logging setup and graceful shutdown
- The shared command line, tasks, and background jobs
- A Postgres job queue, worker pool, and recurring schedules
- `Idempotency-Key` handling
- A transactional outbox and relay on Postgres
- Request-level test helpers
- SeaORM support, for projects leaving Loco

### Out of scope

- Domain logic, handlers, models, and SQL. These stay in each service.
- Broker clients, topics, and event envelopes. `axa` depends on no broker, and a
  service passes its publisher in.
- Verifying tokens and evaluating policy. These stay in `authentication-verifier`,
  and a service's own rules stay in the service. `axa-paseto-auth` is only the Axum layer
  that calls them. `axa-keycloak-auth` is the exception: it carries its own copy of what
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
| P-2 | **Two consumers first, or one with a safety case.** Code is added only once two services need it, or once one service needs it to keep a control that a hazard log, a regulator, or a security review holds it to. |
| P-3 | **Fail closed.** A missing or unsafe production setting stops boot. It never falls back to a development value. |
| P-4 | **Generic errors.** Internal failure text is logged and never sent to a client. |
| P-5 | **Written afresh.** Loco is Apache-2.0 only and this crate offers five licences, so its source is not copied. |
| P-6 | **New dependencies are raised first.** They are a cost every consumer pays. |
| P-7 | **A record is not state.** Where a row is evidence of something a service is answerable for, such as a disclosure, a refusal, or a decision, it is appended and never edited or deleted, and the database is given triggers that refuse it. State that is only a working copy, such as a cache, a lease, or a count, may change. A crate says which of its tables is which. |
| P-8 | **Final means final.** An outcome a service has called final is never undone by a helper. Putting work back is an act of its own, by a named person, and is kept as a record. |
| P-9 | **An outcome commits with its audit.** An API that records an outcome takes the caller's transaction, as `outbox::write` already does, so that a service's audit row and the outcome commit together or not at all. |
| P-10 | **The safe way is the only way.** When a safer or more reliable behaviour replaces a weaker one, the weaker one goes, in a breaking release, with an upgrade path. It is not kept as an option or a default. Where a service must choose, it chooses by passing something explicit, never by leaving a default alone. |

## 4. Requirements

### 4.1 Environment

| Id | Requirement | Test |
|---|---|---|
| ENV-1 | `APP_ENV` names the environment, and no other variable does. A blank value counts as unset. | `only_app_env_names_the_environment`, `a_blank_name_counts_as_unset` |
| ENV-2 | `dev` and `prod` are accepted as short forms, in any case. | `parses_names_and_short_forms` |
| ENV-3 | Any name other than development and test is production-like. | `unknown_environment_is_production_like` |
| ENV-4 | With no name, a debug build is development and a release build is production, so a release binary started with no variables fails closed. The build is read from `cfg!(debug_assertions)`. | `a_debug_build_with_no_name_is_development`, `a_release_build_with_no_name_is_production`, `parsing_a_blank_name_follows_the_build`, and `with_no_name_this_build_decides`, which CI runs in a debug build and a release build |
| ENV-5 | A name means the same in every build. | `a_name_means_the_same_in_every_build` |
| ENV-6 | The lookup lives in `axa-environment`, which has no dependencies, and `axa` re-exports it, so a migration's guard cannot read the environment in a different order from its service. It is not optional. | `this_crate_has_no_dependencies`, `axa_reads_the_environment_through_the_lighter_crate` |

### 4.2 Configuration

| Id | Requirement | Test |
|---|---|---|
| CFG-1 | The default port is 1241. | `default_port_is_1241` |
| CFG-2 | A service may set its own port, and a separate production port. | `production_port_applies_only_in_production` |
| CFG-3 | Production with a database and no `DB_URL` refuses to boot. | `production_refuses_to_boot_without_a_database_url` |
| CFG-4 | A service with an empty `database` default needs no `DB_URL`. | `a_service_with_no_database_boots_in_production_without_one` |
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
| CFG-15 | A blank value counts as unset for the settings of CFG-13 and for `DB_URL`. `CORS_ALLOWED_ORIGINS`, `BINDING`, and `HOST` keep a blank value. | `a_blank_value_counts_as_unset`, `production_refuses_a_blank_database_url` |
| CFG-16 | Warnings are kept on `Logger::warnings`, and `logger::init` writes them to stderr. Valid settings raise none. | `valid_settings_raise_no_warnings`, `check_shows_warnings_outside_production`, `settings_warnings_go_to_stderr_whatever_the_logging` |
| CFG-25 | The `Debug` text of `Database`, and so of `Config` and `AppContext`, hides any password in the connection string. | `debug_output_never_prints_a_password`, `a_pools_debug_text_holds_no_password` |
| CFG-26 | The `Debug` text of `PoolConfig`, in `axa-sqlx`, `axa-seaorm`, `axa-diesel`, and `axa-toasty`, leaves the connection string out. | `debug_text_leaves_out_the_connection_string` |
| CFG-17 | `redact` hides the whole value of a secret-shaped name, the password in a URL, and any secret-shaped `key=value` pair. It errs towards hiding. | `redact_hides_secret_shaped_names`, `redact_hides_the_password_in_a_url`, `redact_hides_secret_pairs` |
| CFG-18 | `Config::apply_cli` applies `start --binding` and `--port`, and nothing else. | `the_command_line_overrides_only_what_start_gives` |
| CFG-19 | Production sends a strict `content-security-policy` unless `HTTP_CONTENT_SECURITY_POLICY` names one, and an empty `HTTP_CONTENT_SECURITY_POLICY` sends none. Elsewhere there is none by default. | `production_sends_a_strict_policy_by_default`, `development_sends_no_policy_by_default`, `a_service_can_name_its_own_policy_or_none` |
| CFG-20 | `HSTS_MAX_AGE_SECONDS` sets the HSTS lifetime, one year by default. | `the_hsts_max_age_can_be_set`, `production_sends_a_strict_policy_by_default` |
| CFG-21 | The database connect timeout is 5 seconds and the idle timeout 10 minutes, unless a service's `Defaults` or a variable says otherwise. | `the_database_timeouts_suit_a_real_database`, `a_service_can_state_its_own_database_defaults` |
| CFG-22 | Outside production a service migrates at boot unless `Defaults::auto_migrate` is off, and a variable beats that. | `development_migrates_at_boot_by_default`, `a_service_can_state_its_own_database_defaults` |
| CFG-23 | An unset smallest pool size follows a smaller largest one. A largest size of 0, or a smallest above the largest, is invalid as in CFG-13. | `a_small_largest_pool_size_carries_the_smallest_with_it`, `a_pool_that_cannot_exist_is_refused` |
| CFG-24 | Production refuses a `DB_URL` that could go in the clear: it must demand TLS or stay on this machine, judged by `host` and `hostaddr` as the driver reads them, unless `DB_ALLOW_INSECURE` is set. `require` encrypts without checking who answers, and `verify-full` is what checks. An address it cannot read is refused, and the error does not quote it. | `production_refuses_a_database_url_that_could_go_in_the_clear`, `production_accepts_a_database_url_that_stays_private` |

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

From the outside in: request id, tracing, security headers, CORS, timeout,
`304`s (CND-1), compression, refusal shaping, body limit, panic catching.

| Id | Requirement | Test |
|---|---|---|
| MID-1 | Every response carries `x-request-id`. | `responses_carry_request_id_and_security_headers` |
| MID-2 | A caller's request id keeps only alphanumerics, `-`, `_`, and `@`, up to 255 characters. | `request_id_keeps_safe_text_and_strips_the_rest` |
| MID-3 | A UUID is minted when nothing usable remains. | `request_id_is_minted_when_nothing_usable_remains` |
| MID-4 | Responses carry `x-content-type-options: nosniff`, `x-frame-options: DENY`, and `referrer-policy: no-referrer`, without replacing a handler's own value. | `responses_carry_request_id_and_security_headers`, `security_headers_do_not_replace_a_handlers_own` |
| MID-5 | `strict-transport-security` is sent when `hsts` is set, for `hsts_max_age` seconds. | `responses_carry_request_id_and_security_headers`, `the_hsts_max_age_is_configurable` |
| MID-6 | A panic in a handler becomes the `500` of any internal failure, logged as an error with no panic text, and it gets the same security and CORS headers as any other answer. | `a_panic_becomes_a_500`, `a_panic_answers_like_any_other_internal_failure`, `a_panic_gets_the_same_headers_as_any_other_answer`, `log_lines_carry_the_request_id_and_not_the_query` |
| MID-7 | A body over the limit is a `413` problem. | `an_oversized_body_is_refused`, `an_oversized_body_is_a_problem` |
| MID-7a | The limit binds a handler that reads the body itself, past the extractors, and `middleware::body_limit` raises it on one route for both. | `a_handler_reading_the_body_itself_is_bound_by_the_limit`, `one_route_can_accept_a_larger_body` |
| MID-8 | An empty origin list adds no CORS layer. Listed origins are validated. | `an_empty_origin_list_means_no_cors_layer`, `listed_origins_are_validated` |
| MID-9 | A request over `timeout_seconds` is a `408` problem, and `0` turns the timeout off. | `a_request_past_its_timeout_is_a_408`, `a_route_limit_answers_as_a_problem`, `a_zero_timeout_means_none` |
| MID-10 | Every log line written while serving a request carries its request id, method, and route template, and never the path itself or its query string. An unmatched request logs as `unmatched`. | `log_lines_carry_the_request_id_and_not_the_query` |
| MID-11 | One route can have its own timeout. | `a_route_can_have_a_longer_timeout`, `an_override_can_turn_the_timeout_off`, `a_trailing_slash_still_matches_an_override` |
| MID-12 | One route can accept a larger body than the service-wide limit. | `one_route_can_accept_a_larger_body` |
| MID-13 | A route or group can state its own timeout where it is declared, with `Routes::add_with_timeout` or `Routes::timeout`. It replaces the limit in force, longer or shorter, and beats `timeout_overrides`. A route's limit beats its group's. Other routes are still cut off. | `a_route_with_a_longer_limit_outlives_the_stack_and_others_do_not`, `a_route_is_still_cut_off_at_its_own_limit`, `a_group_limit_applies_and_a_routes_own_limit_beats_it`, `a_routes_own_limit_beats_a_timeout_override`, `a_route_limit_covers_one_method_on_a_shared_path` |
| MID-14 | A route's limit applies even where `REQUEST_TIMEOUT_SECONDS` is `0`, and a route limit of zero means no limit. | `a_shorter_route_limit_applies_with_the_stack_timeout_off`, `a_zero_route_limit_means_no_limit` |
| MID-15 | `content-security-policy` is sent when a policy is set, without replacing a handler's own value. | `responses_carry_request_id_and_security_headers`, `no_policy_means_no_csp_header`, `a_handler_may_set_its_own_policy` |
| MID-16 | A policy that is not a valid header value refuses to boot. | `a_policy_that_is_not_a_header_value_refuses_to_boot` |
| MID-17 | The request's own `x-request-id` header carries the cleaned or minted id, so that an inner layer reads what the response will carry. | `the_request_header_carries_the_id_the_response_does`, `the_tracked_id_is_the_id_the_response_carries` |
| MID-18 | A layer added before `apply` is inside the standard stack, so a refusal from it has a request id and the security headers. One added after `apply` is outside, and its refusal has neither. | `a_refusal_from_a_layer_inside_the_stack_gets_the_standard_headers`, `a_refusal_from_a_layer_outside_the_stack_gets_none_of_them` |
| MID-19 | A layer outside the stack reads a response body compressed when compression is on, and one inside reads it plain. | `a_layer_outside_the_stack_reads_a_compressed_body`, `a_layer_inside_the_stack_reads_a_plain_body` |
| MID-20 | Responses carry `cross-origin-opener-policy: same-origin`, `cross-origin-resource-policy: same-origin`, and a `permissions-policy`, a deny-list unless `HTTP_PERMISSIONS_POLICY` names another, or none when it is blank, without replacing a handler's own values. | `the_cross_origin_and_permissions_headers_are_set_unless_a_handler_sets_its_own`, `the_permissions_policy_is_a_deny_list_unless_named_or_blanked` |
| MID-21 | A response that sets a cookie is `cache-control: no-store` unless its handler set its own caching, and compression never touches a response that sets a cookie or is `no-store`, against BREACH. | `a_response_setting_a_cookie_is_not_stored_or_compressed`, `no_store_is_read_as_a_directive_among_others` |
| MID-22 | `authorization`, `proxy-authorization`, `cookie`, and `set-cookie` are marked sensitive outside every other layer, so none records them. | `credential_headers_are_marked_sensitive` |

### 4.5 Errors

| Id | Requirement | Test |
|---|---|---|
| ERR-1 | Internal failures become a generic `500` that contains none of the original text. | `internal_errors_do_not_leak_their_text`, `a_missing_shared_value_is_the_generic_500` |
| ERR-2 | Unauthorized text is logged and not returned. | `unauthorized_text_is_logged_but_not_returned` |
| ERR-3 | Bad-request text is returned to the caller. | `bad_request_text_is_returned` |
| ERR-4 | A custom error keeps its status, `code`, `detail`, and `errors`. | `custom_errors_keep_their_status_and_body`, `a_status_with_no_standard_reason_is_still_a_problem` |
| ERR-5 | `not_found`, `unauthorized`, and `bad_request` build the matching variants. | `helpers_build_the_matching_variants`, `statuses_match_their_variants` |
| ERR-6 | The `IntoResponse` match has no wildcard arm, so a new variant must choose its status. | Enforced by the compiler |
| ERR-8 | With the `sqlx`, `queue`, or `sea-orm` feature, a violated unique or foreign key constraint answers `409`, and a violated check or not-null constraint answers `422` with code `unprocessable_entity`. Each logs as a client's mistake, with no text from the database, so no column or value is named. Any other database error is a generic `500`. | `a_sqlx_error_is_a_generic_500`, `a_violated_constraint_through_sqlx_is_the_callers_mistake`, `a_violated_constraint_through_sea_orm_is_the_callers_mistake`, `a_violated_constraint_logs_nothing_at_error` |
| ERR-7 | Only a server fault logs as an error, and it logs the error's message, never its `Debug` text, which for a database error quotes the row. A client's mistake logs no text, because it can quote personal data, and `Unauthorized` logs a warning. | `only_a_server_fault_is_an_error_and_a_clients_text_is_not_logged` |
| ERR-9 | Every error `axa` answers is an RFC 9457 problem with the media type `application/problem+json`: `type` is `about:blank`, `title` is the status's reason, `status` is the status, and `detail` is present only when there is something more to say. A success stays bare JSON. | `the_bodies_are_exactly_these`, `model_errors_answer_as_problems`, `a_panic_answers_like_any_other_internal_failure`, `json_serializes_with_a_json_content_type` |
| ERR-10 | The machine-readable reason is the `code` extension member. `axa`'s own are `bad_request`, `unauthorized`, `not_found`, `conflict`, `validation`, `request_timeout`, `not_ready`, `not_started`, `shutting_down`, `unavailable`, `precondition_failed`, `precondition_required`, and `internal_server_error`, and a refusal shaped under ERR-13 takes its status's reason in lower case with underscores. | `the_bodies_are_exactly_these`, `a_request_past_its_timeout_is_a_408`, `a_wrong_method_is_a_problem_and_keeps_its_allow_header`, `a_stale_write_is_a_412_problem_and_writes_nothing`, `if_match_is_optional_unless_required` |
| ERR-11 | Field errors are the `errors` extension member. A model's validation failure is a `400` with the `code` `validation`. | `field_errors_are_an_extension_member`, `a_validation_failure_keeps_its_field_errors` |
| ERR-12 | A rejected JSON body keeps the rejection's status, its `code` is that status's, and `detail` says what was wrong in fixed text. The parser's text, which can quote the body, is not returned. | `a_rejected_json_body_keeps_its_status_and_hides_its_text` |
| ERR-13 | With the standard stack, an error with no `content-type`, or with `text/plain`, becomes a problem for its status with no `detail`. That covers an unmatched path, a wrong method, and an extractor's rejection. Its text is dropped, as a rejection's can quote the request, its headers stay, and any other media type is left alone. An unmatched path answers as `Error::NotFound` does. | `an_unmatched_path_answers_like_not_found`, `a_wrong_method_is_a_problem_and_keeps_its_allow_header`, `a_rejected_extractor_is_a_problem_that_quotes_nothing`, `a_handlers_plain_text_error_is_not_sent`, `a_bare_status_is_a_problem_and_keeps_its_headers`, `an_error_with_its_own_media_type_and_a_success_are_left_alone`, `only_a_missing_type_or_plain_text_is_bare`, `a_refusal_is_compressed_like_any_other_answer` |
| ERR-14 | `axa::extract`'s `Json`, `Path`, and `Query` reject with a problem whose `detail` says what was wrong, keeping the rejection's status, and quote nothing the caller sent. A `Path` that does not fit its route is a bug in the service, and answers the generic `500`. The prelude keeps Axum's. | `every_rejection_is_a_problem_and_quotes_nothing`, `a_good_request_passes_through`, `a_path_that_does_not_fit_its_route_is_a_server_fault` |

The body of a failed request, with a `detail` and with none:

```json
{"type":"about:blank","title":"Bad Request","status":400,"detail":"name is required","code":"bad_request"}
{"type":"about:blank","title":"Internal Server Error","status":500,"code":"internal_server_error"}
```

Axum writes some refusals itself, as plain text or with no body, and the standard
stack makes those problems too (ERR-13). A handler that takes
`Result<Json<T>, JsonRejection>` and returns the rejection as an `Error` gets a
problem with a `detail`. `axa-tera-templates` answers a failed render in HTML, as its reader is
a browser (TRA-3).

### 4.6 Responses

| Id | Requirement | Test |
|---|---|---|
| FMT-1 | `format::json` serializes with a JSON content type. | `json_serializes_with_a_json_content_type` |
| FMT-2 | `empty_json` is `{}` and `empty` has no body. | `empty_json_is_an_empty_object`, `empty_has_no_body` |
| FMT-3 | `text` and `html` set their content types. | `text_and_html_set_their_content_types` |
| FMT-4 | `redirect` is a `303` to the target. | `redirect_points_at_the_target` |

### 4.7 Health

The health routes, `/_health/live`, `/_health/ready`, and `/_health/startup`, follow the
liveness, readiness, and startup probes Kubernetes gives a container. Their rules, and
the requirements HLT-1 to HLT-11, are in [Health routes](health/index.md).

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
| CLI-9 | `check` refuses what a start would refuse, and returns the refusal as an error, so a `main` that hands it on with `?` exits non-zero. The exit status is the service's `main`'s to give, not this crate's. | `check_refuses_what_a_start_would_refuse` |
| CLI-11 | `cli::parse` answers `--help` and `--version` for the service. Nothing presents this crate as the service. | `help_and_version_speak_for_the_service`, `nothing_says_this_crate_is_the_service` |
| CLI-12 | `cli::parse`, `Cli::parse`, and `Cli::try_parse` refuse `db down`, `db reset`, and `db truncate`, before a service runs anything, unless the process environment is development or test and `AXA_DESTROY_DATA` is `true`. `--environment` can add to that refusal and never lift it. `Cli::parse_from` and `Cli::try_parse_from` read no process arguments, are for tests, and refuse nothing. | `only_down_reset_and_truncate_destroy_data`, `a_destructive_command_is_refused_outside_development_and_test`, `a_destructive_command_needs_confirming`, `the_environment_flag_cannot_lift_the_refusal`, `other_commands_are_never_refused`, `the_written_out_parser_reads_what_the_derived_one_did`. The refusal through `Cli::parse` was seen by hand on a real binary |
| TSK-1 | A registered task runs with its arguments. | `a_registered_task_runs_with_its_arguments` |
| TSK-2 | An unknown task, or a missing argument, is an error. | `an_unknown_task_is_an_error`, `a_missing_argument_is_an_error` |
| TSK-3 | A task marked `development_only` is refused outside development and test. Both the process environment and `--environment` must allow it. Other tasks run anywhere. | `a_task_runs_anywhere_unless_it_says_otherwise`, `a_development_task_needs_both_environments_to_allow_it`, `a_development_task_is_refused_in_production` |

### 4.9 Background jobs

| Id | Requirement | Test |
|---|---|---|
| JOB-1 | `ForegroundBlocking` runs the job before returning and reports its failure. | `foreground_jobs_run_before_returning_and_report_failure` |
| JOB-2 | `BackgroundQueue` with no queue is an error, never a silent drop. | `queued_jobs_fail_without_a_queue` |
| JOB-3 | `BackgroundQueue` with a queue enqueues the job. | `perform_later_enqueues_in_queue_mode` |
| JOB-4 | `perform_in` keeps its delay on the queued job, and a worker's `timeout` goes on its jobs. There is no default limit. | `perform_in_delays_a_queued_job_and_carries_its_workers_timeout` |

### 4.10 Queue

The engine is `axa-postgres-job-queue` (section 4.18), and `axa::queue` puts `BackgroundWorker`
over it. These rows are proved through `axa::queue`. A job gets one attempt by default, because a retry repeats its work. Raising
`max_attempts` makes delivery at least once, so that job's work must be safe to
repeat. These tests need `AXA_TEST_DB_URL`.

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
| QUE-14 | A run that was given up on cannot overwrite the job's later state. It is marked `needs_review` instead (WRK-12). | `a_job_given_up_on_cannot_overwrite_its_replacement` |
| QUE-15 | A job queued on a transaction commits or rolls back with it. | `a_job_queued_in_a_transaction_rolls_back_with_it` |
| QUE-16 | `Queue::connect_existing` changes no schema, and fails at connect when the table is missing or out of date. | `connecting_to_an_existing_table_changes_no_schema` |
| QUE-17 | `queue::cancel` and `queue::retry` take the caller's transaction, a `by`, and a `reason`, and keep the act (WRK-24). An id that is not one matches no job. A `by` or `reason` that is not a code is `Error::BadRequest`. `Queue::acts` reads a job's acts. | `a_persons_act_through_axa_is_kept_with_who_and_why` |

### 4.11 Store, logging, and server

| Id | Requirement | Test |
|---|---|---|
| STR-1 | The store holds one value per type. | `stores_one_value_per_type` |
| LOG-1 | `RUST_LOG` wins, then the override filter, then the default modules plus the service. | `default_filter_names_the_service_and_shared_modules`, `an_override_replaces_the_default_filter`, `rust_log_wins_over_the_override_and_the_level`, `a_blank_or_invalid_rust_log_is_passed_over` |
| LOG-2 | Level and format names parse, with safe defaults. | `levels_parse_and_print`, `formats_parse` |
| LOG-3 | The default filter names the service as its log target reads, with `_` for `-`. | `a_package_name_with_a_hyphen_still_shows_its_logs` |
| LOG-4 | Colour goes to a terminal only. | `logs_to_a_pipe_carry_no_colour`, `colour_once_turned_on_shows` |
| LOG-5 | `logger::init_with` adds layers, which see what the filter lets through, and which keep running when logging is off. | `an_extra_layer_sees_what_the_filter_lets_through`, `extra_layers_keep_running_with_logging_off` |
| LOG-6 | Extra directives widen the default filter only, and never `RUST_LOG` or an override. | `extra_directives_widen_the_default_filter_only`, `rust_log_wins_over_the_override_and_the_level` |
| SRV-1 | An address that cannot be bound is an error. | `serve_reports_an_address_it_cannot_bind` |
| SRV-2 | Shutdown lets in-flight requests finish. | `shutdown_lets_a_request_in_flight_finish`, `sigterm_lets_a_request_in_flight_finish_and_exits_cleanly` |
| SRV-3 | `server::run` runs a server, a job queue, or both, and logs which. With neither it is an error. | `nothing_to_run_is_an_error`, `a_server_alone_stops_when_told`, `a_worker_alone_stops_when_told` |
| SRV-4 | The job loops stop when the server cannot start. | `the_job_loops_stop_when_the_server_cannot_start` |
| SRV-5 | `SHUTDOWN_TIMEOUT_SECONDS`, 25 by default, limits how long shutdown waits for requests and jobs. `0` waits for as long as they take. | `a_hung_request_cannot_hold_shutdown_past_its_limit`, `a_zero_shutdown_limit_waits_for_requests`, `a_hung_job_cannot_hold_shutdown_past_its_limit` |
| SRV-6 | Requests and jobs hear of shutdown at once and share one limit, and no job starts after that. | `requests_and_jobs_share_one_limit_and_no_job_starts_during_it` |
| SRV-7 | A client that does not finish its headers within `HTTP_REQUEST_HEADER_READ_TIMEOUT_SECONDS`, 30 by default, is dropped, from when it connects or sends the first byte of a later request, including one that sends nothing. `0` allows for ever. A limit too long for the clock means none. The exception is a request that starts in the same read as the end of the last one: it is bound by the keep-alive limit instead, because `hyper` holds those bytes where this crate cannot see them. The server speaks HTTP/1 only, so that no step waits with no limit. A handler still sees the client's address. | `a_client_that_stalls_in_its_headers_is_dropped`, `a_header_limit_of_zero_waits_for_ever`, `a_handler_still_sees_the_clients_address`, `a_client_that_sends_nothing_is_dropped`, `an_http2_client_that_goes_silent_is_dropped`, `a_stall_in_a_second_requests_headers_is_dropped` |
| SRV-8 | At most `MAX_CONNECTIONS` connections, by default `server::MAX_CONNECTIONS`, are open at once. An upgraded connection counts for as long as its socket is open. More wait to be accepted, and are not refused. | `connections_past_the_limit_wait_their_turn`, `a_limit_of_zero_means_none`, `an_upgraded_connection_keeps_its_slot` |
| SRV-9 | A connection can still be upgraded, as a WebSocket is. | `a_connection_can_be_upgraded` |
| SRV-10 | `MAX_CONNECTIONS` sets the limit, and `0` is invalid. | `the_connection_limit_is_a_setting_and_cannot_be_zero` |
| SRV-11 | A real `SIGTERM` stops `server::run` cleanly: the process exits with success, and a request in flight and a job in progress finish first, whether the job runs in a worker or beside a server. | `sigterm_lets_a_request_in_flight_finish_and_exits_cleanly`, `sigterm_lets_a_job_in_progress_finish_in_a_worker`, `sigterm_lets_a_job_in_progress_finish_beside_a_server` |
| SRV-12 | Ctrl-C (`SIGINT`) stops `server::run` as `SIGTERM` does: a request in flight finishes, and the process exits with success. | `ctrl_c_lets_a_request_in_flight_finish_and_exits_cleanly` |
| CLT-1 | With no `TRUSTED_PROXIES`, and for any peer not listed, `X-Forwarded-For` is ignored. | `with_no_proxies_the_header_is_ignored`, `a_caller_that_is_no_proxy_cannot_name_an_address`, `with_no_trusted_proxy_a_forged_header_changes_nothing` |
| CLT-2 | Behind trusted proxies, the client is the rightmost untrusted entry, so an entry the caller wrote is not believed. | `the_rightmost_untrusted_entry_is_the_client`, `an_entry_the_caller_wrote_is_not_believed`, `repeated_headers_read_as_one_list`, `with_only_proxies_the_leftmost_is_the_client`, `an_ipv4_address_inside_ipv6_is_the_same_address` |
| CLT-3 | An unreadable entry ends the walk at the last good address, and at most 32 entries are read. | `an_unreadable_entry_ends_the_walk`, `a_long_header_stops_at_the_limit` |
| CLT-4 | The address reaches a handler through `ClientAddr`, and a layer as a plain `IpAddr` extension, and stays out of the request span. | `a_handler_gets_the_address_behind_a_trusted_proxy` |
| CLT-5 | `TRUSTED_PROXIES` takes addresses and CIDR ranges. A bad list trusts none, and refuses to boot in production. | `ranges_read_and_match`, `a_bad_range_is_refused`, `trusted_proxies_are_read_and_a_bad_list_trusts_none` |
| SRV-13 | An idle connection is kept for `KEEP_ALIVE_TIMEOUT_SECONDS`, 75 by default, a separate limit from SRV-7, so that it can outlast a load balancer's. `0` keeps it for ever. Neither limit cuts off a request being served, a response still reaching the client, or an upgraded connection. A connection is idle from the last byte written, so a client that stops reading a finished response is dropped at this limit. | `an_idle_connection_outlasts_the_header_limit`, `an_idle_connection_is_dropped_at_the_keep_alive_limit`, `neither_limit_cuts_off_a_slow_handler`, `neither_limit_cuts_off_a_slow_response_body`, `an_upgraded_connection_is_left_alone`, `a_limit_of_zero_means_none`, `a_long_response_outlasts_the_keep_alive_limit`, `a_response_going_out_at_shutdown_is_not_cut_off`, `a_client_that_stops_reading_a_finished_response_is_dropped`, `a_limit_too_long_for_the_clock_means_none` |
| SRV-14 | A client that has stopped reading can hold up a response for `SEND_STALL_TIMEOUT_SECONDS`, 60 by default, counted from a write its socket cannot take, and is then dropped, freeing its slot, at shutdown too. A write that goes through clears the limit, so a slow client keeps its response, and waiting does not move it. A handler or stream with nothing to send never blocks a write, so no time limit applies to it, and an upgraded connection is left alone. `0` waits for ever. | `a_client_that_stops_reading_a_stream_loses_its_slot`, `a_stream_with_no_send_stall_limit_keeps_its_slot`, `a_reader_that_keeps_up_outlasts_the_send_stall_limit`, `a_reader_that_keeps_up_with_a_stream_outlasts_the_send_stall_limit`, `a_stalled_response_is_dropped_with_no_keep_alive_limit`, `a_stalled_client_cannot_hold_shutdown`, `waiting_on_a_blocked_write_does_not_move_its_deadline`, `neither_limit_cuts_off_a_slow_response_body`, `the_send_stall_limit_is_a_setting`, `a_limit_of_zero_means_none` |
| SRV-15 | Told to stop, the server keeps accepting connections for `SHUTDOWN_DRAIN_SECONDS`, 0 by default, then stops listening and lets requests finish. `SHUTDOWN_TIMEOUT_SECONDS` counts for requests from the end of the drain. | `the_drain_keeps_accepting_and_tells_handlers_shutdown_has_begun`, `with_no_drain_the_listener_closes_at_once` |
| SRV-16 | `server::Shutdown` is an extractor that says whether shutdown has begun, and waits for it, so a long-lived response ends itself. A request served other than by this server gets one that never begins. | `the_drain_keeps_accepting_and_tells_handlers_shutdown_has_begun`, `a_stream_waiting_on_shutdown_ends_itself`, `a_request_outside_this_server_never_sees_shutdown` |

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
stated here and proved by their own tests. There are two exceptions.
`axa-environment` (ENV-6) and `axa-problem` are always included, and `axa::problem`
always exists, as `axa::Error` answers with it. The `problem` feature adds nothing,
and stays so that a service naming it still builds.

Paging is `axa-cursor-pagination`, the `cursor-pagination` feature, as
`axa::cursor_pagination`. Offset paging is gone (P-10): its total could count rows a
record policy hides, an offset is a scan, and the position was the caller's.
`axa-offset-pagination`, and `axa-pagination` before it, stay on crates.io for services
still on axa 0.8 or earlier, and are not published again. `axa` 0.9 has no
`offset-pagination` or `pagination` feature, and no `axa::offset_pagination` or
`axa::pagination`. PAG-1, PAG-2, PAG-3, PAG-5, and PAG-6 went with them.

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
| PAG-4 | `axa-cursor-pagination` | `LIKE` wildcards are escaped, backslash first. | `like_wildcards_are_neutralised`, `backslash_is_escaped_first` |
| PAG-7 | `axa-cursor-pagination` | A collection read MUST NOT take `?offset=`, and MUST NOT send `X-Total-Count`. | `there_is_no_offset_parameter`, `a_response_carries_no_total` |
| PAG-8 | `axa-cursor-pagination` | A page is chosen by an opaque `?cursor=`. The response MUST carry the next page as a `Link` header with `rel="next"`, and the last page MUST carry none. The body is a bare JSON array. | `the_next_page_is_a_link_and_the_last_page_has_none`, `a_cursor_round_trips_to_the_next_page`, `a_body_is_a_bare_json_array_of_the_page` |
| PAG-9 | `axa-cursor-pagination` | A limit that is zero, not a number, given twice, or above the endpoint's maximum MUST be refused with `422`, never clamped or replaced by the default. Only a missing limit uses the default. | `a_bad_limit_is_refused_not_clamped`, `a_missing_limit_uses_the_default_and_the_first_page_starts_at_zero` |
| PAG-10 | `axa-cursor-pagination` | A response MUST NOT send a total, because a total would count rows the caller's record policy hides. | `a_response_carries_no_total` |
| PAG-11 | `axa-cursor-pagination` | A cursor MUST be signed by the service, so that a caller cannot make one up or edit one. A cursor that is malformed, carries a bad signature, is given twice, or belongs to another query MUST be refused with `422`. A cursor belongs to the path and every query parameter but `cursor` and `limit`, in any order, so the limit may change between pages. | `a_made_up_cursor_is_refused`, `an_edited_cursor_is_refused`, `a_cursor_from_another_query_is_refused`, `a_cursor_signed_by_another_key_is_refused`, `the_filters_may_come_in_any_order_and_the_limit_may_change` |
| PAG-12 | `axa-cursor-pagination` | A key is at least 32 bytes, and its `Debug` never shows it. | `a_short_key_is_refused_and_a_key_is_never_shown` |
| PAG-13 | `axa-cursor-pagination` | `CursorPage::bounds` asks for one row more than the page, so that `split` can tell whether more follow, and saturates for Postgres. | `bounds_fetch_one_more_row_and_saturate` |
| PAG-14 | `axa-cursor-pagination` | A refusal quotes nothing the caller sent, and as `axa::Error` it is a `422` problem with the `code` `unprocessable_entity`. | `an_error_quotes_nothing_the_caller_sent`, `a_bad_page_is_a_422_problem_that_quotes_nothing` |
| PST-1 | `axa-paseto-auth` | Every path is protected unless listed as public. | `an_unlisted_path_is_protected_too` |
| PST-8 | `axa-paseto-auth` | Enforcement is on unless a service turns it off, and turning it off logs a warning. | `enforcement_is_on_unless_turned_off`, `a_refusal_leaves_one_audit_line_and_an_allowed_request_none` |
| PST-2 | `axa-paseto-auth` | A public prefix matches whole path segments only. | `prefixes_match_whole_segments_only` |
| PST-3 | `axa-paseto-auth` | A missing, forged, or expired token is a `401`. | `a_missing_token_is_a_401`, `a_token_signed_by_another_key_is_a_401`, `an_expired_token_is_a_401` |
| PST-4 | `axa-paseto-auth` | Under the default policy any valid token reads, and every mutation needs an explicit allow. | `any_valid_token_may_read`, `no_attributes_means_no_mutation`, `write_access_writes_but_cannot_delete_or_merge`, `admin_access_may_delete_and_merge` |
| PST-5 | `axa-paseto-auth` | A trailing slash cannot downgrade a destructive action. | `a_trailing_slash_cannot_downgrade_a_destructive_post`, `a_trailing_slash_does_not_reach_a_destructive_route_with_write_access` |
| PST-6 | `axa-paseto-auth` | `AuthUser` verifies even with enforcement off, and fails closed without the middleware. | `auth_user_verifies_even_with_enforcement_off`, `auth_user_fails_closed_without_the_middleware` |
| PST-7 | `axa-paseto-auth` | An edited policy applies to the next request. | `an_edited_policy_applies_to_the_next_request` |
| PST-9 | `axa-paseto-auth` | A refused caller learns nothing about why. A refusal is a problem. A `401`'s `detail` is always `invalid token` and a `403`'s is always `forbidden`, its `code` says no more than its status, and the reason goes to the log. | `a_refusal_tells_the_caller_nothing_about_why`, `a_rejection_answers_with_the_problem_media_type` |
| PST-10 | `axa-paseto-auth` | `parse_bool` does not read an unknown word as off. | `on_off_settings_parse` |
| PST-11 | `axa-paseto-auth` | A decision carrying an obligation the service has not declared is a `403`. A handler reads a declared one through `Allowed`, which is refused where nothing was decided. | `an_obligation_nobody_carries_out_is_refused`, `a_handler_reads_the_obligations_the_service_declared`, `allowed_is_refused_where_nothing_was_decided` |
| PST-12 | `axa-paseto-auth` | A `401` carries `WWW-Authenticate: Bearer`, as RFC 9110 requires, with no `error` parameter, so the challenge says no more than the status. A `403` carries none. | `a_401_carries_a_bearer_challenge_and_a_403_does_not` |
| DSL-1 | `axa-diesel` | No C database library is linked. | `cargo tree` shows no `pq-sys`, `libpq`, or `openssl`, checked in CI |
| DSL-2 | `axa-diesel` | Migrations apply, and reverting takes back exactly the requested steps. | `pooling_and_migrations_against_postgres` |
| DSL-3 | `axa-diesel` | A name reaches SQL only through `quote_identifier`. | `unsafe_identifiers_are_refused`, `quote_all_fails_on_any_bad_name` |
| DSL-5 | `axa-diesel` | A pool that is all in use answers with an error within `wait_timeout`, rather than waiting for ever. | `a_pool_that_is_all_in_use_answers_with_an_error_in_time` |
| DSL-4 | `axa-diesel` | Recreating a test database keeps its extensions. | `recreating_keeps_extensions` |
| TRA-1 | `axa-tera-templates` | HTML templates escape values unless a template says `safe`. | `html_templates_escape_values`, `safe_turns_escaping_off` |
| TRA-2 | `axa-tera-templates` | A missing variable or template is an error, not blank output. | `a_missing_variable_is_an_error_rather_than_blank`, `a_missing_template_is_an_error` |
| TRA-3 | `axa-tera-templates` | A failed render is HTML, and shows the visitor nothing from the template. | `a_failed_render_shows_nothing_to_the_visitor` |
| TRA-4 | `axa-tera-templates` | A template syntax error is found when templates load. | `a_syntax_error_is_found_at_boot` |
| TRA-5 | `axa-tera-templates` | A page sends its own `content-security-policy`, which beats `axa`'s production default, so its stylesheets and images load. | `a_page_can_load_its_own_files_in_production`, `json_keeps_the_strict_policy` |
| KCK-1 | `axa-keycloak-auth` | A missing, expired, forged, or unsigned token, or one with the wrong issuer or audience, is a `401`. | `bad_tokens_are_401s`, `a_changed_payload_fails_the_signature_check`, `an_unsigned_token_is_a_401` |
| KCK-2 | `axa-keycloak-auth` | A token with an unverified email is a `401`. | `bad_tokens_are_401s` |
| KCK-3 | `axa-keycloak-auth` | A role grants nothing unless the role map names it. | `mapped_roles_decide_what_a_caller_may_do`, `an_unmapped_role_grants_nothing` |
| KCK-4 | `axa-keycloak-auth` | A refused caller learns nothing about why. A refusal is a problem, written here because of KCK-9. A `403` does not name the rule, which goes to the log. | `a_refusal_tells_the_caller_nothing_about_why`, `a_forbidden_caller_is_not_told_which_rule_refused_them`, `a_rejection_answers_with_the_problem_media_type` |
| KCK-5 | `axa-keycloak-auth` | An unreachable realm refuses every token, and a request waits about five seconds at most for the realm's keys. | `an_unreachable_realm_refuses_every_token` |
| KCK-6 | `axa-keycloak-auth` | A PASETO is accepted only through a verifier the service supplies, and then under the same policy. | `a_paseto_is_refused_unless_the_guard_accepts_them`, `with_paseto_both_kinds_of_token_work_under_one_policy` |
| KCK-7 | `axa-keycloak-auth` | A half-finished configuration is an error, never Keycloak silently off. | `a_half_configuration_is_an_error_rather_than_off`, `no_url_means_keycloak_is_off` |
| KCK-8 | `axa-keycloak-auth` | `AuthUser` verifies even with enforcement off, and fails closed without the middleware. | `with_enforcement_off_no_token_is_needed_except_by_auth_user`, `auth_user_fails_closed_without_the_middleware` |
| KCK-9 | `axa-keycloak-auth` | It depends on neither `authentication-verifier` nor any other `axa` crate, and states the source of each copy. | `no_dependency_on_what_this_crate_was_copied_from` |
| KCK-10 | `axa-keycloak-auth` | Only an access token passes: one with no audience, a `typ` other than `Bearer`, or an `nbf` or `iat` in the future is a `401`. | `only_a_current_access_token_for_this_audience_passes` |
| KCK-11 | `axa-keycloak-auth` | A decision carrying an obligation the service has not declared is a `403`. A handler reads a declared one through `Allowed`, which is refused where nothing was decided. | `an_obligation_nobody_carries_out_is_refused`, `a_handler_reads_the_obligations_the_service_declared`, `allowed_is_refused_where_nothing_was_decided` |
| KCK-12 | `axa-keycloak-auth` | `KeycloakVerifier::ready` fails until the realm's keys have been fetched. | `ready_says_whether_tokens_can_be_verified` |
| KCK-13 | `axa-keycloak-auth` | A `401` carries `WWW-Authenticate: Bearer`, as RFC 9110 requires, with no `error` parameter, so the challenge says no more than the status. A `403` carries none. | `a_401_carries_a_bearer_challenge_and_a_403_does_not` |
| RDS-7 | `axa-redis-cache` | A log line holds neither a key, which can name a person, nor a stored value. | `an_unreadable_entry_is_logged_without_its_key_or_value`, `a_failed_cache_write_is_logged_without_its_key_or_value`, `a_failed_read_is_logged_without_its_key` (Redis tests, the connection cut and Redis stalled) |
| RDS-1 | `axa-redis-cache` | Every entry expires. A TTL below one millisecond is refused. | `a_zero_ttl_is_refused`, `a_zero_ttl_stores_nothing`, `entries_expire`, `the_default_ttl_applies` |
| RDS-2 | `axa-redis-cache` | Prefixes keep services apart. | `prefixes_keep_services_apart` |
| RDS-3 | `axa-redis-cache` | An unreadable entry is a miss, not a failure. | `an_unreadable_value_is_a_miss`, `a_changed_type_reads_as_a_miss` |
| RDS-4 | `axa-redis-cache` | `get_or_load` loads once, and a failed load stores nothing. | `get_or_load_loads_once`, `a_failed_load_stores_nothing` |
| RDS-5 | `axa-redis-cache` | Neither an outage nor a frozen Redis fails `get_or_load`. Every call has a time limit, and an outage costs a request one limit, not the client's own retries. | `an_outage_does_not_fail_get_or_load`, `a_frozen_redis_does_not_hold_a_request_up`, `every_call_is_bounded_while_the_client_reconnects` |
| RDS-6 | `axa-redis-cache` | `Cache::memory` behaves like Redis in one process, and holds at most 32 MB. | `the_memory_cache_stores_like_redis`, `clones_of_a_memory_cache_share_it`, `the_memory_cache_refuses_a_zero_ttl`, `the_memory_cache_loads_on_a_miss_only`, `an_expired_entry_is_gone_and_its_bytes_are_returned`, `replacing_an_entry_counts_only_the_new_one`, `the_memory_cache_never_holds_more_than_its_limit`, `stale_entries_go_before_live_ones` |
| OAP-1 | `axa-openapi-docs` | The document and the page are served at their paths. | `the_document_is_served_as_json`, `the_page_is_served_as_html` |
| OAP-2 | `axa-openapi-docs` | Swagger UI is one exact version, and every remote file carries an integrity hash. | `the_page_pins_an_exact_version_with_integrity_hashes` |
| OAP-3 | `axa-openapi-docs` | A title or URL cannot inject markup. | `the_title_cannot_inject_markup`, `the_spec_url_cannot_break_out_of_the_script` |
| OAP-4 | `axa-openapi-docs` | Bearer authentication is declared and required, keeping other components. | `bearer_auth_is_declared_and_required`, `bearer_auth_keeps_other_components` |
| OAP-5 | `axa-openapi-docs` | Parameter names do not distinguish routes. | `parameter_names_do_not_change_a_shape`, `agreement_is_empty_whatever_the_parameters_are_called` |
| OAP-6 | `axa-openapi-docs` | `drift` reports a mounted route the document lacks, and a documented operation with no route. | `a_mounted_route_missing_from_the_document_is_reported`, `a_documented_operation_with_no_route_is_reported`, `a_document_with_no_paths_reports_every_route` |
| OAP-7 | `axa-openapi-docs` | A path kept out on purpose, and a path item's other members, are not reported. | `a_path_kept_out_on_purpose_is_not_reported`, `a_path_items_other_members_are_not_operations` |
| OAP-8 | `axa-openapi-docs` | `dangling_refs` reports a local `$ref` to nothing, and leaves references to other files alone. | `a_reference_that_resolves_is_not_reported`, `a_reference_to_nothing_is_reported_wherever_it_is`, `a_reference_to_another_file_is_left_alone` |
| OAP-9 | `axa-openapi-docs` | The Swagger UI page sets its own `content-security-policy`, so the production default does not blank it. | `the_page_sets_a_policy_that_lets_it_load` |
| SOR-1 | `axa-seaorm` | Pool settings reach `SeaORM`. | `pool_settings_reach_the_options` |
| SOR-2 | `axa-seaorm` | `query::condition()` builds the same conditions as Loco's, all of which must hold. | `eq_and_ne_compare_values`, `several_conditions_must_all_hold` |
| SOR-3 | `axa` | With `sea-orm`, validation failures become a `DbErr` with Loco's compact JSON text, so snapshots do not change. | `the_database_error_is_compact_json` |
| SOR-4 | `axa-seaorm` | Migrations apply, revert the requested steps, and list what is pending. `truncate` empties tables, refusing an unsafe name, and a duplicate is a unique violation. `dangerously_recreate` rebuilds from the first migration. | `migrations_apply_revert_and_list_what_is_pending`, `truncate_empties_tables_and_a_duplicate_is_a_unique_violation`, `dangerously_recreate_rebuilds_from_the_first_migration` |
| TOA-1 | `axa-toasty` | `connect` checks that the database answers, and fails on one that does not. | `connect_checks_the_database_answers` |
| TOA-2 | `axa-toasty` | Migrations apply once, and `pending` names the rest. | `migrations_apply_once_and_pending_names_the_rest` |
| TOA-3 | `axa-toasty` | `truncate` empties tables and refuses an unsafe name, a duplicate is a unique violation, and `dangerously_recreate` rebuilds from the first migration. | `truncate_empties_tables_and_refuses_unsafe_names`, `a_duplicate_is_a_unique_violation`, `dangerously_recreate_rebuilds_from_the_first_migration` |
| SES-1 | `axa-web-session` | `production` refuses a key under 64 bytes, rather than letting the session crate panic on it, and refuses two identical keys. | `a_short_key_is_refused_not_a_panic`, `the_two_keys_must_differ` |
| SES-2 | `axa-web-session` | Production cookies are `Secure`, `HttpOnly`, `SameSite=Lax`, and signed. Only `development` is unsigned. | `a_production_cookie_is_secure_and_signed`, `a_development_cookie_is_neither_secure_nor_signed` |
| SES-3 | `axa-web-session` | A live session's identifier without its signature, or with a changed one, reads nothing. | `a_real_id_with_no_signature_reads_nothing`, `a_changed_signature_reads_nothing`, `an_unsigned_cookie_can_be_forged_in_development` |
| SES-4 | `axa-web-session` | A session survives between requests, `renew` changes its identifier and keeps its data, and `sign_out` ends it. | `a_session_survives_between_requests`, `renew_changes_the_id_and_keeps_the_data`, `destroy_ends_the_session` |
| SES-5 | `axa-web-session` | Keys are never printed. | `debug_output_hides_the_keys` |
| SES-6 | `axa-web-session` | A session is not tied to the client's IP, so a phone changing networks keeps it. | `a_session_survives_a_new_ip_address` |
| SES-7 | `axa-web-session` | An anonymous visitor keeps no cookie, as they are sent only a deletion, and has no stored data and no row, unless a service asks for every visitor to be stored. | `an_anonymous_visitor_keeps_no_cookie_and_nothing_is_stored`, `anonymous_visitors_add_no_rows`, `every_visitor_can_be_stored_when_a_service_asks` |
| SES-8 | `axa-web-session` | In Postgres, session data is encrypted at rest and survives a restart of the store. | `postgres_sessions_are_encrypted_and_survive_a_restart` |
| SES-9 | `axa-web-session` | `sign_in` makes an identifier from before it worthless, keeps the session stored, and replaces the form token. | `sign_in_makes_an_identifier_from_before_it_worthless`, `sign_in_replaces_the_form_token` |
| SES-10 | `axa-web-session` | A form token is stable within a session, passes only for that session, and a session with none refuses every post. | `a_form_token_is_stable_and_only_it_passes`, `a_form_token_belongs_to_one_session`, `a_session_with_no_token_refuses_every_post`, `only_the_same_text_is_the_same` |
| PRB-1 | `axa-problem` | Responses follow RFC 9457 and use `application/problem+json`. | `defaults_follow_the_rfc`, `responses_use_the_problem_media_type` |
| PRB-2 | `axa-problem` | An extension cannot overwrite a standard member. | `reserved_names_cannot_be_overwritten_by_extensions` |
| PRM-1 | `axa-prometheus-metrics` | Metrics render in the text exposition format, with its media type. | `counters_render_in_the_exposition_format`, `the_response_uses_the_exposition_media_type` |
| PRM-2 | `axa-prometheus-metrics` | `RequestMetrics` records each request's duration by method, route template, and status, never by path. A request matching no route is `unmatched`, and a method outside the standard ones is `other`, so no client can add a label. | `requests_are_measured_by_route_template_not_path`, `registering_twice_in_one_registry_fails` |
| VER-1 | `axa-accepts-version-negotiation` | No header means the current version, and a bare major is an alias. | `no_header_means_the_current_version`, `a_bare_major_is_an_alias` |
| VER-2 | `axa-accepts-version-negotiation` | An unsupported version on an `/api` path is a `406` problem, with the `code` `unsupported_api_version` and the `requested` and `supported` extension members. | `an_unsupported_version_is_a_406` |
| VER-4 | `axa-accepts-version-negotiation` | A service can state its own versions, and a current version it does not support is a mistake found at boot. | `a_service_can_serve_its_own_versions`, `a_current_version_that_is_not_supported_is_a_mistake` |
| VER-3 | `axa-accepts-version-negotiation` | Paths outside `/api` ignore the header. | `paths_outside_api_ignore_the_header` |
| RAT-1 | `axa-request-rate-limit` | A key gets exactly its limit in a window, and a refusal is not counted. | `the_limit_is_exact_and_a_refusal_is_not_counted` |
| RAT-2 | `axa-request-rate-limit` | A refusal is a `429` problem with `Retry-After`, rounded up, and every response counted carries the `RateLimit-*` headers. | `a_refusal_says_how_long_to_wait`, `the_wait_is_rounded_up_and_never_too_short` |
| RAT-3 | `axa-request-rate-limit` | The health probes are neither refused nor counted. | `the_probes_are_never_refused_or_counted` |
| RAT-4 | `axa-request-rate-limit` | The client is the address `axa` resolved, so a forged `X-Forwarded-For` buys no fresh limit, and clients behind a trusted proxy are counted apart. | `a_forged_header_does_not_buy_a_fresh_limit`, `a_header_from_an_untrusted_peer_is_ignored`, `clients_behind_one_proxy_get_a_limit_each` |
| RAT-5 | `axa-request-rate-limit` | An IPv6 client counts by its `/64`, and an IPv4 client counts the same in either form. | `one_ipv6_subscriber_counts_once`, `an_ipv4_client_counts_the_same_however_it_arrives` |
| RAT-6 | `axa-request-rate-limit` | Memory is bounded, at `MAX_KEYS` plus the keys with a limit of their own. Past `MAX_KEYS`, under the default `WhenFull::Share`, new keys share one limit, a held key keeps its own, and a full table is scanned at most once a window. | `a_full_table_shares_one_limit_and_never_grows`, `a_table_full_of_live_keys_is_not_scanned_for_each_new_key` |
| RAT-7 | `axa-request-rate-limit` | A key is held only as a keyed hash, and the first request after two windows drops it. | `a_key_is_held_only_as_a_hash`, `old_keys_are_dropped_within_two_windows` |
| RAT-8 | `axa-request-rate-limit` | `lock_out_failures` counts only `401`s, per client, and locks out only that client. | `failures_lock_a_client_out_and_only_that_client`, `success_never_counts_towards_a_lockout` |
| RAT-9 | `axa-request-rate-limit` | A refusal from inside the stack gets the standard headers. | `a_refusal_gets_the_standard_headers` |
| RAT-10 | `axa-request-rate-limit` | `by_key` counts by the key a service's function takes from the request. A request with no key is not counted and gets no `RateLimit-*` headers, and the probes are neither refused nor counted. | `a_keyed_limit_counts_by_the_key_it_takes`, `a_request_with_no_key_is_not_counted`, `keyed_probes_are_never_refused_or_counted` |
| RAT-11 | `axa-request-rate-limit` | With the table full, `WhenFull` decides for a key not held: `Share`, the default, `Allow`, or `Refuse`. `Verdict::overflowed` says so, a held key keeps its own limit, and the table never grows. | `a_full_table_shares_by_default_and_says_so`, `a_full_table_can_let_new_keys_through`, `a_full_table_can_refuse_new_keys` |
| RAT-12 | `axa-request-rate-limit` | `Verdict::first_refusal` is true once per key per window, and again in the next window. A peek never takes it, a lockout's refusal does, and keys that overflowed share one. | `the_first_refusal_in_a_window_is_marked_once`, `a_window_that_ends_between_sweeps_clears_the_refusal`, `a_peek_never_takes_the_first_refusal`, `overflowed_keys_share_one_first_refusal`, `a_lockout_marks_its_first_refusal_and_carries_its_verdict` |
| RAT-13 | `axa-request-rate-limit` | Every response from `by_client`, `by_key`, and `lock_out_failures` carries its `Verdict` as a response extension, refused or not. The crate drives no hook. | `every_response_carries_its_verdict`, `a_lockout_marks_its_first_refusal_and_carries_its_verdict` |
| RAT-14 | `axa-request-rate-limit` | `set_limit_for` gives one key its own limit from its next request, with no restart, and `clear_limit_for` takes it back. Such a key is always held, at most `MAX_OVERRIDES` are, past that a new one is refused and none dropped, and each is held only as a keyed hash. | `a_key_can_have_a_limit_of_its_own_with_no_restart`, `a_key_with_its_own_limit_is_held_when_the_table_is_full`, `too_many_own_limits_are_refused_and_none_is_dropped`, `an_own_limit_is_held_only_as_a_hash` |
| RAT-15 | `axa-request-rate-limit` | Once per limiter, `by_client` warns when the first `SHARED_ALLOWANCE_SAMPLE` (100) requests it counts all came from one private or loopback address, saying `TRUSTED_PROXIES` must list a proxy. It does not warn for mixed or public clients, and the warning never holds the address. | `one_private_client_is_warned_about_once_and_never_named`, `a_loopback_client_is_warned_about_too`, `mixed_clients_are_not_warned_about`, `a_public_client_is_not_warned_about` |
| OTL-1 | `axa-opentelemetry` | Export is off until an endpoint is set, and off leaves requests working with no header. | `export_is_off_until_an_endpoint_is_set`, `off_means_no_layers_no_header_and_the_request_still_works` |
| OTL-2 | `axa-opentelemetry` | A span and the request metric reach a real collector, with the service's name and version. | `a_request_reaches_the_collector_as_a_span_and_a_metric` |
| OTL-3 | `axa-opentelemetry` | Plain `http` is for loopback, unless a clear yes allows it. An invalid endpoint is refused, and an error never quotes the endpoint. | `plain_http_is_for_loopback`, `plain_http_to_another_host_is_refused`, `plain_http_to_another_host_can_be_allowed`, `anything_but_a_clear_yes_keeps_insecure_refused`, `a_bad_endpoint_is_refused`, `an_error_never_quotes_the_endpoint`, `start_refuses_cleartext_to_another_host` |
| OTL-4 | `axa-opentelemetry` | An `https` endpoint trusts the system's roots. | `an_https_endpoint_builds_with_roots_to_trust`, `an_https_endpoint_completes_a_handshake_with_a_root_the_system_trusts` |
| OTL-5 | `axa-opentelemetry` | A collector that is down never holds up a request. | `a_collector_that_is_down_stops_nothing` |
| OTL-6 | `axa-opentelemetry` | A metric's attributes are the method, status, and route template. An unknown method is `_OTHER`, and a path's value never appears. | `an_unknown_method_cannot_create_a_series`, `a_request_reaches_the_collector_as_a_span_and_a_metric` |
| OTL-7 | `axa-opentelemetry` | The middleware adds to the request span `axa` opens, and puts its `traceparent` on the response. A server error marks the span. | `the_middleware_joins_the_request_span_axa_opens`, `a_server_error_marks_the_span` |
| OTL-9 | `axa-opentelemetry` | A panic and a cancelled request are counted, with `error.type` saying which. | `a_panic_and_a_cancelled_request_are_still_counted` |
| OTL-8 | `axa-opentelemetry` | `DIAGNOSTIC_DIRECTIVES` are valid filter directives. | `the_diagnostic_directives_are_valid` |
| AUD-1 | `axa-paseto-auth`, `axa-keycloak-auth` | A refused request writes one `auth_refused` line at `warn`, with status, action, and the client's address when known, inside the request's span. An allowed request writes none. | `a_refusal_leaves_one_audit_line_and_an_allowed_request_none` |
| AUD-2 | `axa-paseto-auth`, `axa-keycloak-auth` | The line never holds the token or email. A policy refusal names the subject's id and the reason. | `a_refusal_leaves_one_audit_line_and_an_allowed_request_none` |
| TRC-1 | `axa-tracing` | `Redacted` hides its value in `Display` and `Debug`, and so in a log line. | `redacted_hides_its_value_in_a_log_line` |
| TRC-2 | `axa-tracing` | `SlowSpans` reports a span at or past its threshold, inside that span so that it carries its fields, and tells busy time from idle time. A quick span is not reported. | `a_slow_span_is_reported_with_its_fields`, `a_quick_span_is_not_reported`, `busy_and_idle_time_are_told_apart` |
| TRC-3 | `axa-tracing` | `track` makes the request id readable anywhere the request runs, cleaned by `axa`'s rule, or minted when nothing usable remains. Outside a request there is none. | `a_handler_can_read_the_request_id`, `a_callers_id_is_cleaned_as_axa_cleans_it`, `an_id_is_minted_when_nothing_usable_remains`, `there_is_no_request_id_outside_a_request` |
| TRC-4 | `axa-tracing` | `propagate` adds the id to a call to a peer, and keeps a value already there. | `propagate_sends_the_id_and_keeps_one_already_set` |
| TRC-5 | `axa-tracing` | `spawn` keeps the request id and the span in the new task, and works outside a request. | `a_spawned_task_keeps_the_request_id`, `a_spawned_task_keeps_the_span`, `spawn_works_outside_a_request` |
| TRC-6 | `axa-tracing` | `Watch::watch` tells a future that blocked a runtime thread from one that only waited. | `a_blocking_future_is_reported`, `a_future_that_only_waits_is_slow_but_not_blocking`, `a_quick_future_is_not_reported` |
| TRC-7 | `axa-tracing` | A task that wakes late is reported with a snapshot of the runtime, and none of this needs `tokio_unstable`. | `a_stalled_runtime_is_reported`, `a_snapshot_describes_the_runtime` |
| TRC-8 | `axa-tracing` | `log_panics` logs where a panic happened, and never its text. | `a_panic_is_logged_without_its_text` |
| TST-1 | `axa-test` | `boot_test` hands back a service's failure to boot for tests. | `a_failed_boot_is_handed_back` |
| TST-2 | `axa-test` | A request test on a service that cannot boot fails, and its body never runs. | `a_request_test_on_a_service_that_cannot_boot_fails` |

### 4.14 Databases

Four crates reach Postgres: `axa-sqlx`, `axa-seaorm`, `axa-diesel`, and `axa-toasty`,
behind the features `sqlx`, `sea-orm`, `diesel`, and `toasty`. Each has the same shape,
so a service moves from one to another by changing names, not structure:

| Item | `axa-sqlx` | `axa-seaorm` | `axa-diesel` | `axa-toasty` |
|---|---|---|---|---|
| `Pool`, the handle a service holds | `PgPool` | `DatabaseConnection` | a `deadpool` pool | `toasty::Db` |
| `Error` | `sqlx::Error` | `DbErr` | its own | `toasty::Error` |
| `PoolConfig::new(uri)`, its `Debug` text without the URL | yes | yes | yes | yes |
| `connect`, `ping` | yes | yes | yes | yes, with the models |
| `migrate`, `pending` | yes | yes, for a `Migrator` type | yes | yes, for a `MigrationSet` |
| `revert` | yes | yes | yes | no: Toasty migrations go forward only |
| `truncate`, `dangerously_recreate` | yes | yes | yes | yes |
| `is_unique_violation`, `quote_identifier` | yes | yes | yes | yes |

| Id | Requirement | Test |
|---|---|---|
| DB-1 | `AppContext::db` is the `Pool` of the database feature that is on, and `()` with none. With more than one on, `sqlx` wins, because it is not an ORM, then `sea-orm`, `diesel`, and `toasty`. `build.rs` names the winner as `cfg(axa_db = ...)`. | `cargo clippy --all-targets` with each database feature alone and with all of them, in CI |
| DB-2 | `db::connect` opens that `Pool` from `config::Database`, and each crate's `PoolConfig` is built from it with `From`. Toasty's also takes the models. | `cargo clippy --all-targets` with each database feature alone, in CI |
| DB-3 | `/_health/ready` asks `AppContext::db` as `database`, whichever it is. | `readiness_fails_when_the_database_does_not_answer`, `readiness_names_the_database` |
| DB-4 | A violated unique or foreign key constraint answers `409`, and a check or not-null one `422`, through each of the four, and no database's own text reaches the client. | `a_violated_constraint_through_sqlx_is_the_callers_mistake`, `a_violated_constraint_through_sea_orm_is_the_callers_mistake`, `a_violated_constraint_through_diesel_is_the_callers_mistake`, `a_violated_constraint_through_toasty_is_the_callers_mistake` |
| DB-5 | `axa::model` and `axa::validation` are the same for every database, and come with any database feature. `ModelError` has a variant for each database's error. | `model_errors_answer_as_problems`, `a_validation_failure_keeps_its_field_errors` |
| DB-6 | Each crate's `pending` names the migrations not yet applied, and is empty once `migrate` has run. | `status_on_a_fresh_database_shows_nothing_applied`, `migrations_apply_revert_and_list_what_is_pending`, `migrations_apply_once_and_pending_names_the_rest` |
| DB-7 | A database that cannot serve for now answers `503` with the `code` `unavailable` and `Retry-After`, through each of the four: a pool with no free connection, a connection lost, or a read-only, shutting-down, or full server. It quotes none of the driver's text. Any other database failure is still a `500`. | `a_sqlx_pool_with_no_connection_is_unavailable`, `a_sea_orm_pool_with_no_connection_is_unavailable`, `a_read_only_diesel_database_is_unavailable`, `a_read_only_toasty_database_is_unavailable`, `a_read_only_database_is_unavailable_not_a_failure` |

### 4.15 Conditional requests

`axa::conditional` follows RFC 9110. Comparing costs nothing, so it is in the standard
stack, and it changes an answer only where a handler set an `ETag` and the caller sent
`If-None-Match`. Hashing a body costs time and suits only some routes, so a route or
router opts in. Most routes that want a tag have a timestamp in the body and need a tag
of their own, and some services send `no-store` on every `/api` answer, which a hash
cannot help. It is a module and not a subcrate because `apply` calls it.

| Id | Requirement | Test |
|---|---|---|
| CND-1 | The standard stack answers `304` to a `GET` or `HEAD` holding the `ETag` of its answer, whoever set that tag. The comparison is weak, reads a list over any number of header lines with commas inside tags allowed, and `*` alone matches any success, tagged or not. | `a_caller_holding_the_tag_gets_a_304_with_no_body`, `a_caller_with_an_old_tag_or_none_gets_the_whole_answer`, `lists_and_weak_tags_compare_weakly`, `two_header_lines_read_as_one_list`, `a_star_matches_an_answer_with_no_tag`, `a_head_is_compared_like_a_get`, `weak_comparison_ignores_who_is_weak`, `a_list_is_read_with_commas_inside_tags_and_empty_members`, `a_list_can_run_over_several_header_lines`, `a_star_stands_for_anything_but_only_alone`, `bytes_outside_ascii_are_compared_as_bytes` |
| CND-2 | An `If-None-Match` that cannot be read holds nothing, so the caller gets the whole answer. | `what_is_not_a_tag_matches_nothing`, `lists_and_weak_tags_compare_weakly` |
| CND-3 | A `304` has no body and no header describing one, `content-length` included, and keeps every other header the answer had, so `etag`, `cache-control`, `vary`, `expires`, `content-location`, `set-cookie`, and a count header survive. | `a_304_keeps_what_the_200_said_and_nothing_about_a_body`, `a_304_is_sent_with_no_body_on_a_real_connection` |
| CND-4 | The comparison sits outside compression, so a `304` has the `vary` its `200` would have had, and inside everything else, so it gets the request id, security headers, and CORS headers. | `a_304_has_the_vary_compression_would_have_added`, `a_304_gets_everything_the_stack_adds` |
| CND-5 | Only a `2xx` to a `GET` or `HEAD` is compared. A write has done its work by then and is never touched. | `only_a_success_is_compared`, `a_write_is_never_answered_with_a_304` |
| CND-6 | A `304` is made from the answer the caller was about to get. Nothing is stored and nothing short-circuits, so a refused caller gets the refusal whatever they hold, wherever the auth layer sits, and a tag covers what this caller may see. | `a_caller_who_is_refused_gets_the_refusal_whatever_they_hold`, `a_tag_covers_what_this_caller_may_see` |
| CND-7 | The comparison reads the tag on the outgoing answer, so a layer inside it that removes the tag, as paging does on a partial page, means no `304`. | `a_layer_that_takes_the_tag_away_means_no_304` |
| CND-8 | `conditional::etag`, added to a router or a route, gives a `GET`'s `200` a weak tag from a hash of its body, the same on every replica. It covers what passes through it, so outside paging it covers the page. | `an_answer_gets_a_weak_tag_from_its_body_and_then_a_304`, `a_changed_body_has_a_new_tag`, `a_hash_covers_only_a_gets_200`, `one_route_can_opt_in_alone`, `a_hash_outside_paging_covers_the_page`, `a_digest_is_the_same_each_time_and_differs_by_content` |
| CND-9 | `conditional::etag` leaves alone an answer with an `ETag`, with `no-store`, with an empty body, or already encoded. A handler's own tag still works on a `no-store` answer. A `HEAD` gets no generated tag, as RFC 9110 allows, because Axum has dropped the body. | `a_handlers_own_tag_is_left_alone`, `no_store_gets_no_generated_tag_and_a_handlers_tag_still_works`, `no_store_is_read_as_a_directive`, `an_empty_answer_gets_no_tag`, `an_already_encoded_answer_is_not_hashed`, `a_head_gets_no_generated_tag_and_keeps_its_length` |
| CND-10 | `conditional::etag` hashes plain bytes, inside compression, so the tag is weak and the same for every encoding, and a tag held from one encoding matches on another. A handler's strong tag is left alone when compressed. | `the_tag_is_the_same_whichever_encoding_is_asked_for`, `a_handlers_strong_tag_is_left_alone_when_its_answer_is_compressed` |
| CND-11 | `conditional::etag` reads only a body of exact known size, up to `MAX_BODY` (8 MB), which is nearly always in memory already. A stream and server-sent events pass through unread, and a larger body untagged. A body that fails while being read is the generic `500`. | `a_stream_is_never_buffered`, `server_sent_events_reach_the_caller_as_they_are_sent`, `a_body_at_the_limit_is_tagged_and_one_past_it_is_not`, `a_body_that_fails_is_a_generic_500` |
| CND-12 | A timestamp or request id in a body changes the hash each time, so a caller never matches and gets the whole answer, which gains nothing and harms nothing. A handler can leave it out with its own tag. | `a_timestamp_in_the_body_never_matches_and_does_no_harm`, `a_handler_can_leave_a_timestamp_out_of_its_tag` |
| CND-13 | `ETag` builds a weak or strong tag from text, refusing what a tag cannot hold, or from a value's JSON, where `ETag::weak_of` gives what `conditional::etag` gives the same JSON. | `a_tag_holds_only_what_a_tag_can_hold`, `weak_of_gives_the_tag_the_layer_gives_the_same_json` |
| CND-14 | `conditional::if_match` passes a write with the current tag or `*`, or with no `If-Match`. It compares strongly, so a weak tag on either side fails. Anything else, including what cannot be read, is a `412` problem with the `code` `precondition_failed`. | `a_write_with_the_current_tag_goes_through`, `a_stale_write_is_a_412_problem_and_writes_nothing`, `a_weak_current_tag_never_passes_if_match`, `strong_comparison_refuses_any_weak_tag` |
| CND-15 | `conditional::require_if_match` does the same, and a missing `If-Match` is a `428` problem with the `code` `precondition_required`. `conditional::if_match_holds` gives the answer alone, for a service with its own refusal. | `if_match_is_optional_unless_required`, `a_service_can_give_its_own_answer_to_a_stale_write` |
| CND-16 | With origins listed, CORS exposes `etag`, so a page can read a tag to send back. | `a_page_on_a_listed_origin_can_read_the_tag` |

`If-Modified-Since`, `If-Unmodified-Since`, `If-Range`, and `If-None-Match` on a write are
not handled: no service sends them or sets `Last-Modified`, and on a write it would need
checking before the work, in the handler.

### 4.16 Idempotency keys

`axa-idempotency-key` runs a write once per caller and `Idempotency-Key`. A retry gets the
first answer. It is one layer, and a service adds it after `into_router`, before its
guards, and before `axa::middleware::apply`, so that the layer sits inside compression
and inside whatever refuses a caller. [Where the layer goes](../help/idempotency-keys/index.md)
says why. The crate holds no trait for a service to implement: a closure given to
`Idempotency::new` names the caller, and a service calls `sweep`, `forget`, and
`forget_caller` when it chooses. Nothing runs by itself.

The table is `axa_idempotency`. The crate does not own a service's schema, so a service
runs `SETUP_STATEMENTS` in a migration, and `check` tells it at boot if that was missed.

A run that ends without an answer, by a crash, a hang-up, a panic, or a timeout, may
have done its work, so by default its claim holds the key for the key's whole `ttl`
(P-10). A service that would rather run the write again after a crash says so with
`rerun_abandoned_after`, which names what it allows.

The guarantee has one known gap: the answer is stored after the handler commits, in a
separate transaction, so a crash between the two leaves the key held and no answer to
replay. A retry is told to wait until the claim's lease ends, and with
`rerun_abandoned_after` it then runs the handler again. A handler that must not repeat
needs a unique constraint of its own.

The Postgres tests need `AXA_TEST_DB_URL`.

| Id | Requirement | Test |
|---|---|---|
| IDM-1 | A request with no key that `required` does not name, or a `GET`, `HEAD`, `OPTIONS`, or `TRACE`, runs as if the layer were not there, reaches no store, and leaves no row. | `with_no_key_nothing_is_kept_and_every_request_runs`, `a_method_that_changes_nothing_ignores_its_key`, `the_store_being_down_stops_the_request_before_it_runs` |
| IDM-2 | The same key and request after a finished `2xx` or `3xx` gets the first status, body, and the headers in `KEPT_HEADERS`, with `Idempotent-Replayed: true`, and runs nothing. A `set-cookie` is not kept. | `a_retry_gets_the_first_answer_and_runs_nothing`, `a_redirect_is_kept_as_the_write_it_answers`, `an_empty_answer_replays_as_an_empty_answer`, `a_replay_hands_out_no_cookie`, `only_the_listed_headers_are_kept`, `a_replay_is_the_first_answer_and_says_so`, `an_empty_answer_has_a_known_size_and_is_kept` |
| IDM-3 | The same key with a different method, path, query, or body is a `422` with the `code` `idempotency_key_reused`, whether the first finished or not, and runs nothing. The request is compared by a SHA-256 hash that also marks where each part ends. | `the_same_key_on_a_different_request_is_refused`, `a_duplicate_while_the_first_runs_is_told_to_wait`, `the_fingerprint_sees_every_part_and_where_each_ends`, `what_a_key_holds_decides_the_answer` |
| IDM-4 | Of any number of requests with one key, on any number of replicas, exactly one runs. A duplicate while it runs is a `409` with the `code` `idempotency_in_progress`, and its `Retry-After` is the seconds the first has run so far, rounded up, at least 1 and at most the seconds left on its lease. | `of_many_requests_at_once_on_two_replicas_exactly_one_runs`, `a_duplicate_while_the_first_runs_is_told_to_wait`, `a_duplicate_of_a_long_run_is_told_to_wait_longer_and_never_past_its_lease`, `a_duplicate_waits_as_long_as_the_first_has_run_and_no_longer_than_its_lease`, `obeying_the_wait_asks_about_a_lost_run_a_handful_of_times_a_day` |
| IDM-5 | A key belongs to one caller. Two callers with one key run separately, and neither can read the other's answer. | `one_key_is_two_keys_for_two_callers` |
| IDM-6 | A keyed request with no caller, or an empty one, is a `401` with the `code` `idempotency_no_caller`, runs nothing, and leaves no row. | `with_no_caller_a_keyed_request_is_refused_and_does_not_run` |
| IDM-7 | Mounted as stated, a guard refuses a caller before a replay can answer it. | `a_guard_outside_the_layer_refuses_before_a_replay_can_answer` |
| IDM-8 | A `4xx` or `5xx` frees the key, so a retry under it runs, and a refusal from the standard stack does too. | `a_failure_releases_the_key_so_a_retry_runs`, `a_failure_is_released_and_a_success_or_redirect_is_kept`, `the_service_body_limit_still_applies_and_its_refusal_frees_the_key` |
| IDM-9 | Where a service chose a lease with `rerun_abandoned_after`, a claim whose run stopped reporting in, by a crash, a hang-up, a panic, or the stack's timeout, can be taken over by the same request once that lease ends, and by exactly one such request. | `a_claim_abandoned_by_a_crash_is_taken_over_by_the_same_request`, `of_many_retries_of_an_abandoned_claim_exactly_one_takes_it`, `a_cancelled_run_stops_reporting_in_and_its_key_comes_free`, `a_panic_is_answered_and_its_key_comes_free_after_the_lease`, `a_request_the_stack_cuts_off_frees_its_key_after_the_lease` |
| IDM-10 | A different request cannot take over an abandoned claim, as the lost run may have done its work. | `a_different_request_cannot_take_over_an_abandoned_claim` |
| IDM-11 | A run reports in every third of its lease, so that it can run for as long as it needs without being taken for lost. The lease is the `ttl` unless a service chose a shorter one. | `a_run_that_reports_in_outlives_its_lease` |
| IDM-12 | A run that lost its claim cannot overwrite the answer of the run that took over. | `a_run_that_lost_its_claim_cannot_overwrite_the_next_runs_answer` |
| IDM-13 | Mounted as stated, the plain body is stored, and each replay is compressed or not as its caller asked. | `inside_compression_the_plain_body_is_stored_and_each_caller_gets_what_it_asked_for` |
| IDM-14 | An answer with a `content-encoding` is never stored, wherever the layer sits. The caller gets it, and a retry is a `409` with the `code` `idempotency_answer_not_kept`. | `outside_compression_a_compressed_answer_is_never_stored_or_replayed`, `an_answer_a_handler_compressed_is_sent_and_not_kept`, `what_cannot_be_replayed_is_not_kept` |
| IDM-15 | An answer of unknown size is a stream, and one over `max_response_bytes`, 1 MiB by default, is too large. Either is passed on without being read, nothing of it is stored, and a retry is the `409` of IDM-14, because the write ran. | `a_stream_is_passed_on_and_not_kept`, `an_answer_over_the_limit_is_sent_and_not_kept`, `what_cannot_be_replayed_is_not_kept` |
| IDM-16 | A keyed request over `max_request_bytes`, 1 MiB by default, is a `413` with the `code` `idempotency_body_too_large`, and runs nothing. The limit does not apply to a request with no key. | `a_body_over_the_limit_is_refused_before_it_runs` |
| IDM-17 | A key must be one header value of 1 to 255 visible ASCII characters. Anything else is a `400` with the `code` `idempotency_key_invalid`, and runs nothing. A key is never trimmed or cut. | `a_key_that_is_not_one_clean_value_is_refused`, `a_key_is_one_value_of_visible_ascii_within_the_limit`, `a_key_is_never_cut_so_two_keys_cannot_become_one` |
| IDM-18 | A store that cannot be reached is a `503` with the `code` `idempotency_unavailable` and no `detail`, and the request does not run. | `the_store_being_down_stops_the_request_before_it_runs` |
| IDM-19 | A store that fails after the work still gives the caller its answer, and logs an error. The claim is then left to its lease, as in IDM-31. | `the_store_failing_after_the_work_still_answers_the_caller` |
| IDM-20 | A key lives for `ttl`, 24 hours by default, from when its run finished. Past that an answer is never replayed, swept or not, and any request may use the key. | `an_expired_answer_is_not_replayed_and_the_key_runs_afresh`, `an_expired_answer_is_never_replayed` |
| IDM-21 | `sweep` deletes what has expired, and leaves a live key and a run still reporting in. | `a_sweep_deletes_what_has_expired_and_nothing_else` |
| IDM-22 | `forget` frees one finished key of one caller, for a job that failed after its `202`, and leaves a run in progress alone. | `forget_lets_a_retry_run_once_a_job_has_failed`, `forget_leaves_a_run_in_progress_alone` |
| IDM-23 | `forget_caller` deletes all that is held for one caller and nothing else. | `erasing_a_caller_deletes_what_is_held_for_them_and_for_nobody_else` |
| IDM-24 | Of a request, a row holds the caller, the key, and a hash. Its path, query, and body are not stored. | `the_row_holds_no_request_and_a_copy_of_the_answer` |
| IDM-25 | No log line or `Debug` text holds a key, a caller, or a body. | `a_log_line_holds_no_key_caller_or_body`, `the_debug_text_holds_only_settings` |
| IDM-26 | Every refusal is a problem with its `code`, and with the standard stack it gets the standard headers. | `a_refusal_gets_the_standard_headers`, `a_duplicate_while_the_first_runs_is_told_to_wait` |
| IDM-27 | `SETUP_STATEMENTS` alone make a working table, and each is safe to repeat. `setup` runs them under an advisory lock, so services booting together all succeed. `check` fails when the table is missing. | `the_setup_statements_alone_make_a_working_table`, `services_booting_together_all_set_up`, `check_finds_a_missing_table` |
| IDM-28 | `only` narrows what is covered, and cannot widen it to a method that changes nothing. | `only_narrows_what_is_covered_and_cannot_widen_it` |
| IDM-29 | A row that cannot be read as an answer is not guessed at. It is the `503` of IDM-18, and the request does not run. | `a_row_that_cannot_be_read_is_not_guessed_at` |
| IDM-30 | `required` refuses a covered request with no key with a `400` and the `code` `idempotency_key_required`, runs nothing, and leaves no row. A key stays optional by default, and `required` cannot widen what is covered. | `a_required_key_refuses_a_request_without_one`, `a_required_key_cannot_widen_what_is_covered` |
| IDM-31 | By default a claim's lease is the key's `ttl`, so a run that stopped reporting in holds its key for a whole `ttl` from when it last did, and the write never runs twice under one key. Until then a retry is the `409` of IDM-4. After it the key is free, as an expired key is. | `the_lease_is_the_ttl_unless_a_shorter_one_is_chosen`, `by_default_a_lost_run_holds_its_key_for_the_whole_ttl`, `by_default_a_panic_holds_its_key_and_a_retry_is_told_to_wait` |
| IDM-32 | A shorter lease is only ever passed, as `rerun_abandoned_after`. One under 60 ms, which the heartbeat cannot keep, or longer than the `ttl`, is refused at boot with a panic, in whichever order the two are set, and so is a `ttl` under 60 ms. | `a_lease_longer_than_the_ttl_is_refused`, `a_ttl_shorter_than_its_lease_is_refused`, `a_lease_the_heartbeat_cannot_keep_is_refused`, `a_ttl_the_heartbeat_cannot_keep_is_refused` |

### 4.17 Outbox

`axa-transactional-outbox` keeps each event as a record, written in the caller's transaction, and a
relay decides whether it may go, publishes it, and records what happened. A decision, a
publisher, and a record are closures passed to `Outbox::run`, or a service runs its own
loop on `claim` and `delivered`, so there is no trait to implement and no broker client
here. Delivery is at least once: a consumer must dedupe on `event_id`, which
`inbox::seen` does.

`axa_outbox_events`, `axa_outbox_attempts`, `axa_outbox_payloads`, and `axa_inbox` are
records (P-7), and are only ever inserted into. `axa_outbox_owed`, which the database
derives from the attempts, and `axa_outbox_leases`, where a relay reports in, are working
state, and change only as OBX-39 allows. An event's state is its latest attempt: `claimed`, `delivered`,
`refused`, `rejected`, `failed`, `lost`, `abandoned`, `returned`, `released`, `erased`,
or `imported`.

These tests need `AXA_TEST_DB_URL`, except where the name is in `src/`.

| Id | Requirement | Test |
|---|---|---|
| OBX-1 | An event commits or rolls back with the caller's transaction, through sqlx or SeaORM. Writing takes a transaction and not a pool, so that the type refuses the mistake. A repeated `event_id` fails the write. | `an_event_rolls_back_with_its_transaction`, `an_event_commits_with_its_transaction`, `a_seaorm_transaction_carries_an_event_too`, `a_repeated_event_id_fails_the_write`, `each_event_gets_its_own_id` |
| OBX-2 | A crash after the commit loses nothing, and neither does a relay killed mid-batch. | `an_event_committed_before_a_crash_is_still_published`, `a_relay_killed_mid_batch_loses_nothing` |
| OBX-3 | An event is `delivered` only after its publisher answers `Ok`. An event caught between the broker and the ack is published again, and that is the one duplicate a crash leaves. | `a_failed_publish_is_not_delivered`, `a_relay_killed_mid_batch_loses_nothing` |
| OBX-4 | Each event is claimed by one relay at a time, using `FOR UPDATE SKIP LOCKED`, so several relays publish each event once in the normal case. | `several_relays_publish_each_event_once`, `two_claims_at_once_share_nothing` |
| OBX-5 | A lost claim is found from its attempts: a `claimed` attempt with no later one, whose lease has not been heard from for `stale_minutes`. Recovery appends a `lost` attempt, charged, or `abandoned` with no attempts left, and edits no record. Every `claim` does this first. A relay reports in every `heartbeat_interval`, across a whole batch, so a live one is not taken for lost. | `a_claim_not_heard_from_comes_back`, `an_event_that_keeps_killing_relays_is_abandoned`, `a_slow_publish_that_reports_in_is_not_taken_for_lost`, `a_batch_of_quick_publishes_reports_in_too` |
| OBX-6 | A relay given up on cannot change an event's later state by failing or returning it. | `a_late_failure_changes_nothing`, `a_late_return_changes_nothing` |
| OBX-7 | An event the broker refuses holds up no other key. After `max_attempts` it is `failed`, keeping its payload, and frees its key. Putting it back is `release(tx, event_id, by, reason)`, a `released` attempt naming who and why. A refusal by the service's decision is `refused`, and nothing puts it back (OBX-25). | `a_poison_event_does_not_hold_up_other_keys`, `an_event_out_of_attempts_is_kept_and_frees_its_key`, `a_failed_event_can_be_released_by_a_named_person` |
| OBX-8 | Events with one topic and key go out in `id` order, across relays, through retries and recovery. A retry or a hold holds back only its own key. A key's order assumes each transaction changes the same row before writing, and does not hold across an event that ended as `failed`. | `a_key_goes_out_in_order_across_relays`, `one_topic_and_key_is_one_line`, `a_retry_holds_back_only_its_own_key`, `a_recovered_event_goes_before_those_behind_it` |
| OBX-9 | A publisher that says the broker is unavailable charges no event. The round stops, the rest of the batch goes back, and the relay waits twice as long each time, up to a minute. The events go out once the broker is back. The event being tried waits `retry_backoff`, so the next round tries another, and a refusal called an outage holds up only its own key. | `an_outage_charges_nothing_and_stops_the_round`, `the_events_go_out_once_the_broker_is_back`, `a_refusal_called_an_outage_holds_up_only_its_key`, `an_outage_waits_longer_each_time_up_to_a_minute` |
| OBX-10 | A rejected event waits `retry_backoff`, doubling each time up to an hour. What is kept is a reason, a code (OBX-32). Free error text is never stored. | `a_retry_waits_for_its_backoff`, `each_retry_waits_twice_as_long_up_to_a_limit`, `a_long_reason_is_kept_as_a_code` |
| OBX-11 | A publisher that panics has rejected that event, and the loop carries on. | `a_publisher_that_panics_fails_that_event_and_the_loop_carries_on` |
| OBX-12 | A publish past `publish_timeout` is charged an attempt and stops the round as an outage does, as a hang can be either. | `a_publish_past_its_limit_is_charged_and_stops_the_round` |
| OBX-13 | Removed in 0.2.0. Nothing is swept (OBX-30). | `nothing_is_swept` |
| OBX-14 | A log line holds no payload, key, reason, or publisher's error text, and the `Debug` text of `Event`, `Claimed`, and `Attempt` leaves out the payload and the key. | `no_log_line_holds_a_payload_a_key_or_a_failure_text`, `debug_text_leaves_out_the_payload_and_the_key` |
| OBX-15 | Relays booting together all connect, and `Outbox::connect_existing` changes no schema. A migration runs `SETUP_STATEMENTS`. | `relays_booting_together_all_connect`, `connecting_to_existing_tables_changes_no_schema` |
| OBX-16 | Events already delivered do not slow a claim or `stats`: a claim reads only what is owed, and `stats` only that and the last hour of outcomes. | `what_is_owed_is_found_without_reading_delivered_events` |
| OBX-17 | On shutdown a publish in progress finishes and the rest of the batch goes back, charged nothing. A relay started beside `server::run` and stopped after it survives a real `SIGTERM`, and the process exits with success. A relay whose sender is gone stops rather than spins. | `a_publish_in_progress_finishes_and_the_rest_go_back`, `sigterm_lets_a_publish_in_progress_finish_beside_a_server`, `a_relay_stops_while_waiting_for_work`, `a_relay_nobody_can_stop_stops` |
| OBX-18 | `Outbox::stats` says what is owed, held, and failed, and how long the oldest of each has waited (OBX-24). | `stats_count_every_outcome` |
| OBX-19 | A service can do all of this from its own loop, with `claim`, `delivered`, `failed`, `refused`, `returned`, and `heartbeat`. Each outcome takes the caller's transaction (OBX-28). | `a_service_can_drive_the_outbox_from_its_own_loop` |
| OBX-20 | Options that cannot work are refused before connecting. | `options_that_cannot_work_are_refused`, `refused_options_stop_a_connect_before_it_reaches_the_network`, `the_default_options_are_valid` |
| OBX-21 | The crate depends on no broker client. | `this_crate_depends_on_no_broker_client` |
| OBX-22 | `connect_existing` refuses to start unless every table, its indexes, the fingerprint function, and every trigger are present, the guards of OBX-39 included, with the refusing triggers set to fire always. `connect` creates all of them. There is no option to run without the triggers. | `a_missing_trigger_stops_the_relay`, `connect_makes_the_triggers` |
| OBX-23 | Events and attempts are only ever inserted. `UPDATE`, `DELETE`, and `TRUNCATE` on either, and on payloads, are refused by the database, whoever sends them and with `session_replication_role` set to `replica`, apart from the one erasure (OBX-30). | `the_tables_refuse_every_change` |
| OBX-24 | An event's state is read from its latest attempt, and never stored on the event. `stats` counts `pending`, `claimed`, `held`, and `failed`, of which `tampered`, and `delivered` and `refused` in the last hour, with the age of the oldest pending, held, and failed. | `state_is_read_from_the_attempts`, `stats_count_every_outcome` |
| OBX-25 | `Refuse(reason)` is final. The event is never claimed again, `release` refuses it, and it frees its key as `delivered` does. | `a_refusal_is_final`, `a_refusal_cannot_be_released`, `a_refusal_frees_its_key` |
| OBX-26 | `Hold` writes no attempt, charges nothing, does not grow the backoff, and leaves the event to be decided again after `hold_interval`, for as long as the hold lasts. A held event holds back its own key, and no other. A hold has no limit and changes no state: `stats` gives the count and the age of the longest hold. | `a_hold_charges_nothing`, `a_hold_lasts_as_long_as_it_is_given`, `a_hold_holds_back_only_its_key` |
| OBX-27 | `Outbox::run` takes a decision closure, run before the publisher in the claim's transaction, answering `Deliver`, `Refuse(reason)`, or `Hold`. It is required. A service with no rule passes `decide::deliver_all`, which says so in its own code. A refused event never reaches the publisher. A decision that fails leaves its event as it was, to be decided again after `hold_interval`. | `a_refused_event_never_reaches_the_publisher`, `a_failed_decision_leaves_the_event_as_it_was`, and the `compile_fail` example on `Outbox::run` |
| OBX-28 | `Outbox::run` takes a record closure, run after each outcome is written, in that outcome's transaction, before it commits. It is required. A service with nothing to audit passes `record::nothing`. If it fails, the outcome rolls back, the event stays claimed to be recovered, and the rest of the batch goes back. Every outcome method of a service's own loop takes the caller's transaction, and there is no form that takes a pool. | `an_outcome_and_its_audit_commit_together`, `a_failed_record_rolls_back_the_outcome` |
| OBX-29 | Every attempt names its relay and its claim, and an outcome must match the claim it ends, never an event's id alone. A relay given up on still cannot write a later outcome over the relay that took over (OBX-6). Its late success is kept as an attempt of its own, so a duplicate delivery is on record, not merged into another. | `each_attempt_names_its_relay_and_claim`, `a_late_success_is_kept_as_its_own_attempt`, `an_outcome_must_match_its_claim` |
| OBX-30 | Nothing is swept, and there is no `retention_days`. A payload lives in `axa_outbox_payloads`, and one that must go is erased with `erase(tx, event_id, by, reason)`: it appends an `erased` attempt, then sets the payload to null in the caller's transaction. The trigger allows that one change, and only once the event has an `erased` attempt. An erased event is no longer owed, so it is never claimed, checked, or published. Its fingerprint stays, and is never read again. | `nothing_is_swept`, `an_erasure_blanks_only_the_payload_and_is_kept`, `nothing_else_can_blank_a_payload`, `an_erased_event_is_not_published_and_is_not_tampered` |
| OBX-31 | A fingerprint, SHA-256 over the event's id, topic, kind, key, and stored payload, is made by the database at write and checked before the decision. An event that no longer matches is `failed: tampered`, never published, and counted in `stats`. | `a_changed_row_is_never_published`, `a_sealed_payload_moved_to_another_event_is_tampered` |
| OBX-32 | A reason is at most 200 characters with no line breaks, because it is a code and not content. A longer one is kept as `reason_too_long`, and one with a line break as `reason_not_a_code`, logged without its text. The table refuses either. `release` and `erase` refuse a `by` or `reason` that is not a code. | `a_reason_is_a_short_code`, `a_long_reason_is_kept_as_a_code`, `a_failed_event_can_be_released_by_a_named_person` |
| OBX-33 | A payload over `max_payload_bytes`, 64 KiB by default, is refused at write, so the caller's transaction fails rather than storing a large copy of personal data. | `a_large_payload_fails_the_write`, `a_large_payload_or_a_denied_field_is_refused_before_any_write` |
| OBX-34 | With the `encryption` feature, a `Writer` given `Keys` seals each payload with XChaCha20-Poly1305 and a random nonce, and stores only the sealed form. The key comes from the service, is at least 32 bytes, its `Debug` text leaves it out, and its key material is wiped from memory when it is dropped. A sealed payload names its key's id, so older keys still open it, and is bound to its event's id, so it does not open on another. A relay without the key leaves a sealed event as it was, charging and writing nothing. | `a_sealed_payload_is_stored_encrypted_and_published_plain`, `a_relay_without_the_key_leaves_sealed_events_alone`, `a_sealed_payload_moved_to_another_event_is_tampered`, `a_sealed_payload_opens_only_for_its_event_and_with_its_key`, `an_older_key_still_opens_what_it_sealed`, `each_seal_has_its_own_nonce`, `a_short_secret_is_refused`, `debug_text_leaves_out_the_key` |
| OBX-35 | `WriteOptions::deny` names fields, empty by default, and a write refuses a payload with any of them as an object key at any depth, so the caller's transaction fails. | `a_denied_field_fails_the_write`, `a_denied_field_is_found_at_any_depth` |
| OBX-36 | `inbox::seen(tx, event_id)` notes an event in the consumer's transaction and says whether it was noted before, so a consumer applies each event once. A note rolled back is not seen. `axa_inbox` is a record, and refuses every change. | `the_inbox_says_what_was_seen`, `a_rolled_back_note_is_not_seen`, `the_inbox_refuses_every_change` |
| OBX-37 | A topic given `Order::None` in `topic_order` has no order per key, so one stuck event holds back nothing. Every other topic keeps OBX-8. | `a_topic_with_no_order_is_not_held_back`, `the_unordered_topics_are_those_given_none` |
| OBX-38 | `IMPORT_STATEMENTS` copy each 0.1.0 `axa_outbox` row into the record with a fingerprint and one attempt for its state, keeping its order, its payload, and its count of attempts, and none of its error text. They are safe to run twice, and leave `axa_outbox` as it was. | `the_old_table_imports_with_its_states` |
| OBX-39 | `axa_outbox_owed` changes only as the record does. Its guard, set to fire always, refuses `INSERT`, `DELETE`, and `TRUNCATE` except from within a trigger, as the record's own are, and an `UPDATE` from outside one unless it changes only a hold, `held_since` and `recheck_at`, to no later than a day ahead, so a hold set by hand delays an event by at most a day and shows in `stats`. `hold_interval` is at most a day. `axa_outbox_leases` refuses a lease heard from in the future, or moved to another claim or relay. A lease changed otherwise, or deleted, can at worst bring its claim's recovery early, which is a `lost` or `abandoned` attempt on record. One kept fresh by hand keeps a claim alive as its relay would, and the event shows in `stats` as claimed, with a rising `oldest`. `REBUILD_OWED_STATEMENTS` make `axa_outbox_owed` again from the record, without its holds. A role that can create a trigger can write through one, so a service's role is kept from creating triggers. | `the_owed_table_changes_only_with_the_record`, `a_changed_lease_can_only_bring_a_recovery_which_is_recorded`, `rebuilding_owed_restores_it_from_the_record`, `connect_makes_the_triggers`, `a_missing_trigger_stops_the_relay`, `options_that_cannot_work_are_refused` |

### 4.18 Worker pool, job status, and schedules

`axa-postgres-job-queue` holds the queue's engine: its tables and SQL, the job loops, and what is
below. It does not depend on `axa`, so its handlers are closures over JSON, its error is
its own, and `axa::queue` adapts both. These tests need `AXA_TEST_DB_URL`, except
where the name is in `src/`.

`axa_queue` and `axa_schedule` are working state, and change as jobs run. `axa_job_acts`
is a record (P-7) of what a person did to a job, and is only ever inserted into.

| Id | Requirement | Test |
|---|---|---|
| WRK-1 | A process runs at most `workers` jobs at once. | `a_process_runs_no_more_jobs_at_once_than_its_loops` |
| WRK-2 | `limit` caps one job type in a process, at least one, and other types keep the remaining loops. A limit is per process, not across replicas. | `a_limit_holds_one_job_type_and_the_rest_keep_the_loops` |
| WRK-3 | Any number of processes run each job once. | `two_processes_run_each_job_once` |
| WRK-4 | Jobs in progress finish at shutdown. Dropping `run` at a time limit stops its jobs too, and leaves them for stale recovery. | `jobs_in_flight_finish_at_shutdown_and_dropping_the_run_stops_them` |
| WRK-5 | A job delayed by `run_in` or `run_at` is not claimed before its time, on the database's clock. | `a_delayed_job_is_not_claimed_before_its_time` |
| WRK-6 | A ready job with a higher priority is claimed first, and equal priorities oldest first. | `a_higher_priority_runs_first` |
| WRK-7 | `job` gives a job's state, attempts, priority, time limit, times, and last error, never its arguments, and `None` for no such job. A kept error is at most 1024 bytes. | `a_job_status_follows_it_and_leaves_out_its_arguments`, `a_long_error_is_cut_on_a_character_boundary` |
| WRK-8 | `cancel(tx, id, by, reason)` stops only a job still queued, once, and a cancelled job never runs. Cancelling while loops claim either cancels a job or lets it run, never both. | `a_cancelled_job_never_runs_and_a_running_one_cannot_be_cancelled`, `cancelling_while_loops_claim_runs_each_job_or_cancels_it_never_both` |
| WRK-9 | `retry(tx, id, by, reason)` queues a failed job, or one in review, for one more try, and a run given up on still cannot write over it. | `a_failed_job_retried_gets_one_more_try_and_a_lost_run_cannot_overwrite_it` |
| WRK-10 | `enqueue_all` queues a batch whole or not at all, and `enqueue_all_on` rolls back with its transaction. | `a_batch_is_queued_whole_or_not_at_all` |
| WRK-11 | A job has no time limit unless it is given one. Past its limit a run is stopped and fails with `timed out after` and the limit. | `a_job_has_no_time_limit_unless_it_is_given_one` |
| WRK-12 | A run whose claim is taken, or that cannot report in for `stale_minutes`, is stopped, and its job is marked `needs_review` with the reason. So is one that finishes and finds its claim gone. Stale recovery, retention, and attempts never touch it, and only `retry` releases it, kept as a `released` act (P-8). | `a_run_that_loses_its_claim_is_stopped_and_waits_for_review`, `a_run_that_finishes_after_losing_its_claim_waits_for_review` |
| WRK-13 | `counts` says how many jobs are in each state, and how long the oldest ready one has waited. | `a_delayed_job_is_not_claimed_before_its_time`, `a_run_that_loses_its_claim_is_stopped_and_waits_for_review` |
| WRK-14 | `connect_existing` fails on tables without the `priority` and `timeout_ms` columns, the `axa_schedule` table, or the `axa_job_acts` table and its triggers, set to fire always, and changes no schema. `connect` makes them all. There is no option to run without the triggers. | `connecting_to_tables_without_the_new_columns_fails_and_changes_nothing`, `a_missing_or_turned_off_acts_trigger_stops_the_queue` |
| WRK-15 | Across any number of processes ticking at once, each due time of a schedule is queued once, as an ordinary job. | `replicas_ticking_together_queue_each_due_time_once`, `a_scheduled_job_is_an_ordinary_job_and_shutdown_stops_the_ticks` |
| WRK-16 | Times missed while every process was down queue one job, and the schedule keeps its place on the grid of its first time. | `missed_times_queue_one_job_and_the_schedule_keeps_its_place` |
| WRK-17 | A due time is skipped while the schedule's last job is queued or running, and is not skipped after it completed, failed, was cancelled, or went to review. | `a_due_time_is_skipped_while_the_last_job_has_not_finished` |
| WRK-18 | Due times count from the first, not from when a job ran, so they do not drift. | `due_times_count_from_the_first_and_do_not_drift` |
| WRK-19 | `first_at` applies only when the schedule's row is made. | `a_first_time_applies_only_when_the_row_is_made` |
| WRK-20 | Shutdown stops the ticks. | `a_scheduled_job_is_an_ordinary_job_and_shutdown_stops_the_ticks` |
| WRK-21 | A schedule no live process has seen for `stale_minutes` is archived, once, never deleted, and queues nothing. Its queued job still runs, `schedules` lists it, and a schedule of that name in code restores the same row, keeping its place. | `a_schedule_gone_from_code_is_archived_kept_and_restored_when_it_returns` |
| WRK-22 | A schedule with no name, a name over 200 characters, a name used twice, or an interval under a second panics at boot, and a delay on a schedule's job is ignored. | `a_schedule_under_a_second_is_refused_at_boot`, `a_schedule_with_no_name_is_refused_at_boot`, `a_schedule_added_twice_is_refused_at_boot`, `a_schedule_ignores_a_delay_on_its_job` |
| WRK-23 | The `Debug` text of `NewJob` and `Schedule` leaves out a job's arguments. | `the_debug_text_leaves_out_arguments` |
| WRK-24 | A person's `cancel`, `retry` of a failed job, and `retry` of a job in review each append one act to `axa_job_acts`: `cancelled`, `retried`, or `released`, with the job's id, name, and attempts, `by`, `reason`, and the time. An act that does not apply answers `false`, changes nothing, and keeps no act. `Queue::acts` gives a job's acts, oldest first. | `each_act_is_kept_with_who_and_why_and_a_refused_one_is_not`, `a_run_that_loses_its_claim_is_stopped_and_waits_for_review` |
| WRK-25 | An act and its change to the job are one statement, in the caller's transaction, so they and the service's own audit row commit or roll back together (P-9). `cancel` and `retry` take a transaction and not a pool, and there is no form that takes a pool. | `an_act_commits_with_its_change_and_the_callers_audit` |
| WRK-26 | `by` and `reason` are codes: 1 to 200 characters with no line breaks. Anything else is refused with `Error::NotACode` before anything is written, and the table refuses it too. | `an_act_needs_a_short_code_for_who_and_why`, `a_by_or_reason_that_is_not_a_code_is_refused` |
| WRK-27 | Acts are only ever inserted. `UPDATE`, `DELETE`, and `TRUNCATE` on `axa_job_acts` are refused by the database, whoever sends them and with `session_replication_role` set to `replica`. | `the_acts_table_refuses_every_change` |
| WRK-28 | An act names its job by id, with no foreign key, so retention deletes the job's row and keeps its acts, and `Queue::acts` still reads them. | `acts_are_kept_when_their_job_is_swept` |

### 4.19 MySQL job queue

`axa-mysql-job-queue` is the MySQL twin of `axa-postgres-job-queue`, with the same API,
tables, and states, on MySQL 8.4 or later. It does not depend on `axa`, and is not yet a
feature of it. Its tests, in `tests/mysql.rs`, need `AXA_TEST_MYSQL_URL`, and have the
names of the tests in section 4.18 except where a row below names its own.

| Id | Requirement | Test |
|---|---|---|
| MYQ-1 | It meets WRK-1 to WRK-13, WRK-15 to WRK-24, WRK-26, and WRK-28, each by the tests named there. | As named in section 4.18 |
| MYQ-2 | `connect_existing` fails on a missing or out-of-date `axa_queue` or `axa_schedule`, or a missing `axa_job_acts` or one of its two triggers, and changes no schema. `connect` makes them all, one process at a time under a named lock. | `connecting_to_tables_missing_or_out_of_date_fails_and_changes_nothing`, `a_missing_acts_trigger_stops_the_queue` |
| MYQ-3 | An act locks its job, changes it, and keeps the act, all in the caller's transaction, so they and the service's own audit row commit or roll back together (P-9). There is no form that takes a pool. | `an_act_commits_with_its_change_and_the_callers_audit` |
| MYQ-4 | `UPDATE` and `DELETE` on `axa_job_acts` are refused by the database, whoever sends them. MySQL has no trigger for `TRUNCATE`, which needs the `DROP` privilege, so a service's role is given no `DROP` on it. | `the_acts_table_refuses_every_change` |
| MYQ-5 | Names are compared byte for byte, and every time is UTC on the database's clock, whatever the session's `time_zone`. The queue's own transactions run at `READ COMMITTED`, so a claim holds no gap locks. | `two_processes_run_each_job_once`, `a_delayed_job_is_not_claimed_before_its_time` |

### 4.20 SQLite job queue

`axa-sqlite-job-queue` is the SQLite twin of `axa-postgres-job-queue`, with the same API,
tables, and states, on a file, for a service on one machine. It does not depend on `axa`,
and is not yet a feature of it. Its tests, in `tests/sqlite.rs`, each make a database file
of their own, so they run in `cargo test` with no server, and have the names of the tests
in section 4.18 except where a row below names its own.

| Id | Requirement | Test |
|---|---|---|
| SQJ-1 | It meets WRK-1 to WRK-13, WRK-15 to WRK-24, WRK-26, and WRK-28, each by the tests named there. | As named in section 4.18 |
| SQJ-2 | `connect_existing` fails on a missing or out-of-date `axa_queue` or `axa_schedule`, or a missing `axa_job_acts` or one of its two triggers, and changes no schema. `connect` makes them all, in one `BEGIN IMMEDIATE` transaction. | `connecting_to_tables_missing_or_out_of_date_fails_and_changes_nothing`, `a_missing_acts_trigger_stops_the_queue` |
| SQJ-3 | An act changes its job and keeps the act in the caller's transaction, so they and the service's own audit row commit or roll back together (P-9). There is no form that takes a pool. | `an_act_commits_with_its_change_and_the_callers_audit` |
| SQJ-4 | `UPDATE` and `DELETE` on `axa_job_acts` are refused by the database, whoever sends them. | `the_acts_table_refuses_every_change` |
| SQJ-5 | A claim is one statement, and a schedule's tick and setup run in `BEGIN IMMEDIATE` transactions, so processes on one file take turns and each job and due time goes out once. | `two_processes_run_each_job_once`, `replicas_ticking_together_queue_each_due_time_once` |
| SQJ-6 | It is not published while the `libsqlite3-sys` sqlx builds has RUSTSEC-2026-0459. | The release checklist |

## 5. Verification

CI runs all of this on every push, in `.github/workflows/ci.yml`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --all-targets --features queue -- -D warnings
cargo clippy --all-targets --features sqlx -- -D warnings
cargo clippy --all-targets --features sea-orm -- -D warnings
cargo clippy --all-targets --features diesel -- -D warnings
cargo clippy --all-targets --features toasty -- -D warnings
cargo clippy -p axa-transactional-outbox --all-targets --features encryption -- -D warnings
cargo clippy -p axa-transactional-outbox --all-targets --features sea-orm -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --release -p axa-environment
cargo test --features queue
cargo test --features sqlx
cargo test --doc --features idempotency,paseto
cargo test --workspace --all-features
cargo doc --workspace --no-deps --all-features
cargo tree -p axa-diesel --all-features -e normal   # must show no C database library
AXA_TEST_DB_URL=postgres://... cargo test --features queue -- --ignored
AXA_TEST_DB_URL=postgres://... cargo test --all-features --test constraint_errors -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test --features outbox --test outbox_shutdown -- --ignored
AXA_TEST_DB_URL=postgres://... cargo test -p axa-transactional-outbox --all-features -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test -p axa-postgres-job-queue -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test -p axa-sqlx -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test -p axa-seaorm -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test -p axa-diesel -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test -p axa-toasty -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test -p axa-web-session --all-features -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test -p axa-idempotency-key -- --ignored --test-threads=1
AXA_TEST_DB_URL=postgres://... cargo test --features idempotency --test idempotency -- --ignored --test-threads=1
AXA_TEST_MYSQL_URL=mysql://... cargo test -p axa-mysql-job-queue -- --ignored --test-threads=1
AXA_TEST_REDIS_URL=redis://... cargo test -p axa-redis-cache -- --ignored
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
`axa-environment` has no dependencies, so it can go first. `axa-postgres-job-queue` goes before
`axa`, which its `queue` feature needs. `axa-problem` goes before
`axa-idempotency-key`, `axa-paseto-auth`, `axa-request-rate-limit`, and `axa-accepts-version-negotiation`, which depend on it.
`axa-test` depends on `axa`, so it goes after it.

All must pass before a commit is pushed or a version is published. The feature
sets are not interchangeable. The health, task, and queue tests compile
only without `sea-orm`, and the worker tests only without `sea-orm` and `queue`.
The rest of the suite runs under every set.

### Releasing

A version breaks when a public struct gains a field, because services build these
by hand. Publish only from a commit where everything above has passed.

## 6. Decided, not yet built

None.

## 7. Open questions

None.
