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
```

### Running Database Tests

Database tests require a running PostgreSQL, MySQL, and Redis instance.
Use the provided Podman Compose configuration:

```sh
# Start the containers
podman-compose -f podman-compose.yml up -d

# Run database tests
AXA_TEST_DB_URL=postgres://axa:axa@localhost:5432/axa_test \
  AXA_TEST_MYSQL_URL=mysql://axa:axa@localhost:3307/axa_test \
  AXA_TEST_REDIS_URL=redis://localhost:6379 \
  cargo test --all-features -- --ignored
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
  cargo-tarpaulin` first, set by [`tarpaulin.toml`](tarpaulin.toml), into
  `target/coverage/` as HTML and LCOV. It covers the tests that need no server; add
  `-- --ignored` with the database URLs set to include the rest.
- **Benchmarks**: `make bench` runs [`benchmarks/`](benchmarks/), with criterion: the
  standard middleware's cost over bare Axum; checking a cursor, signing and verifying a
  webhook with HMAC and Ed25519, and escaping `LIKE` text; and a ping, insert, read by
  key, and page of 50 on
  SQLite, and on Postgres and MySQL when `AXA_BENCH_POSTGRES_URL` and
  `AXA_BENCH_MYSQL_URL` name a database for them. Each makes and drops the table
  `axa_bench_items`.
- **Mutation testing**: `make mutants CRATE=axa-webhooks` runs `cargo mutants` on one
  crate with the lean `mutants` profile. A missed mutant is a change no test notices.

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
