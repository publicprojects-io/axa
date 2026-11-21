# Upgrading to Axa 0.21

This covers a service on 0.20. A service that sets none of the old setting names below
upgrades by changing the versions.

## What will not compile

| Change | What to do |
|---|---|
| `axa-test` 0.12 is the one for `axa` 0.21 | Set `axa-test = "0.12"` in `[dev-dependencies]` |

## What changes with no compile error

| Change | What to do |
|---|---|
| A setting `axa` renamed is refused in production | `DATABASE_URL`, `BODY_LIMIT_BYTES`, `HEADER_READ_TIMEOUT_SECONDS`, `HTTP_CSP`, and `LOCO_ENV` were ignored, so a service that still set one ran on a default. Production now fails to boot, naming the name to set: `DB_URL`, `HTTP_REQUEST_BODY_LIMIT_BYTE_COUNT`, `HTTP_REQUEST_HEADER_READ_TIMEOUT_SECONDS`, `HTTP_CONTENT_SECURITY_POLICY`, and `APP_ENV`. Run `service check` with the deploy config before upgrading: elsewhere each is a warning it prints |
| `Cache::connect` waits for no answer from Redis | A Redis that is down at boot no longer stops the service. A bad URL is still refused. A service that relied on `connect` to fail when Redis was down asks `Cache::ping` instead, as readiness does |
| `axa` installs ring as the rustls crypto provider | `db::connect` and `AppContext::new` install it first, and do nothing if the service installed one. A service that calls its own `install_tls_provider()` may keep it or call `axa::tls::install_provider()` |

## What a service may delete

| Its own | Axa's in its place |
|---|---|
| A helper that installs the rustls provider before a test or a client | `axa::tls::install_provider()`, which `axa-test` already calls before every boot |
| A table of retired setting names checked at boot | `Settings::renamed(old, Some(new))`, beside `axa::config::RENAMED` for Axa's own |
| Tests that set process environment variables, run one at a time | `TestApp::boot_test_with` and `axa_test::request_with(&[("NAME", "value")], ...)` |
| A helper that runs a test again in a process of its own | `axa_test::own_process!()` |
| A mail error's own transient-or-permanent split, and an address check | `axa_mail::Error::is_transient` and `axa_mail::address_is_safe` |
| A field-error collector for `422` problems | `axa::error::Problems` |
| A check that integers fit a JSON client | `axa::extract::SafeInt` |
| A file watch that reloads the ABAC policy | `ReloadablePolicy::watch_file` |
| `cross_site_paths` entries built at boot for each path under a prefix | One entry ending `/*`, such as `/forms/*` |
