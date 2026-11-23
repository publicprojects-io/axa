# Makefile for axa development
# Provides common commands for building, testing, and development

.PHONY: semver test-fast all build test lint doc clean fmt check ci-minimum-supported-rust-version fuzz bench coverage mutants mutants-diff

# Default target
all: check

# Build targets
build:
	cargo build --workspace

build-all:
	cargo build --workspace --all-features

build-release:
	cargo build --workspace --release

# Test targets
test:
	cargo test --workspace

test-all:
	cargo test --workspace --all-features

# The same tests that need no server, faster: cargo-nextest runs each test in its own
# process, across every test binary at once. It runs no doctests, and not `cli_exit`,
# which has a custom harness, so those two run with `cargo test`.
test-fast:
	cargo nextest run --workspace --all-features
	cargo test --workspace --all-features --doc
	cargo test --test cli_exit --all-features

# Run ignored tests (requires running containers)
test-ignored:
	AXA_TEST_DB_URL=postgres://axa:axa@localhost:5432/axa_test \
	AXA_TEST_MYSQL_URL=mysql://axa:axa@localhost:3307/axa_test \
	AXA_TEST_REDIS_URL=redis://localhost:6379 \
	cargo test --all-features -- --ignored

# The public API of every published crate against its last release on crates.io
# (cargo install cargo-semver-checks). CI checks only the crates a change touches: run
# `bin/semver BASE` for the same. It cannot see a changed return type.
semver:
	bin/semver

# Lint and format
lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all --check

# Documentation
doc:
	cargo doc --workspace --no-deps --all-features

doc-open:
	cargo doc --workspace --no-deps --all-features --open

# Clean targets
clean:
	cargo clean

clean-all:
	cargo clean
	rm -rf target

# Check all
check: fmt lint test doc

# Container management (requires podman)
up:
	podman-compose -f podman-compose.yml up -d

down:
	podman-compose -f podman-compose.yml down

logs:
	podman-compose -f podman-compose.yml logs -f

ps:
	podman-compose -f podman-compose.yml ps

# Security checks
audit:
	cargo audit

supply-chain:
	cargo deny --all-features check advisories bans licenses sources

# Version info
version:
	@echo "Rust: $(shell rustc --version)"
	@echo "Cargo: $(shell cargo --version)"
	@echo "Axa: $(shell grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)"

# CI targets (run locally what CI runs)
ci-fmt:
	cargo fmt --all --check

ci-clippy:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

ci-test:
	cargo test --workspace --all-features

ci-doc:
	cargo doc --workspace --no-deps --all-features
	cargo doc --workspace --no-deps --all-features --document-private-items

ci: ci-fmt ci-clippy ci-test ci-doc

# Fuzz each target for FUZZ_SECONDS (60 by default), on nightly, with cargo-fuzz.
FUZZ_SECONDS ?= 60
fuzz:
	cd fuzz && for target in $$(cargo +nightly fuzz list); do \
		cargo +nightly fuzz run "$$target" -- -max_total_time=$(FUZZ_SECONDS) || exit 1; \
	done

# Benchmarks: the middleware's cost, and each database. Set AXA_BENCH_POSTGRES_URL and
# AXA_BENCH_MYSQL_URL to include Postgres and MySQL; SQLite always runs.
bench:
	cd benchmarks && cargo bench

# Test coverage, with cargo-tarpaulin, into the output-dir that tarpaulin.toml names.
coverage:
	cargo tarpaulin

# Mutation testing of one crate, with cargo-mutants: make mutants CRATE=axa-webhooks
mutants:
	cargo mutants -p $(CRATE) --test-package $(CRATE) --profile mutants

# Mutation testing of the lines this branch changed since origin/main: bin/mutants-diff
mutants-diff:
	bin/mutants-diff

# Check every target on the minimum supported Rust (N-2) and on N-1.
ci-minimum-supported-rust-version:
	bin/make-ci-minimum-supported-rust-version

# Install hooks
install-hooks:
	pre-commit install

# Run pre-commit manually
pre-commit:
	pre-commit run --all-files

# Help
hhelp:
	@echo "Available targets:"
	@echo "  build          - Build the workspace"
	@echo "  build-all      - Build with all features"
	@echo "  test           - Run all tests"
	@echo "  test-all       - Run tests with all features"
	@echo "  test-ignored   - Run ignored tests (requires containers)"
	@echo "  lint           - Run clippy"
	@echo "  fmt            - Check formatting"
	@echo "  doc            - Generate documentation"
	@echo "  clean          - Clean build artifacts"
	@echo "  check          - Run all checks (fmt, lint, test, doc)"
	@echo "  up/down/logs   - Manage development containers"
	@echo "  ci             - Run full CI suite"
	@echo "  audit          - Check for security vulnerabilities"
	@echo "  supply-chain   - Check supply chain security"
