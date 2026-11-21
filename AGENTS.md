# AGENTS.md

Instructions for any AI coding agent working in this repository. See
[`CLAUDE.md`](CLAUDE.md) for Claude Code-specific supplements.

## What this crate is

`axa` is an accessories library for Axum websites, databases, microservices, and more.
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
`rust-toolchain.toml` pins stable, N, for local work. `.github/workflows/ci.yml` sets
`RUSTUP_TOOLCHAIN` to `rust-version`, N-2, over that pin, and
`make ci-minimum-supported-rust-version` checks N-2 and N-1, so change all three together
when stable moves.

## Conventions

- Do not add lifecycle hooks, route macros, a dependency-injection container,
  or a database abstraction.
- Write behaviour afresh rather than copying Loco source: Loco is Apache-2.0
  only, and this crate offers seven licences.
- Add code only once two services need it, or one service needs it to keep a
  control that a hazard log, a regulator, or a security review holds it to.
- Raise any new dependency with the user first.
- Update `llms.txt` and `llms.json` when a feature, crate, or module changes.
- An integration test is a module of `tests/main.rs`, so a crate's tests link once. One
  that needs a process of its own, because it starts its binary again as a child or
  because `tracing` caches per process whether a log site is on, is a `[[test]]` of its
  own in `Cargo.toml`, which sets `autotests = false`. CI picks a module with a filter:
  `cargo test --test main -- --ignored stale_plan::`.
- Property tests are `proptest`, in a `#[cfg(test)] mod properties` beside a crate's
  unit tests. [`fuzz/`](fuzz/) and [`benchmarks/`](benchmarks/) are standalone packages,
  outside the workspace, so their dependencies stay out of the published lockfile: see
  CONTRIBUTING.md for running them.

## Skills

- [`skill/axa-maintainer-skill`](skill/axa-maintainer-skill/SKILL.md): working on this
  codebase, including the traps and the release checklist.
- [`skill/axa-skill`](skill/axa-skill/SKILL.md): using Axa in a service. Update it when
  anything a service sees changes.
