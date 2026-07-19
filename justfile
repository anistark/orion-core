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

# ── Docs (Astro Starlight site in docs/, uses pnpm) ──────────────────

# Install docs site dependencies
docs-install:
    cd docs && pnpm install

# Build the static docs site (output: docs/dist)
docs-build:
    cd docs && pnpm run build

# Serve the docs locally with hot reload (http://localhost:4321/orion-core/)
docs:
    cd docs && pnpm run dev

# Serve the docs exposed on the local network (LAN, Tailscale, etc.)
docs-host:
    cd docs && pnpm exec astro dev --host

# Preview the production build locally
docs-preview:
    cd docs && pnpm run preview
