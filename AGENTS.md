# AGENTS.md

Instructions for any AI coding agent working in this repository. See
[`CLAUDE.md`](CLAUDE.md) for Claude Code-specific supplements.

## What this crate is

`axa` is a small shared library for services built directly on Axum.
It is a library, not a framework: a service's `main.rs` stays ordinary Axum
and calls into this crate.

[`spec/index.md`](spec/index.md) is the single source of truth. Code conforms
to the spec, and a change in behaviour carries a spec edit, a code edit, and a
test edit together.

## Build, test, lint

```sh
cargo fmt --all --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo doc --workspace --no-deps --all-features
```

All four must be clean. They are the short form: the feature sets are not
interchangeable, and [section 5 of the spec](spec/index.md#5-verification) has the
full list that CI runs, including the Postgres and Redis tests. `[lints]` in
`Cargo.toml` denies pedantic clippy and missing docs, and forbids unsafe code.

`rust-version` is two releases behind stable, and is the same in every manifest.
`.github/workflows/ci.yml` pins that version too, so change them together.

## Conventions

- Do not add lifecycle hooks, route macros, a dependency-injection container,
  or a database abstraction.
- Write behaviour afresh rather than copying Loco source: Loco is Apache-2.0
  only, and this crate offers five licences.
- Add code only once two services need it, or one service needs it to keep a
  control that a hazard log, a regulator, or a security review holds it to.
- Raise any new dependency with the user first.
- Update `llms.txt` and `llms.json` when a feature, crate, or module changes.

## Skills

- [`skill/axa-maintainer-skill`](skill/axa-maintainer-skill/SKILL.md): working on this
  codebase, including the traps and the release checklist.
- [`skill/axa-skill`](skill/axa-skill/SKILL.md): using axa in a service. Update it when
  anything a service sees changes.
