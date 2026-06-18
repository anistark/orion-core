# Contributing to Orion

Thanks for your interest in improving Orion! The `orion-core` crate is the
backend-agnostic agent harness that powers [OrionPod](https://orionpod.com), and
contributions of all kinds are welcome.

## Getting started

```sh
# Requires Rust 1.85+ (the crate's MSRV)
git clone https://github.com/anistark/orion-core
cd orion-core

cargo build
cargo test
cargo run --example mock_backend   # see the agent loop end to end
```

If you have [`just`](https://github.com/casey/just), the common tasks are
wrapped as recipes — run `just` to list them (`just check`, `just lint`,
`just test`, `just format`).

## Before you open a PR

Please make sure these pass — CI runs the same checks:

```sh
cargo fmt --all --check                       # formatting
cargo clippy --all-targets -- -D warnings     # lints (warnings are errors)
cargo test --all-targets
cargo test --doc                              # doctests
```

## Code style

- Keep the crate **backend-agnostic** — no dependency on any specific inference
  engine, runtime, or application. If a change only makes sense for one host,
  it probably belongs in the host, not here.
- Match the surrounding code: clear names, minimal comments. Document **public**
  items with `///` doc comments; keep examples in docs/README compiling.
- New public behavior should come with a test in `tests/` (or a doctest).

## Stability & versioning

- The crate follows [SemVer](https://semver.org/). While `0.x`, a minor bump
  (`0.y.0`) may carry breaking changes; patch bumps (`0.y.z`) are additive or
  bug fixes only. Any public API removal or rename goes in a minor bump.
- `CoreError` and `AgentEvent` are `#[non_exhaustive]`: always match them with a
  wildcard arm so added variants don't break downstream builds.
- **MSRV is Rust 1.85**, and raising it is a minor-version (never a patch)
  change. The MSRV guarantee covers the **default feature set** only — optional
  example features such as `openai-example` pull a heavier dependency tree, may
  require a newer toolchain, and are exercised on stable rather than on MSRV.

## Commit messages & PRs

- Use [Conventional Commits](https://www.conventionalcommits.org/) for the
  subject (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`).
- Keep PRs focused. Describe what changed and why; link any related issue.
- Update `CHANGELOG.md` under `[Unreleased]` for user-facing changes.

## Reporting bugs & proposing features

Open an issue with a minimal reproduction (a small `LlmBackend` mock is usually
enough — see `examples/mock_backend.rs`) or a clear description of the proposed
API and its motivation.

By contributing, you agree that your contributions are licensed under the
project's [MIT license](LICENSE).
