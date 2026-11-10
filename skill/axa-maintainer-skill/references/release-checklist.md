# Release checklist

A crate cannot be unpublished, only yanked, and a new crate name is claimed for good.
Get the user's explicit go-ahead first, and name any crate being published for the
first time, because that is a scope decision rather than a mechanical step.

## 1. Decide what ships

Compare crates.io with the repository. A version in `Cargo.toml` does not prove it was
published.

```sh
git tag | tail -3
for crate in crates/*; do
  echo "$crate: $(git diff --stat "$LAST_TAG" HEAD -- "$crate" | tail -1)"
done
```

- A subcrate with code changes since its published version needs a new version. Doc
  comments alone do not.
- Update each changed subcrate's version in its own `Cargo.toml` and in the root
  `Cargo.toml`, which depends on them by version.
- A subcrate with no changes is not republished.

## 2. Prepare

- Move `[Unreleased]` in `CHANGELOG.md` under the new version, and name each crate
  covered, in the intro of that section.
- Update the upgrade guide for anything a service must act on.
- Update versions in `README.md`, `llms.txt`, `llms.json`, and the guides.
- Make sure `skill/axa-skill/SKILL.md` describes the new version.

## 3. Verify

Publish only from a commit where everything in section 5 of `spec/index.md` has passed.

1. Commit and push, and wait for CI on that exact commit: `check`, `supply chain`,
   `workflow audit`, and `secret scan` must all pass. CI is the only place the full
   Postgres and Redis suite is sure to have run.
2. Fetch, and confirm that `HEAD` equals `origin/main` and the tree is clean.
3. Dry-run every crate going out, naming each with `-p`. Cargo resolves unpublished
   workspace members in this mode:

```sh
cargo publish --dry-run -p axa-environment -p axa-postgres-sqlx -p axa -p ...
```

## 4. Publish

Subcrates publish before `axa`, because it depends on them by version. `axa-postgres-job-queue` is
one of them, as the `queue` feature needs it. `axa-problem` goes before `axa-idempotency-key`, `axa-paseto-auth`, `axa-request-rate-limit`, and `axa-accepts-version-negotiation`, which
depend on it. No other subcrate depends on another, so the rest can go in any order.
`axa-test` is the one crate that depends on `axa`, so it publishes after `axa`.

The SQLite crates are held: `axa-sqlite-job-queue`, `axa-sqlite-sqlx`, `axa-sqlite-seaorm`,
and `axa-sqlite-diesel`. Do not publish them until sqlx releases the raised cap on
`libsqlite3-sys` (sqlx PR #4502), `cargo update -p libsqlite3-sys` brings in 0.39 or
later, and the RUSTSEC-2026-0459 ignore in `.cargo/audit.toml` is gone.

crates.io refuses a crate whose dependencies, optional ones too, are not published, so
`axa` cannot be published while its `sqlite-*` features name a held crate. Until the SQLite
crates go out, a release of `axa` takes those features out of its `Cargo.toml` first, on
the release commit only, and puts them back after.

`axa-pagination` and `axa-offset-pagination` are retired, as axa 0.9 removed offset
paging. Never publish either again.

**crates.io limits new crates.** After a small burst, each new name waits about ten
minutes, and the 429 names the time to retry. Existing crates are not held back. A
partial publish is a normal state to recover from:

- Publish the existing crates first where you can, then the new ones.
- Run a loop that retries on a 429 and stops on any other error, with `axa` last.
- A new crate that depends on `axa`, such as `axa-test` was in 0.10.0, must not wait
  behind it. Hold `axa` until the 429's retry time has passed, then publish `axa` and
  that crate back to back. In 0.10.0, `axa` went out nine minutes before `axa-test`,
  and a service following the upgrade path in the CHANGELOG found no `axa-test`.
- Tell other sessions only once every crate is in the index.
- Do not treat the shell's exit code as the result. Read cargo's output, and check the
  crates.io index for each crate.

## 5. Finish

- Tag the published commit with an annotated tag, and push it:

```sh
git tag -a vX.Y.Z -m "axa X.Y.Z" <commit>
git push origin vX.Y.Z
```

- Build a fresh project against the published crates, with `axa = "=X.Y.Z"` and the
  features you changed, and run `cargo check`.
- Start a new `## [Unreleased]` section in `CHANGELOG.md`.
- Tell any other session working here that main is free, with the commit and the crates.
