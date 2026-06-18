<!-- Thanks for contributing to Orion! -->

## What & why

<!-- What does this change do, and what problem does it solve? Link any issue. -->

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --all-targets` and `cargo test --doc` pass
- [ ] Public API changes are documented (`///` docs / README)
- [ ] `CHANGELOG.md` updated under `[Unreleased]` (for user-facing changes)
- [ ] Change stays backend-agnostic (no host/engine-specific coupling)
