# Security policy

## Reporting a vulnerability

Report a vulnerability privately, through GitHub: open the repository's
[Security tab](https://github.com/publicprojects-io/axa/security) and choose
**Report a vulnerability**. Only the maintainers see the report, and it becomes a draft
security advisory that we can discuss and fix with you in private.

Please do not open a public issue, pull request, or discussion for a vulnerability.

A useful report says which crate and version it affects, what an attacker can do, and
how to reproduce it, ideally as a failing test or a small service.

## What happens next

We confirm the report, work out its reach, and fix it on `main` with a test. We then
publish the fixed crates, and the advisory with a CVE where one fits, and credit you
unless you ask us not to. Advisories for published crates go to the
[RustSec advisory database](https://rustsec.org/), so that `cargo audit` and
`cargo deny` warn the services that use them.

## Supported versions

Axa is before 1.0, so fixes go into the newest minor release of each crate, such as
`axa` 0.20, and are not backported to older ones. Upgrade with the guides in
[`help/`](help/).

## Scope

In scope is every crate in this repository: `axa`, `axa-test`, and the `axa-*` crates
under [`crates/`](crates/). A flaw in a dependency, such as Axum, sqlx, or rustls, goes
to that project, but tell us too if Axa uses the dependency in a way that makes it
reachable, or if a setting Axa chooses makes it worse.

Some risks are known and stated where they apply, such as the
[`libsqlite3-sys` advisory](crates/axa-sqlite-job-queue/README.md#security-advisory)
for the SQLite crates, and the advisories `deny.toml` ignores, each with its reason.
