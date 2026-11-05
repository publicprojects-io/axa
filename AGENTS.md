# AGENTS.md

Instructions for any AI coding agent working in this repository. See
[`CLAUDE.md`](CLAUDE.md) for Claude Code-specific supplements.

## What this crate is

`axa` is a small shared library for services built directly on Axum and sqlx.
It is a library, not a framework: a service's `main.rs` stays ordinary Axum
and calls into this crate.

[`spec/index.md`](spec/index.md) is the single source of truth. Code conforms
to the spec, and a change in behaviour carries a spec edit, a code edit, and a
test edit together.

## Build, test, lint

```sh
cargo test
cargo clippy --all-targets --all-features
cargo doc --no-deps
```

All three must be clean. `[lints]` in `Cargo.toml` denies pedantic clippy and
missing docs, and forbids unsafe code.

## Conventions

- Do not add lifecycle hooks, route macros, a dependency-injection container,
  or a database abstraction.
- Write behaviour afresh rather than copying Loco source: Loco is Apache-2.0
  only, and this crate offers five licences.
- Add code only once two services need it.
- Raise any new dependency with the user first.
