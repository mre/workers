# Makefile for workers crate
.PHONY: help build test check fmt clippy clean doc all ci

# Default target
all: fmt clippy test

# Help target - shows available commands
help:
	@echo "Available targets:"
	@echo "  build       - Build the project"
	@echo "  test        - Run all tests (uses TestContainers)"
	@echo "  check       - Check that the project compiles"
	@echo "  fmt         - Format code using rustfmt"
	@echo "  clippy      - Run clippy lints"
	@echo "  clean       - Clean build artifacts"
	@echo "  doc         - Generate documentation"
	@echo "  all         - Run fmt, clippy, and test (default)"
	@echo "  ci          - Run all CI checks (check, clippy, test, fmt-check)"
	@echo ""
	@echo "Note: Tests automatically spin up PostgreSQL containers using TestContainers"

# Build the project
build:
	cargo build

# Build in release mode
build-release:
	cargo build --release

# Run all tests
test:
	cargo test

# Run tests with output
test-verbose:
	cargo test -- --nocapture

# Check that the project compiles without building
check:
	cargo check

# Format code using rustfmt
fmt:
	cargo fmt

# Check formatting without modifying files
fmt-check:
	cargo fmt -- --check

# Run clippy lints
clippy:
	cargo clippy --all-targets --all-features -- -D warnings

# Clean build artifacts
clean:
	cargo clean

# Generate documentation
doc:
	cargo doc --no-deps

# Generate and open documentation
doc-open:
	cargo doc --no-deps --open

# Update dependencies
update:
	cargo update

# Check for outdated dependencies
outdated:
	cargo outdated || echo "Run 'cargo install cargo-outdated' to check for outdated dependencies"

# Security audit
audit:
	cargo audit || echo "Run 'cargo install cargo-audit' to check for security vulnerabilities"

# Run all CI checks
ci: check clippy test fmt-check

# Lint and fix common issues automatically
fix:
	cargo fix --allow-dirty --allow-staged
	cargo clippy --fix --allow-dirty --allow-staged
	cargo fmt