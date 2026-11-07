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
cargo publish --dry-run -p axa-environment -p axa-sqlx -p axa -p ...
```

## 4. Publish

Subcrates publish before `axa`, because it depends on them by version. `axa-worker` is
one of them, as the `queue` feature needs it. `axa-problem` goes before `axa-idempotency`, `axa-paseto`, `axa-ratelimit`, and `axa-versioning`, which
depend on it. No other subcrate depends on another, so the rest can go in any order.

**crates.io limits new crates.** After a small burst, each new name waits about ten
minutes, and the 429 names the time to retry. Existing crates are not held back. A
partial publish is a normal state to recover from:

- Publish the existing crates first where you can, then the new ones.
- Run a loop that retries on a 429 and stops on any other error, with `axa` last.
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
