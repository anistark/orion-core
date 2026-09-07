# Changelog

All notable changes to `orion-core` are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/), and this
project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.7.1] - 2026-09-07

### Added
- **`OpenAiHttpBackend` implements `ChatBackend`.** 0.7.0 added the message-native
  path and left the crate's own HTTP backend on the prompt one, which meant the
  collapse it was released to fix still happened to anyone using it against a
  hosted chat API: `OpenAiEndpoint::Chat` delivers an already-templated prompt as
  a single user message. Driven as a `ChatBackend` it now sends
  `/v1/chat/completions` a real message list, system prompt included, and the
  server applies the model's own template. The `LlmBackend` impl is untouched and
  is still the one to use against `/v1/completions`, where a raw prompt is the
  point. `OpenAiConfig::endpoint` is not consulted on the chat path, since a
  message list has only one endpoint it can mean.
- **`PartialEq` on `Message`, `ToolCall` and `ToolResult`,** so a consumer can
  assert on a conversation without comparing it field by field.

### Changed
- **The blocking HTTP client is built on first use.** It carries a runtime of its
  own, and building or dropping one inside an async context panics, so a backend
  driven only through `ChatBackend` must never make one. A failure to build it now
  surfaces from `generate` rather than from `OpenAiHttpBackend::new`.
- Both transports read one shared SSE envelope reader, so the blocking and async
  paths cannot drift on what a usage block or a `[DONE]` means.

### Documented
- The summarize strategy is now tested through a chat backend as well as a prompt
  one. It was planned the same way for both and only ever exercised through one.

## [0.7.0] - 2026-09-06

### Added
- **`ChatBackend`: a backend fed messages rather than a formatted prompt.** A hosted
  chat API takes a structured message list and applies the model's own template
  server-side, so handing it a templated string means either templating twice or
  collapsing the whole conversation into a single user turn - which is what
  `OpenAiHttpBackend` does today against `OpenAiEndpoint::Chat`. Implementors get
  `system` and `&[Message]` and the agent skips its own templating for them.
  Everything else - pruning, the tool loop, the event stream - is unchanged. The
  trait is `async`, because the work behind it is I/O rather than compute and there
  is no reason to hold a blocking thread for it. Gated on the new `chat-backend`
  feature, on by default.
- **`Backend`, the enum a turn actually runs against.** Callers do not name it:
  `Arc<dyn LlmBackend>` and `Arc<dyn ChatBackend>` both convert into it. A local
  engine keeps the `spawn_blocking` path it needs; a hosted one stays async.
- **`Agent::send`.** Runs a turn and answers with the assistant's message, for a
  caller that is not streaming. `prompt` reports everything through events, which
  is what streaming wants and what a caller who only needs the answer had to unpick
  for itself. An error the agent reported as an event comes back as `Err`, since a
  caller with no event stream has nowhere else to see it.
- **`estimate_tokens`.** The four-characters-a-token approximation, public now that
  it is the default for backends with no local tokenizer.

### Changed
- **`Agent::prompt` and `Agent::prompt_stream` take `impl Into<Backend>`.** Existing
  callers passing `Arc<dyn LlmBackend>` are unaffected.
- **`LlmBackend::tokenize_count` and `LlmBackend::is_ready` are provided methods.**
  A remote backend cannot tokenize locally, and making every implementor invent a
  number was worse than defaulting to the estimate. `is_ready` defaults to true,
  which is the right answer for a backend with nothing to load. Existing overrides
  keep working.
- **`PreparedContext` carries `system` and `messages`** beside `prompt`: the system
  block without template markup, and the turns that survived pruning. Breaking for
  anyone constructing the struct by hand rather than taking it from
  `prepare_context`.

## [0.6.0] - 2026-07-23

### Added
- **Supported OpenAI-compatible HTTP backend.** `backends::OpenAiHttpBackend`
  (behind the new `http-backend` feature) is a streaming client that works
  against OpenAI, llama.cpp `llama-server`, vLLM, LM Studio, and Ollama by
  changing only the base URL. Tokens stream through `on_token`; the
  `GenerationResult` carries the real token counts from the response `usage`
  block; the abort flag is honored between chunks. Configured via `OpenAiConfig`
  (`base_url`, `model`, optional `api_key`, endpoint, request timeout).
  `OpenAiEndpoint` selects `/v1/chat/completions` (default - the formatted prompt
  is sent as a user message) or `/v1/completions` (the already-templated prompt
  is sent verbatim, avoiding double-templating against a local completion
  endpoint). Promotes what was previously the `openai-example` example to a
  depended-on module. New `CoreError::BackendUnreachable` distinguishes a
  transport failure (no response - safe to retry or fail over) from
  `CoreError::Backend` (the endpoint answered with an error).
- **Pre-execute approval hook.** `Agent::set_approval_hook` installs an
  `ApprovalHook` that is consulted once per parsed tool call - after parsing,
  before execution - so a host can authorize, sandbox, or interactively confirm
  tool use (e.g. a per-call "allow this?" prompt). The hook is `async` and may
  block for as long as it needs, including awaiting a human decision. Returning
  `ApprovalDecision::Deny { reason }` skips the call, feeds `reason` back to the
  model as an error tool result so it can adapt, and emits the new
  `AgentEvent::ToolDenied` event (distinct from an execution failure). With no
  hook installed the tool loop behaves exactly as before. Gated on the `tools`
  feature.

### Changed
- The `openai-example` cargo feature is replaced by `http-backend`, which now
  gates the supported `backends::OpenAiHttpBackend` module (the `openai_backend`
  example is a thin demo of it).

### Documented
- **`AgentEvent::GenerationStats` emission guarantee.** Documented and pinned
  with tests: exactly one `GenerationStats` is emitted per completed LLM
  iteration within a `prompt()` call, always before the closing `AgentEnd`, so
  summing them yields exact per-run token totals with no gaps or double counting.
  Behaviour unchanged; the contract is now explicit for consumers metering usage.

## [0.5.0] - 2026-06-15

First release as a standalone, independently published crate.

### Added
- **More chat templates.** `Llama2Template`, `GemmaTemplate`, `Phi3Template`,
  `DeepSeekTemplate`, and `CommandRTemplate` join the existing set. Both
  `detect_template()` (GGUF metadata) and `template_from_name()` (manual
  override, with aliases) now resolve them. Supported set documented in the
  README.
- `Agent::prompt_stream(text, backend)` - convenience that creates the event
  channel and returns `(receiver, future)`, so callers don't have to wire up the
  `mpsc` themselves.
- **`tools` cargo feature** (enabled by default) gating the `Tool` trait,
  `ToolOutput`, `ToolUpdateCallback`, `Agent::set_tools`, and the execution loop.
  Build with `--no-default-features` to drop the `async-trait` dependency for
  minimal chat-only consumers; tool-call *parsing* and `ToolSchema` stay
  available regardless.
- `examples/openai_backend.rs` - a streaming OpenAI-compatible HTTP backend
  (behind the `openai-example` feature) demonstrating a real over-the-wire
  `LlmBackend`.
- Full `#![deny(missing_docs)]` coverage and compile-checked doctests mirroring
  the README snippets; property tests for the pruning invariants and `criterion`
  benchmarks for the context pipeline.
- **Tool execution loop.** `Agent::prompt` now parses tool calls from the
  model's reply, runs the matching registered `Tool`, appends a tool-result
  message, and loops back to the LLM until it returns a tool-free answer -
  emitting `ToolExecStart` / `ToolExecUpdate` / `ToolExecEnd` along the way.
- `parse_tool_calls()` + `ParsedToolCall` - lenient parser for the advertised
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
- `plan_prune()` + `PrunePlan` - exposes the turn-level keep/drop decision
  (`prepare_context` is now a thin formatter over it); used by the summarizer to
  find which turns to fold.
- `Agent::replace_messages()` advances the id counter past restored `msg-N` ids
  to avoid collisions (pins/tool results are addressed by id).

### Changed
- `CoreError` and `AgentEvent` are now `#[non_exhaustive]` for forward
  compatibility - downstream `match`es must include a wildcard arm. Documented
  the SemVer + MSRV (Rust 1.85, default features) stability policy in
  `CONTRIBUTING.md` and the README.

## [0.2.0] - 2026-06

### Added
- `ChatTemplate` trait with built-in `ChatMLTemplate`, `Llama3Template`,
  `MistralTemplate`, `AlpacaTemplate`, and `VicunaTemplate` implementations.
- `detect_template()` - resolves a chat template from a GGUF metadata template
  string (Llama 3 → ChatML → Mistral `[INST]` → Alpaca → Vicuna), falling back
  to ChatML.
- `template_from_name()` - resolves manual-override names (with aliases) to a
  template, returning `None` for unimplemented families.
- `prepare_context()` - single pass that prunes message pairs to fit the token
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
