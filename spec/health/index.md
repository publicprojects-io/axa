# Health routes

`axa` serves one route for each probe Kubernetes gives a container, so that whoever runs
the service sees each route's purpose at once, and points each probe at the route made
for it. This page is part of the [specification](../index.md), and the source of truth
for these routes.

| Route | Probe | It decides | A failure means | It asks |
|---|---|---|---|---|
| `/_health/live` | Liveness | Whether to kill and restart the container | Restart it | Nothing |
| `/_health/ready` | Readiness | Whether the pod gets incoming traffic | Take the pod out of the Service | What the service owns: its database, its queue, and its named checks |
| `/_health/startup` | Startup | Whether the service has booted, holding off the other two until it has | Not booted yet | That the code has started and the primary database answers |

## Liveness: `/_health/live`

It answers `200` as soon as the server's loop is running, and asks nothing, not even
the database. It never checks a third party. If a payment provider goes down,
restarting the container does not bring it back: it only takes the service down too.

## Readiness: `/_health/ready`

It checks the things the service owns: the database and queue `axa` holds, and the
named checks in `AppContext::health_checks`, such as its Redis or search index. It
never blocks on a third-party API, and no named check should either. A pod that is not
ready is pulled from the Service, so every visitor gets a `503`. A pod that stays ready
while a third party is down can still serve cached pages, show a "payments are offline"
banner, or queue the work for later.

From the moment the service is told to stop, readiness answers `503` with the `code`
`shutting_down`, whatever its checks say. Kubernetes takes a few seconds after `SIGTERM`
to take a pod out of the Service, so `SHUTDOWN_DRAIN_SECONDS` keeps the server accepting
for that long while readiness refuses, and only then stops listening and lets requests
finish. Set it to a little more than the time the cluster takes, with
`terminationGracePeriodSeconds` above it plus `SHUTDOWN_TIMEOUT_SECONDS`.

## Startup: `/_health/startup`

It checks only that the service's code has started and its primary database answers,
and never a third party. It holds off liveness and readiness while a slow service
boots. Once it has passed it passes for good, as Kubernetes asks it no more after that.

## Requirements

| Id | Requirement | Test |
|---|---|---|
| HLT-1 | `/_health/live`, `/_health/ready`, and `/_health/startup` answer `200` and `{"ok":true}` when healthy. No other health path is served. | `every_health_route_answers_ok` |
| HLT-2 | `/_health/ready` answers `503` when the database or queue does not answer, naming it, while `/_health/live` still passes. | `readiness_fails_when_the_database_does_not_answer`, `readiness_names_the_database`, `readiness_names_a_queue_that_does_not_answer` |
| HLT-3 | A service can register named async checks in `AppContext::health_checks`, which only readiness asks. All passing gives `{"ok":true}`. Any failing gives a `503` problem with the `code` `not_ready`, `ok` of `false`, and `failed` naming the checks in the order added. | `readiness_with_every_check_passing_is_just_ok`, `readiness_names_what_failed_in_the_order_added`, `only_a_failing_answer_is_a_problem` |
| HLT-4 | A check's error text is logged and never sent. | `readiness_never_sends_a_checks_error_text` |
| HLT-5 | A check past its time limit, or one that panics, has failed. | `a_check_past_its_time_limit_fails`, `a_check_that_panics_fails` |
| HLT-6 | One answer, passing or failing, is reused for a few seconds, and only one round runs at a time, so a public route cannot drive load at a dependency. A caller that hangs up does not cancel the round. | `one_answer_is_reused_until_it_goes_stale`, `a_failed_answer_is_reused_too`, `callers_during_a_round_share_it`, `a_caller_that_hangs_up_does_not_waste_the_round` |
| HLT-7 | `/_health/ready` asks `AppContext::db` as `database`, for each database feature (DB-3). A pool held anywhere else is a named check. | `the_context_pool_is_checked_without_being_named`, `a_sqlx_pool_is_checked_by_name` |
| HLT-8 | `/_health/live` asks nothing, and a failing named check holds back only readiness, never liveness or startup. | `liveness_asks_nothing`, `a_failing_named_check_holds_back_only_readiness` |
| HLT-9 | `/_health/startup` asks only the database `axa` holds. It answers `503` with the `code` `not_started` until the database answers, and once it has passed, it passes for good. | `readiness_fails_when_the_database_does_not_answer`, `startup_once_passed_stays_passed` |
| HLT-10 | The three paths stay open without a token in `axa-paseto-auth` and `axa-keycloak-auth`, and uncounted in `axa-request-rate-limit`, and only those three: nothing else under `/_health/` is let through. | `health_routes_are_public_by_default` in both auth crates, `health_stays_public_and_everything_else_needs_a_token`, `the_probes_are_never_refused_or_counted` |
| HLT-11 | From the moment shutdown begins, `/_health/ready` answers `503` with the `code` `shutting_down` and asks nothing, while `/_health/live` still passes. | `readiness_refuses_once_shutdown_begins_and_liveness_does_not` |
