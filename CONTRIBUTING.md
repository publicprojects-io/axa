# Contributing to axa

Thank you for your interest in contributing to axa! This document outlines how to
set up your development environment, run tests, and submit changes.

---

## Getting Started

### Prerequisites

- Rust 1.98 or newer (axa's `rust-version` in `Cargo.toml`)
- Podman (for local development with containers)
- Git

### Setup

1. **Clone the repository:**
   ```sh
   git clone https://github.com/publicprojects-io/axa.git
   cd axa
   ```

2. **Install Rust:** `rust-toolchain.toml` pins the current stable, 1.100.0, with
   clippy, llvm-tools, and rustfmt, and rustup installs it on the first `cargo` in the
   repository. To check the minimum supported version, 1.98, and 1.99 as well:
   ```sh
   make ci-minimum-supported-rust-version
   ```

3. **Install dependencies:**
   ```sh
   # Install cargo tools
   cargo install cargo-audit cargo-deny
   
   # Install pre-commit hooks
   pip install pre-commit
   pre-commit install
   ```

---

## Development Workflow

### Building

```sh
# Build the entire workspace
cargo build --workspace

# Build with all features
cargo build --workspace --all-features
```

### Running Tests

```sh
# Run all tests (unit tests and doc tests)
cargo test --workspace --all-features

# Run only unit tests
cargo test --workspace

# Run with a specific feature
cargo test --features postgres-sqlx

# The tests that need no server, faster, with cargo-nextest (`cargo install cargo-nextest`):
# each test runs in its own process, across every test binary at once. It runs no
# doctests, and not `cli_exit`, so `make test-fast` runs those with `cargo test`.
make test-fast
```

### Changing a published crate's public API

CI's `semver` job compares each published crate that a push or pull request changed,
and `axa` beside it, with its last release on crates.io. `make semver` checks every
crate, and `bin/semver BASE` the ones changed since a commit; both need
`cargo install cargo-semver-checks`. Where a crate's
version is still the published one and its API changed in a way that needs a new
version, the job fails: bump the crate's `version` in its own `Cargo.toml` and in the
root `Cargo.toml`, which names it, and say what changed in the CHANGELOG. Before 1.0 a
breaking change is a new minor version, and anything else is a patch.

The check cannot see everything. A changed return type, such as a function that now
returns a `Result`, passes it, so those changes need a version bump by hand and a line
in the upgrade guide. A crate that is already past its release is not checked, as its
bump is the signal. A new crate, or one that is held back, has no release to compare
with, so a held one is named in `bin/semver`'s `unpublished` list until it is published.

### Running Database Tests

Database tests require a running PostgreSQL, MySQL, and Redis instance, and the S3
tests an S3-compatible endpoint with a bucket.
Use the provided Podman Compose configuration:

```sh
# Start the containers
podman-compose -f podman-compose.yml up -d

# Run database tests
AXA_TEST_DB_URL=postgres://axa:axa@localhost:5432/axa_test \
  AXA_TEST_MYSQL_URL=mysql://axa:axa@localhost:3307/axa_test \
  AXA_TEST_REDIS_URL=redis://localhost:6379 \
  cargo test --all-features -- --ignored

# Make the S3 test bucket once, then run the S3 tests
curl --aws-sigv4 "aws:amz:us-east-1:s3" --user axatest:axatestsecret \
  -X PUT http://localhost:9000/axa-test
AXA_TEST_S3_URL=http://axatest:axatestsecret@localhost:9000/axa-test \
  cargo test -p axa-storage-with-apache-arrow-and-aws --test storage -- --ignored
```

### Property Tests, Fuzzing, Benchmarks, and Mutation Testing

- **Property tests** are `proptest` tests in a `properties` module beside the unit
  tests, in `axa-cursor-pagination`, `axa-webhooks`, `axa-postgres-sqlx`, and
  `axa-mysql-sqlx`. They run with `cargo test`, as any other test.
- **Fuzzing**: `make fuzz` runs each target in [`fuzz/`](fuzz/) for 60 seconds, on
  nightly, with `cargo install cargo-fuzz` first. `FUZZ_SECONDS=600 make fuzz` runs
  longer. Each target reads untrusted input: `X-Forwarded-For`, cursor queries, upload
  file names, `If-Match`, and webhook headers. A crash leaves its input in
  `fuzz/artifacts/`; turn it into a test before fixing it.
- **Coverage**: `make coverage` runs `cargo tarpaulin`, with `cargo install
  cargo-tarpaulin` first, set by [`tarpaulin.toml`](tarpaulin.toml), which names the
  reports and the directory they go to. Its two sections, every feature and then `axa`
  with no database, are merged into one report. It covers the tests that need no
  server; add `-- --include-ignored --test-threads=1` with the database and S3 URLs set
  to include the rest, as CI runs them. `bin/test-coverage` does the same and commits the
  report in [`docs/coverage/`](docs/coverage/). CI's coverage job runs the tests that
  need no server and fails when a section of `tarpaulin.toml` measures under 52% of lines.
  No one run shows the real figure, as the server tests, the no-database section, and
  the rest each cover what the others cannot. Run each into its own `--output-dir`, and
  give every `lcov.info` to `bin/coverage-union`, which counts a line covered when any
  run covered it and leaves out lines inside `tracing::` macro calls, which read as
  uncovered in a process where no subscriber was on. `--files N` lists the worst files.
- **Benchmarks**: `make bench` runs [`benchmarks/`](benchmarks/), with criterion: the
  standard middleware's cost over bare Axum; checking a cursor, signing and verifying a
  webhook with HMAC and Ed25519, and escaping `LIKE` text; and a ping, insert, read by
  key, and page of 50 on
  SQLite, and on Postgres and MySQL when `AXA_BENCH_POSTGRES_URL` and
  `AXA_BENCH_MYSQL_URL` name a database for them. Each makes and drops the table
  `axa_bench_items`. `bin/test-benchmarks` runs the same package with `cargo criterion` and
  writes the report in [`docs/benchmarks/`](docs/benchmarks/), with the revision, Rust
  version, host, and databases that produced it, then commits it (`--no-commit` skips
  that). It replaces the report only when the middleware, primitives, and SQLite
  benchmarks all measured, so a failed or empty run exits nonzero and keeps the last one.
- **Mutation testing**: `make mutants CRATE=axa-webhooks` runs `cargo mutants` on one
  crate with the lean `mutants` profile. A missed mutant is a change no test notices.
  `make mutants-diff`, or `bin/mutants-diff BASE`, mutates only the lines changed since
  `BASE`, `origin/main` by default, uncommitted ones included, and CI runs it on every
  pull request. It fails when the diff changes Rust but matches no mutant, which a diff
  with the wrong path root would do while seeming to pass.

CI builds and lints `fuzz/` and `benchmarks/` on every push, so neither rots, but does
not run them: fuzzing needs nightly, and the database benchmarks need servers.

### Code Quality Checks

```sh
# Format check
cargo fmt --all --check

# Lint check
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Documentation
cargo doc --workspace --no-deps --all-features
```

### Pre-commit Hooks

The repository includes pre-commit hooks that run:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo doc --workspace --no-deps --all-features`
- `cargo test --workspace --all-features --lib`

To install:
```sh
pre-commit install
```

---

## Pull Request Process

1. **Fork the repository** and create a feature branch from `main`.

2. **Follow the spec-first discipline:**
   - Every behavior change requires three parts:
     - A spec edit in `spec/index.md`
     - A code edit
     - A test edit
   - Each spec requirement should name its test.

3. **Run all checks before pushing:**
   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-features
   cargo doc --workspace --no-deps --all-features
   ```

4. **Update documentation:**
   - Update `llms.txt` and `llms.json` if adding new documentation or crates
   - Update `CHANGELOG.md` for breaking changes

5. **Submit your PR** with a clear description of the changes.

---

## Code Review Principles

- **Library, not framework:** Keep axa as a library. No lifecycle hooks, route
  macros, or code generation.
- **Fail closed:** Production refuses unsafe configurations by default.
- **Audit-friendly:** Every request path must be traceable from `main.rs`.
- **Spec-first:** Code conforms to the specification, not the other way around.

---

## Reporting Issues

Please include:
- The version of axa you're using
- The Rust version
- Steps to reproduce the issue
- Expected vs actual behavior

---

## Architecture

For a detailed understanding of axa's architecture, see:
- [docs/architecture/](docs/architecture/index.md) - Design principles and structure
- [docs/architecture-diagram/](docs/architecture-diagram/index.md) - Visual diagrams
- [spec/index.md](spec/index.md) - Single source of truth for behavior

---

## See Also

- [README.md](README.md) - Project overview
- [AGENTS.md](AGENTS.md) - Build, test, lint conventions
- [CHANGELOG.md](CHANGELOG.md) - Release notes
- [plan.md](plan.md) - Roadmap and task list
- [tasks.md](tasks.md) - Detailed task breakdown
