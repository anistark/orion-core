# Changelog

All notable changes to `orion-core` are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/), and this
project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.9.0] - 2026-10-23

### Added
- **Agent Skills (feature `skills`).** `skills::Skill` loads a `SKILL.md`
  folder (frontmatter `name`, `description`, `allowed-tools`), with `body`,
  `files` and `read_file`, which never leaves the skill's folder.
  `discover_skills` finds skills the way Claude Code reads a plugin's `skills`
  setting. `SkillSet` keeps them by unique name, renders a system-prompt index
  with `system_section`, and with `tools` hands out the `load_skill` tool
  (`SkillTool`) that returns a skill's instructions or one of its files.
- **`/skill` commands.** `SkillSet::parse_command` finds the skill a
  `/name request` message names, and `expand_command` turns it into a message
  carrying the skill's instructions, so the model follows a skill the user
  picked without calling `load_skill`. `skills::parse_expanded` reads the skill
  and request back out, for showing the message as typed.
- **Plugin marketplaces (feature `plugins`).** `plugins::Catalog` parses and
  fetches Claude Code marketplace catalogs (`marketplace.json` or a bare array)
  with every source shape: `github`, `url`, `git-subdir`, `npm`, catalog paths.
  `PluginStore` installs plugins from git into `~/.agents/plugins` in two steps,
  `prepare` (fetch into staging, nothing runs) and `commit`, plus `cancel`,
  `remove`, `latest_commit` and a catalog cache. `PluginContents` reads the MCP
  servers (`plugin.json`, `.mcp.json` or the catalog entry, with
  `${CLAUDE_PLUGIN_ROOT}` filled in) and skills a plugin ships, and lists the
  parts only Claude Code uses.
- `CoreError::Skill` and `CoreError::Plugin`.

### Changed
- **`http-backend` moves to reqwest 0.13.** TLS now goes through
  `rustls-platform-verifier`, so certificates are checked against the OS trust
  store (and its revocations) instead of a bundled root list. This picks up the
  October 2026 eMudhra intermediate-CA distrust from macOS and Windows updates;
  on Linux, keep `ca-certificates` current. Lockfiles resolve the patched
  `rustls-webpki` 0.103.16 and `webpki-root-certs` 1.0.10. The `http-backend`
  feature turns on reqwest's `blocking` client itself.
- Benchmarks use criterion 0.7; the docs site moves to Astro 7 and
  Starlight 0.42.

## [0.8.0] - 2026-10-14

### Added
- **A tool can hand back a result later.** `ToolOutput::pending(content)` records
  a placeholder as the call's result, marked `ToolResult::pending`, so the model
  knows the work is under way and the conversation stays well formed. The call is
  listed by `Agent::pending_calls` until its result is delivered. Pending calls
  are worked out from the messages, so they survive a save and restore.
- **`Inbox`: a way into an agent while it runs.** `Agent::inbox` hands out a
  cloneable handle. `push` queues a user message, and `resolve` and `reject`
  deliver a pending call's result, which fires `AgentEvent::ToolResolved`. The
  agent reads the inbox at the start of a run, before every model call after the
  first, and when the model gives its final answer, so anything that lands while
  it writes is answered in the same run.
- **`Agent::resume` carries a conversation on without new user text.** Use it
  after a result lands between runs, after a pause, or to pick up a conversation
  restored with `replace_messages`. A tool call that a run left without a result,
  because the process stopped mid-round, is marked failed with a `Warning` and
  never run again. `Agent::needs_reply` says whether there is anything to resume.
- **`AgentConfig::pause_on_pending`.** With it set, a turn that leaves calls
  pending ends the run with `AgentEvent::Paused` rather than letting the model
  answer around them. It is off by default.
- **`ToolContext`, handed to every tool call.** It carries the call's id and
  tool name, the run's stop flag (`is_aborted`, `abort_flag`), `update` for
  progress and `forward` for passing events into the run.
- **`ToolContext::run_child` runs a child agent inside a tool call.** The child's
  events stream live into the parent's as `AgentEvent::Nested` under the call
  that started it, stopping the parent stops the child, and the call answers
  with the child's reply.
- `ToolOutput::new`, `with_details` and a `Default` impl, so building an output
  no longer means spelling out every field.

### Changed
- **Breaking: `Tool::execute` takes `(args, ctx: &ToolContext)`.** It used to
  take `(tool_call_id, args, on_update)`. The call id is `ctx.call_id()`, and
  `on_update(partial)` is `ctx.update(partial)`. `ToolUpdateCallback` is gone.
- **Breaking: `ToolOutput`, `ToolResult` and `AgentConfig` each gain a field**
  (`pending`, `pending`, `pause_on_pending`). Code that builds one with a struct
  literal needs the new field, `..Default::default()`, or a constructor.
  Serialized messages from earlier releases still load, since a missing
  `pending` reads as false.
- **`AgentEnd.messages` reports what a failed run left in the conversation.** It
  used to be empty whenever generation failed, even when earlier turns of the
  same run stayed in the conversation. It now lists every message the run added
  that is still there, other than the prompt's own user message, which is
  removed when the first generation fails.
- The `tools` feature now enables tokio's `macros` feature.

### Documented
- **`ChatBackend`, `Agent::send` and the two paths through `OpenAiHttpBackend`**
  are covered in the README and the guide. Both said there was one trait to
  implement, and described the chat endpoint only as a prompt sent in a single
  user message, which has not been the whole story since 0.7.1.
- Event sequences now show `TurnStart` after the user message and include
  `GenerationStats`, as the agent emits them. `MessageStart` is documented as
  firing for user and tool-result messages only.
- `Message::token_count` is documented as left unset by the agent, and the Quick
  Start reads its token count from `GenerationStats`, since it always printed 0.
- The templates guide passes an `Arc`, which `with_template` and `set_template`
  take, and the snippet is now a doctest. `AGENTS.md` and `CONTRIBUTING.md` name
  the real features in place of a nonexistent `openai-example`.
- `cargo doc --no-default-features` builds again: two links to `ChatBackend`
  from `PreparedContext` resolved only with the feature on.

## [0.7.2] - 2026-09-16

### Fixed
- **A fence tag with stray punctuation is still a tool call.** Models drift to
  ```` ```tool_call> ```` and the like, and `parse_tool_calls` matched the tag to
  the letter, so every call written that way was passed over: the model got no
  result, and a consumer saw the JSON as the reply. The tag is now matched after
  trimming punctuation from both ends, so ```` ```tool_call> ````,
  ```` ```<tool_call> ```` and ```` ```JSON: ```` all parse.

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
