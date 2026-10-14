# Orion - Project Instructions

## About
Orion (full name: **Orion Agent Harness**) is a backend-agnostic agent harness
for local LLM inference, published as the `orion-core` crate. It owns
the conversation loop - context management, token budgets, streaming events, chat
templates, and an automatic tool-execution loop - and leaves inference to you:
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
│   ├── agent.rs                # Agent struct, conversation state, prompt/resume + tool loop, inbox reads, pending calls
│   ├── backend.rs              # LlmBackend and ChatBackend traits, the Backend enum
│   ├── backends.rs             # OpenAiHttpBackend (http-backend feature)
│   ├── context.rs              # Context pipeline: prune strategies, prepare_context, plan_prune
│   ├── error.rs                # CoreError, CoreResult (non_exhaustive)
│   ├── events.rs               # AgentEvent enum (lifecycle + tool exec events, non_exhaustive)
│   ├── inbox.rs                # Inbox: messages and late tool results for an agent mid-run
│   ├── messages.rs             # Message, Role, ToolCall, ToolResult
│   ├── template.rs             # ChatTemplate trait, per-family templates, detect_template
│   └── tools.rs                # Tool trait, ToolContext, ToolSchema, ToolOutput, parse_tool_calls
├── examples/
│   ├── mock_backend.rs         # Runnable in-process backend (quick start)
│   └── openai_backend.rs       # Streaming OpenAI-compatible backend (http-backend feature)
├── tests/                      # Integration + property tests (context, prune, template, tool, long-running)
├── benches/context_bench.rs    # Criterion benchmarks for the context pipeline
├── .github/workflows/ci.yml    # fmt + clippy + test on stable & MSRV 1.85
├── justfile                    # Task runner commands
└── AGENTS.md                   # This file
```

## Documentation
- ALWAYS update `CHANGELOG.md` when making user-facing or API changes - add
  entries under `[Unreleased]`.
- ALWAYS keep `README.md` current. Its code snippets are mirrored by
  compile-checked doctests, so update both together and run `cargo test --doc`.
- Every public item must be documented - `#![deny(missing_docs)]` and
  `deny(rustdoc::broken_intra_doc_links)` are on. Keep `cargo doc` clean in both
  default and `--no-default-features` builds.

## Development
- Use `just` as task runner (see `justfile`)
- Minimum Rust version: 1.85 (covers the default feature set). `resolver = "3"`
  makes Cargo pick dependency versions that still build on it, so check MSRV
  with a real 1.85 toolchain first on `PATH`: `rustup run 1.85.0 cargo` alone
  can still pick up another `rustc`.
- Features: `tools` (default - `Tool` trait + execution loop + `async-trait`,
  and tokio's `macros` for `ToolContext::run_child`); `chat-backend` (default -
  the `ChatBackend` trait, `async-trait`); `http-backend` (optional `reqwest`,
  `OpenAiHttpBackend`, gates the OpenAI example). Tool-call parsing and
  `ToolSchema` stay available with `--no-default-features`.
- Clean code, proper naming, minimal comments, proper docstrings
- Avoid over-commenting. Add only useful comments like docstrings, TODOs and NOTEs.
- Latest stable dependency versions
- After completing a set of tasks, run these checks:
  - `just format` - `cargo fmt`
  - `just lint` - `cargo clippy --all-targets -- -D warnings`
  - `just check` - `cargo check`
  - `just test` - `cargo test`

## Stability
- Follows [SemVer](https://semver.org/). While `0.x`, a minor bump may carry
  breaking changes and a patch bump is additive/fixes only.
- `CoreError` and `AgentEvent` are `#[non_exhaustive]` - keep them that way and
  match with a wildcard arm.
- MSRV is raised only in a minor release. See `CONTRIBUTING.md` for the full policy.

## Git
- Do NOT commit automatically
- Wait for explicit "commit it" instruction
- Then stage all changes, write a short brief commit message, and commit

## Publishing
- Bump the version in `Cargo.toml`, update `CHANGELOG.md`, then:
  - `just publish-dry` - `cargo publish --dry-run` (verify the package)
  - `just publish` - `cargo publish` to crates.io
- Confirm docs.rs builds and tag the release after publishing.

## Project
- Open source, MIT licensed
- Ask instead of assuming when confused
```
