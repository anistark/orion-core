default:
    @just --list

# Format code
format:
    cargo fmt

# Lint with clippy (matches CI: lints examples and tests too)
lint:
    cargo clippy --all-targets -- -D warnings

# Type check
check:
    cargo check

# Run tests
test:
    cargo test

# Build release
build:
    cargo build --release

# Clean build artifacts
clean:
    cargo clean

# Publish to crates.io (dry run)
publish-dry:
    cargo publish --dry-run

# Publish to crates.io
publish:
    cargo publish
