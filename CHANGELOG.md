# Changelog

All notable changes to `orion-core` are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/), and this
project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.5.0] — 2026-06-15

First release as a standalone, independently published crate.

### Added
- **More chat templates.** `Llama2Template`, `GemmaTemplate`, `Phi3Template`,
  `DeepSeekTemplate`, and `CommandRTemplate` join the existing set. Both
  `detect_template()` (GGUF metadata) and `template_from_name()` (manual
  override, with aliases) now resolve them. Supported set documented in the
  README.
- `Agent::prompt_stream(text, backend)` — convenience that creates the event
  channel and returns `(receiver, future)`, so callers don't have to wire up the
  `mpsc` themselves.
- **`tools` cargo feature** (enabled by default) gating the `Tool` trait,
  `ToolOutput`, `ToolUpdateCallback`, `Agent::set_tools`, and the execution loop.
  Build with `--no-default-features` to drop the `async-trait` dependency for
  minimal chat-only consumers; tool-call *parsing* and `ToolSchema` stay
  available regardless.
- `examples/openai_backend.rs` — a streaming OpenAI-compatible HTTP backend
  (behind the `openai-example` feature) demonstrating a real over-the-wire
  `LlmBackend`.
- Full `#![deny(missing_docs)]` coverage and compile-checked doctests mirroring
  the README snippets; property tests for the pruning invariants and `criterion`
  benchmarks for the context pipeline.
- **Tool execution loop.** `Agent::prompt` now parses tool calls from the
  model's reply, runs the matching registered `Tool`, appends a tool-result
  message, and loops back to the LLM until it returns a tool-free answer —
  emitting `ToolExecStart` / `ToolExecUpdate` / `ToolExecEnd` along the way.
- `parse_tool_calls()` + `ParsedToolCall` — lenient parser for the advertised
  ```` ```tool_call ```` JSON convention (fenced `tool_call`/`json` blocks, or a
  whole-message bare JSON object with `name` + `arguments`).
- `AgentConfig::max_tool_iterations` (default 8) bounds the tool loop; the agent
  emits a `Warning` and stops if exhausted.
- Re-exported `ToolUpdateCallback` from the crate root.
- **Pinned messages.** `Message` gains a `pinned` flag (and a `Message::pinned()`
  builder); pinned messages always survive context pruning, turn-aware so a pin
  never orphans its pair. `Agent::set_pinned(id, bool)` toggles a message by id.
- **`PruneStrategy::Summarize`.** When the conversation overflows, the agent
  folds the oldest dropped turns (and any prior summary) into a single pinned
  summary message via one extra backend call, instead of discarding them.
  Best-effort, falling back to the sliding window on failure.
  `Agent::set_prune_strategy()` selects the strategy.
- `plan_prune()` + `PrunePlan` — exposes the turn-level keep/drop decision
  (`prepare_context` is now a thin formatter over it); used by the summarizer to
  find which turns to fold.
- `Agent::replace_messages()` advances the id counter past restored `msg-N` ids
  to avoid collisions (pins/tool results are addressed by id).

### Changed
- `CoreError` and `AgentEvent` are now `#[non_exhaustive]` for forward
  compatibility — downstream `match`es must include a wildcard arm. Documented
  the SemVer + MSRV (Rust 1.85, default features) stability policy in
  `CONTRIBUTING.md` and the README.

## [0.2.0] — 2026-06

### Added
- `ChatTemplate` trait with built-in `ChatMLTemplate`, `Llama3Template`,
  `MistralTemplate`, `AlpacaTemplate`, and `VicunaTemplate` implementations.
- `detect_template()` — resolves a chat template from a GGUF metadata template
  string (Llama 3 → ChatML → Mistral `[INST]` → Alpaca → Vicuna), falling back
  to ChatML.
- `template_from_name()` — resolves manual-override names (with aliases) to a
  template, returning `None` for unimplemented families.
- `prepare_context()` — single pass that prunes message pairs to fit the token
  budget, applies the chat template, and reports tokens used / messages kept /
  messages pruned. Pruning is turn-aware and never orphans a user/assistant
  pair. System-prompt and tool-schema tokens are deducted before pruning.
- `AgentEvent::GenerationStats` carrying measured tokens/sec, time-to-first-token,
  and generation time; `GenerationResult` exposes the same fields.
- `Agent::with_template()` / `set_template()` for runtime template switching.

### Changed
- `Agent::prompt` now takes a caller-supplied `mpsc::UnboundedSender<AgentEvent>`
  and streams events as generation runs, returning `CoreResult<()>`.
- Context overflow (system prompt or latest message exceeding the budget)
  surfaces as `CoreError::Context`.

## [0.1.0]

### Added
- Initial agent harness: the `Agent` loop, the `LlmBackend` trait, `Message` /
  `Role` types, the `AgentEvent` system, `Tool` trait scaffolding, and a
  sliding-window context pipeline.
