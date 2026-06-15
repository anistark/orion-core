# orion-core — Project Instructions

## About
orion-core is a backend-agnostic agent harness for local LLM inference. It owns
the conversation loop — context management, token budgets, streaming events, chat
templates, and an automatic tool-execution loop — and leaves inference to you:
implement the `LlmBackend` trait for any engine (llama.cpp, MLX, a cloud API).
Pure Rust library, no `tauri`/`llama`/app coupling.

Repository: https://github.com/anistark/orion-core
License: MIT © Kumar Anirudha

## Project Structure
```sh
├── Cargo.toml                  # Crate manifest, features, crates.io metadata
├── README.md                   # Module docs and usage guide (snippets are doctested)
├── src/
│   ├── lib.rs                  # Public API + re-exports; #![deny(missing_docs)]
│   ├── agent.rs                # Agent struct, conversation state, prompt + tool loop
│   ├── backend.rs              # LlmBackend trait (implement for your engine)
│   ├── context.rs              # Context pipeline: prune strategies, prepare_context, plan_prune
│   ├── error.rs                # CoreError, CoreResult (non_exhaustive)
│   ├── events.rs               # AgentEvent enum (lifecycle + tool exec events, non_exhaustive)
│   ├── messages.rs             # Message, Role, ToolCall, ToolResult
│   ├── template.rs             # ChatTemplate trait, per-family templates, detect_template
│   └── tools.rs                # Tool trait, ToolSchema, ToolOutput, parse_tool_calls
├── examples/
│   ├── mock_backend.rs         # Runnable in-process backend (quick start)
│   └── openai_backend.rs       # Streaming OpenAI-compatible backend (openai-example feature)
├── tests/                      # Integration + property tests (context, prune, template, tool)
├── benches/context_bench.rs    # Criterion benchmarks for the context pipeline
├── .github/workflows/ci.yml    # fmt + clippy + test on stable & MSRV 1.85
├── justfile                    # Task runner commands
└── AGENTS.md                   # This file
```

## Documentation
- ALWAYS update `CHANGELOG.md` when making user-facing or API changes — add
  entries under `[Unreleased]`.
- ALWAYS keep `README.md` current. Its code snippets are mirrored by
  compile-checked doctests, so update both together and run `cargo test --doc`.
- Every public item must be documented — `#![deny(missing_docs)]` and
  `deny(rustdoc::broken_intra_doc_links)` are on. Keep `cargo doc` clean in both
  default and `--no-default-features` builds.

## Development
- Use `just` as task runner (see `justfile`)
- Minimum Rust version: 1.85 (covers the default feature set)
- Features: `tools` (default — `Tool` trait + execution loop + `async-trait`);
  `openai-example` (optional `reqwest`, gates the OpenAI example). Tool-call
  parsing and `ToolSchema` stay available with `--no-default-features`.
- Clean code, proper naming, minimal comments, proper docstrings
- Avoid over-commenting. Add only useful comments like docstrings, TODOs and NOTEs.
- Latest stable dependency versions
- After completing a set of tasks, run these checks:
  - `just format` — `cargo fmt`
  - `just lint` — `cargo clippy --all-targets -- -D warnings`
  - `just check` — `cargo check`
  - `just test` — `cargo test`

## Stability
- Follows [SemVer](https://semver.org/). While `0.x`, a minor bump may carry
  breaking changes and a patch bump is additive/fixes only.
- `CoreError` and `AgentEvent` are `#[non_exhaustive]` — keep them that way and
  match with a wildcard arm.
- MSRV is raised only in a minor release. See `CONTRIBUTING.md` for the full policy.

## Git
- Do NOT commit automatically
- Wait for explicit "commit it" instruction
- Then stage all changes, write a short brief commit message, and commit

## Publishing
- Bump the version in `Cargo.toml`, update `CHANGELOG.md`, then:
  - `just publish-dry` — `cargo publish --dry-run` (verify the package)
  - `just publish` — `cargo publish` to crates.io
- Confirm docs.rs builds and tag the release after publishing.

## Project
- Open source, MIT licensed
- Ask instead of assuming when confused
```
