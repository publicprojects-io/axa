# Contributing to axa

Thank you for your interest in contributing to axa! This document outlines how to
set up your development environment, run tests, and submit changes.

---

## Getting Started

### Prerequisites

- Rust 1.97 or newer (axa's `rust-version` in `Cargo.toml`)
- Podman (for local development with containers)
- Git

### Setup

1. **Clone the repository:**
   ```sh
   git clone https://github.com/publicprojects-io/axa.git
   cd axa
   ```

2. **Install Rust:**
   ```sh
   rustup install 1.97
   rustup default 1.97
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
AXA_TEST_DATABASE_URL=postgres://axa:axa@localhost:5432/axa_test \
  AXA_TEST_MYSQL_URL=mysql://axa:axa@localhost:3307/axa_test \
  AXA_TEST_REDIS_URL=redis://localhost:6379 \
  cargo test --all-features -- --ignored
```

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
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo doc --workspace --no-deps --all-features`
- `cargo test --workspace`

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
