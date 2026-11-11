# Justfile for axa development
# Install: https://just.systems/managing-projects/installation/

# Default recipe - runs all checks
default:
    just check

# Run all checks
check:
    just fmt
    just lint
    just test
    just doc

# Format code
fmt:
    cargo fmt --all --check

# Run linter
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run tests
test:
    cargo test --workspace

# Run tests with all features
test-all:
    cargo test --workspace --all-features

# Run database tests (requires podman-compose)
test-db:
    podman-compose -f podman-compose.yml up -d
    AXA_TEST_DATABASE_URL=postgres://axa:axa@localhost:5432/axa_test \
      AXA_TEST_MYSQL_URL=mysql://axa:axa@localhost:3307/axa_test \
      AXA_TEST_REDIS_URL=redis://localhost:6379 \
      cargo test --all-features -- --ignored

# Generate documentation
doc:
    cargo doc --workspace --no-deps --all-features

# Build the project
build:
    cargo build --workspace

# Build with all features
build-all:
    cargo build --workspace --all-features

# Clean build artifacts
clean:
    cargo clean

# Run a specific test
# Usage: just test-one package::test_name
test-one TEST:
    cargo test --workspace {{TEST}}

# Start development containers
up:
    podman-compose -f podman-compose.yml up -d

# Stop development containers
down:
    podman-compose -f podman-compose.yml down

# View container logs
logs:
    podman-compose -f podman-compose.yml logs -f

# Run the example (if exists)
run:
    cargo run

# Audit dependencies
# Requires: cargo install cargo-audit
audit:
    cargo audit

# Check for supply chain issues
# Requires: cargo install cargo-deny
supply-chain:
    cargo deny --all-features check advisories bans licenses sources

# Update dependencies
update:
    cargo update

# Show version information
version:
    echo "Rust version: $(rustc --version)"
    echo "Cargo version: $(cargo --version)"
    echo "Axa version: $(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)"

# Run pre-commit hooks manually
pre-commit:
    pre-commit run --all-files

# Install pre-commit hooks
install-hooks:
    pre-commit install

# Check for outdated dependencies
outdated:
    cargo outdated --workspace

# Run a specific clippy check
clippy-one PACKAGE:
    cargo clippy -p {{PACKAGE}} --all-targets --all-features -- -D warnings

# Build and run in release mode
run-release:
    cargo build --release && ./target/release/example_service
