---
name: axa-maintainer-skill
description: Maintain the axa codebase itself. Use this whenever working directly in the publicprojects-io/axa repository, including changing `src/` or any `crates/axa-*` subcrate, editing `spec/index.md`, adding a subcrate or feature, updating dependencies or Dependabot PRs, changing CI, verifying a change, or preparing and publishing a release, even if the user only says "fix this in axa" or "release axa". Not for a service that merely depends on axa; use axa-skill for that.
---

# Maintaining axa

axa is a small shared library for services built directly on Axum, written to be
audited: the SQL, the middleware, and the startup path are all visible, with no
framework between the reader and the behaviour. Judge a change by whether an auditor
can still read the whole path.

This skill is the working layer: how to make a change here without breaking the things
that are easy to break. It does not restate the rules, which live in one place each.

## Read first

1. [`AGENTS.md`](../../AGENTS.md): build commands and conventions.
2. [`spec/index.md`](../../spec/index.md): the single source of truth. Read sections 2
   and 3 before adding anything, and section 5 before claiming a change is verified.
3. The `[Unreleased]` section of [`CHANGELOG.md`](../../CHANGELOG.md): what this cycle
   has already changed.

## Other sessions share this repository

Several sessions often work here at once, and the main checkout can be on someone
else's branch.

- Run `git status -sb` before assuming which branch a checkout is on.
- Work in your own worktree off `origin/main`, and fetch before starting and again
  before pushing.
- Check `git worktree list` and the branches for work that overlaps yours. If another
  session has the same change in progress, coordinate rather than duplicate it.

## Making a change

A change in behaviour is one change carrying three parts: the spec edit, the code edit,
and the test edit. Each spec requirement has an id and names the test that proves it.

1. Find the requirement in `spec/index.md`, or add a row with the next free id.
2. Write a test that fails without the change. For a security fix, watch it fail
   against the old code, because a test that was never red proves little.
3. Change the code.
4. Run the commands in section 5 of the spec. See the traps below.
5. Update what the change touches:

| If you changed | Also update |
|---|---|
| Behaviour | `spec/index.md`, and `CHANGELOG.md` under `[Unreleased]` |
| A setting | The table in `README.md`, and `Config::settings` so `check` prints it |
| A feature, subcrate, or module | `README.md`, the table in `src/lib.rs`, `Cargo.toml`, `llms.txt`, `llms.json` |
| Anything a service must act on | The upgrade guide in `help/upgrading-to-0.3/index.md` |
| Anything a service sees | `skill/axa-skill/SKILL.md` |
| How this repository works | `AGENTS.md` and this skill |

## Before adding code

These come from the principles in section 3 of the spec, and each is a reason to say no:

- A hook, route macro, code generator, dependency-injection container, plugin system,
  configuration language, or database abstraction does not belong here. Section 2 lists
  these, because each puts indirection between a reader and the behaviour.
- Code goes in once two services need it. Otherwise the README marks it as having no
  service yet.
- A new dependency is raised with the user first, even one already in the tree as a
  transitive dependency, because it is a cost every consumer pays.
- Never copy Loco source. Loco is Apache-2.0 only and this crate offers five licences,
  so write the behaviour afresh.
- Fail closed. A missing or unsafe production setting stops boot, and internal failure
  text is logged and never sent to a client.

## Traps

**The feature sets are not interchangeable.** Some tests compile only without `sea-orm`,
and the worker tests only without `queue` as well. A single `--all-features` run is
not verification, so run all three test sets from section 5.

**Clippy differs by version.** CI runs it on 1.97 and on stable, and a lint can pass on
one and fail on the other.

**`#[ignore]` tests need real services.** `AXA_TEST_DATABASE_URL` and
`AXA_TEST_REDIS_URL` turn them on, and a plain `cargo test` skips them silently. Start
a throwaway container on a random loopback port, or rely on CI, and say which you did.

**Idempotency tests share one table.** Each starts by truncating `axa_idempotency`,
so these suites run with `--test-threads=1`, and in parallel they fail intermittently.

**Queue tests share one table.** Every test that touches `axa_queue` must hold the
`TABLE` lock in `src/queue.rs`, or tests that race on it fail intermittently. A test
that drops a column must put it back.

**Outbox tests share one table too.** They hold the `TABLE` lock in
`crates/axa-outbox/tests/postgres.rs`, and CI runs them with `--test-threads=1`, which
also keeps `logging.rs` off that table. They run only with `-p axa-outbox --all-features
-- --ignored`, and a plain `--features queue,testing` run does not reach them. The
claim's `FOR UPDATE ... SKIP LOCKED` is the only thing stopping two relays taking one row,
as the outer `UPDATE` does not check `status` again: `several_relays_publish_each_event_once`
is the test that says so.

**Logging tests get their own binary.** `tracing` caches per process whether a log site
is on, so a test that captures logs lives in its own file under `tests/`.

**Production fixtures must pass production's rules.** A test using
`Environment::Production` needs a `DATABASE_URL` that stays private, such as
`postgres://localhost/x`.

**README examples are compiled.** `src/lib.rs` includes `README.md` as doctests when
`sea-orm` is off, so an edit there can fail `cargo test`.

**A public struct gaining a field is a breaking change**, because services build these
by hand. Note it in the upgrade guide.

**A `304` must not say its size.** Axum writes `content-length: 0` on any answer whose
body says it is empty, and on a `304` that header can only state the `200`'s length. So
`conditional` uses a body with no size, and `Body::empty()` there is a bug no status check
would catch.

**`conditional::not_modified` stays outside compression.** `tower-http` adds
`vary: accept-encoding` only to what it could compress, which a `304` is not, so inside it
a `304` loses the `vary` its `200` had.

**`axum::serve` is not used.** `server.rs` runs its own connection loop, because
`axum::serve` cannot limit a connection. A change to serving belongs there, and an axum
upgrade should be checked against axum's own loop. `hyper`'s header timer stays off:
it runs across a request's headers and the wait between requests, which need different
limits, so `Tracker` does both. `hyper_util`'s `auto::Builder` must not come back,
because its version detection has no limit and `http1_only` does nothing on its upgrade
path.

**`axa-keycloak` stands alone.** It depends on neither `authentication-verifier` nor any
other axa crate, and states the source of each copy. A fix to the original does not
reach a copy, so check both ways, including `axa-paseto`, which shares its shape.

## Dependencies

- `rust-version` is two releases behind stable, the same in all 17 manifests, and
  matched in `.github/workflows/ci.yml`. Change them together.
- `jsonwebtoken` is a dev-dependency held at the major version `axum-keycloak-auth`
  uses, so that the tests mint with the library that verifies. Dependabot ignores its
  major versions. Lift the hold when the engine moves.
- Actions are pinned to a commit with the version in a comment, and images to a digest.
- The repository's Dependabot cooldown is 7 days, so take the newest release at least
  that old when bumping by hand.
- Before closing a Dependabot PR as superseded, confirm that the change is on `main` and
  CI passed there. Dependabot may open a new PR for the next patch at any time.
- `cargo audit` and `cargo deny --all-features check advisories bans licenses sources`
  must both pass.

## Releasing

A release cannot be undone, only yanked. Follow
[`references/release-checklist.md`](references/release-checklist.md) in full.
