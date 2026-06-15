# orion-core

[![Crates.io](https://img.shields.io/crates/v/orion-core.svg?style=flat-square)](https://crates.io/crates/orion-core)
[![docs.rs](https://img.shields.io/docsrs/orion-core?style=flat-square)](https://docs.rs/orion-core)
[![CI](https://img.shields.io/github/actions/workflow/status/anistark/orion-core/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/anistark/orion-core/actions/workflows/ci.yml)
[![Downloads](https://img.shields.io/crates/d/orion-core.svg?style=flat-square)](https://crates.io/crates/orion-core)
[![License: MIT](https://img.shields.io/crates/l/orion-core.svg?style=flat-square)](LICENSE)
![MSRV](https://img.shields.io/badge/MSRV-1.85-blue?style=flat-square)

Agent harness for local LLM inference. Backend-agnostic — bring your own model runtime (llama.cpp, MLX, cloud APIs, anything).

orion-core handles the conversation loop so you don't have to: context management, token budgets, streaming events, chat formatting, and an automatic tool-execution loop (the agent parses tool calls, runs your tools, feeds the results back, and repeats until the model gives a final answer — see [`tools`](#tools--give-the-model-abilities)).

## How It Works

```sh
User sends "Hello"
  → Agent.prompt("Hello")
    → Context pipeline (prune old messages to fit token budget)
      → Format prompt (ChatML template + tool definitions)
        → LlmBackend.generate() (streams tokens one by one)
          → AgentEvent stream (your UI subscribes here)
            → If the model called tools: run them, append results, loop back
              → Done (model returns a tool-free answer)
```

You implement one trait (`LlmBackend`) for your inference engine. orion-core handles everything above it.

## Quick Start

```rust
use std::sync::Arc;
use orion_core::{Agent, AgentConfig, AgentEvent, LlmBackend};
use tokio::sync::mpsc;

// 1. Implement the backend trait for your engine (see `backend` below)
let backend: Arc<dyn LlmBackend> = Arc::new(MyBackend::new());

// 2. Create an agent
let mut agent = Agent::new(AgentConfig {
    system_prompt: "You are a helpful assistant.".into(),
    ..Default::default()
});

// 3. You supply the event channel; the agent streams events into it
//    while generation runs, then returns when the turn is done.
let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

// Consume events concurrently — forward them to your UI.
let consumer = tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::MessageDelta { delta, .. } => print!("{delta}"),
            AgentEvent::MessageEnd { message } => {
                println!("\n\nDone: {} tokens", message.token_count.unwrap_or(0));
            }
            AgentEvent::ContextBudget { used_tokens, max_tokens, .. } => {
                println!("Context: {used_tokens}/{max_tokens} tokens");
            }
            AgentEvent::Error { message } => eprintln!("Error: {message}"),
            _ => {}
        }
    }
});

agent.prompt("What is Rust?", backend, tx).await?;
consumer.await?;
```

> A complete, runnable version lives in [`examples/mock_backend.rs`](examples/mock_backend.rs) — try it with `cargo run --example mock_backend`.

> Don't want to manage the channel yourself? `agent.prompt_stream(text, backend)` creates it for you and hands back `(receiver, future)` — drive the future (e.g. with `tokio::join!`) while you drain the receiver.

> For a real over-the-wire backend, [`examples/openai_backend.rs`](examples/openai_backend.rs) implements `LlmBackend` against a streaming OpenAI-compatible `/v1/completions` endpoint (OpenAI, llama.cpp `server`, vLLM, LM Studio, Ollama). Run it with `cargo run --example openai_backend --features openai-example`.

## Modules

### `agent` — The Orchestrator

The `Agent` struct is the main entry point. It owns the conversation state and drives the prompt → LLM → response loop.

```rust
use orion_core::{Agent, AgentConfig, InferenceParams, ContextConfig};

let mut agent = Agent::new(AgentConfig {
    system_prompt: "You are a coding assistant.".into(),
    inference_params: InferenceParams {
        max_tokens: 4096,
        temperature: 0.4,
        context_size: 8192,
        n_threads: 6,
    },
    context_config: ContextConfig {
        max_context_tokens: 8192,
        max_response_tokens: 4096,
        ..Default::default()
    },
    ..Default::default()
});

// Change settings on the fly
agent.set_system_prompt("You are a pirate.");
agent.set_inference_params(InferenceParams { temperature: 1.2, ..Default::default() });

// Conversation management
agent.clear();                              // Reset conversation
agent.replace_messages(saved_messages);     // Restore a saved conversation

// Abort a running generation
agent.abort();
```

### `backend` — Bring Your Own LLM

Implement the `LlmBackend` trait to plug in any inference engine:

```rust
use orion_core::{LlmBackend, InferenceParams, GenerationResult, TokenCallback, CoreResult};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

struct LlamaCppBackend {
    // your engine state
}

impl LlmBackend for LlamaCppBackend {
    fn generate(
        &self,
        prompt: &str,               // Fully formatted (chat template applied)
        params: &InferenceParams,    // max_tokens, temperature, context_size
        abort: Arc<AtomicBool>,      // Check this each token to support cancellation
        on_token: TokenCallback,     // Call with (token_text, count, tokens_per_sec)
    ) -> CoreResult<GenerationResult> {
        // Feed prompt, sample tokens, call on_token for each one.
        // Return final stats when done.
        todo!()
    }

    fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
        // Count tokens without running inference.
        // Used by the context pipeline for budget management.
        todo!()
    }

    fn is_ready(&self) -> bool {
        // Whether a model is loaded and ready for inference.
        todo!()
    }
}
```

The backend runs on a blocking thread — no async required. orion-core handles the async orchestration.

### `messages` — Conversation Data

Messages support five roles covering the full agent lifecycle:

```rust
use orion_core::Message;

// Standard conversation
let sys = Message::system("msg-1", "You are helpful.");
let user = Message::user("msg-2", "Hello!");
let asst = Message::assistant("msg-3", "Hi there!");

// Tool interaction
let result = Message::tool_result(
    "msg-4",            // message id
    "call-1",           // tool_call_id (links to the assistant's request)
    "read_file",        // tool name
    "file contents...", // result content
    false,              // is_error
);
```

**Roles:** `System`, `User`, `Assistant`, `ToolCall`, `ToolResult`

Every message has an `id`, `timestamp`, and optional `token_count` (populated after tokenization). Assistant messages can carry `tool_calls`; tool result messages carry a `tool_result`.

### `events` — Real-Time UI Updates

The agent emits events as it processes. Subscribe to these for building reactive UIs.

**Event sequence for a simple prompt:**

```sh
AgentStart
TurnStart
MessageStart    { user message }
MessageEnd      { user message }
ContextBudget   { used: 120, max: 4096, included: 5, pruned: 0 }
MessageDelta    { delta: "Hello", tokens: 1, tps: 45.2 }
MessageDelta    { delta: " there", tokens: 2, tps: 46.1 }
MessageDelta    { delta: "!", tokens: 3, tps: 44.8 }
MessageEnd      { assistant message }
TurnEnd         { message, tool_results: [] }
AgentEnd        { messages: [...] }
```

**Event sequence with tool calls:**

```sh
AgentStart
MessageStart    { user message }
MessageEnd      { user message }
TurnStart
ContextBudget   ...
MessageDelta    ...
MessageEnd      { assistant message with tool_calls }
ToolExecStart   { tool_call_id, tool_name, args }
ToolExecUpdate  { partial progress }
ToolExecEnd     { result }
MessageStart    { tool_result message }
MessageEnd      { tool_result message }
TurnEnd         { message, tool_results: [...] }
TurnStart                            ← new turn: LLM responds to tool result
MessageDelta    ...
MessageEnd      { final assistant message }
TurnEnd         { message, tool_results: [] }
AgentEnd        { messages: [...] }
```

**All event types:**

| Event | When | Key Data |
|-------|------|----------|
| `AgentStart` | Processing begins | — |
| `AgentEnd` | All done | All new messages |
| `TurnStart` | New LLM call begins | — |
| `TurnEnd` | LLM call + tools done | Assistant message, tool results |
| `MessageStart` | Any message added | Full message |
| `MessageDelta` | Each streamed token | `delta`, `tokens_generated`, `tokens_per_sec` |
| `MessageEnd` | Message complete | Full message |
| `ToolExecStart` | Tool begins running | Tool name, args |
| `ToolExecUpdate` | Tool streams progress | Partial output |
| `ToolExecEnd` | Tool finished | Result, is_error |
| `ContextBudget` | After context prep | Tokens used/max, messages included/pruned |
| `Warning` | Non-fatal issue | Warning text |
| `Error` | Fatal error | Error text |

### `context` — Token Budget Management

Handles the hard problem of fitting a conversation into a fixed-size context window.

**What it does:**
1. **Prunes** old messages when the conversation exceeds the token budget (sliding window — keeps system prompt + most recent messages)
2. **Formats** the surviving messages into a ChatML prompt string
3. **Reports** how many tokens are used and how many messages were pruned

```rust
use orion_core::ChatMLTemplate;
use orion_core::context::{prepare_context, ContextConfig};

let token_counter = |text: &str| -> u32 { /* your tokenizer */ 0 };

// Prune to fit the budget *and* format with a chat template in one step.
let prepared = prepare_context(
    &ChatMLTemplate,           // any `ChatTemplate` impl
    "You are helpful.",        // system prompt
    &messages,                 // full conversation history
    &tool_schemas,             // tool schemas to inject (may be empty)
    &ContextConfig::default(),
    &token_counter,
)?;

// `prepared.prompt` is the formatted string to feed your backend.
println!(
    "{} tokens, {} kept, {} pruned",
    prepared.token_count, prepared.messages_included, prepared.messages_pruned,
);
```

The agent calls this automatically before each LLM call. You don't need to call it directly unless you want custom control.

**Prune strategies** (`ContextConfig::prune_strategy`):
- `SlidingWindow` (default) — drop the oldest turns first to fit the budget.
- `Summarize` — before pruning, the agent folds the oldest overflowing turns
  into a single **pinned** summary message (one extra backend call), so their
  gist survives instead of being dropped. Prior summaries are consolidated, so
  exactly one accumulates. Best-effort: if summarization fails it falls back to
  the sliding window.

**Pinned messages.** Any `Message` with `pinned == true` always survives pruning,
regardless of budget or strategy. Build one with `Message::user(id, text).pinned()`,
or toggle an existing message via `agent.set_pinned(message_id, true)`. Pruning is
turn-aware, so a pinned message keeps its whole turn (no orphaned pairs).

### `template` — Chat Prompt Formats

Each model family wants its prompt wrapped a certain way. orion-core ships a
`ChatTemplate` for the common ones and picks the right one automatically.

**Supported families:** ChatML (default), Llama 3, Llama 2, Mistral / Mixtral,
Gemma / Gemma 2, Phi-3, DeepSeek (LLM chat), Command-R / Command-R+, Alpaca, and
Vicuna.

- `detect_template(gguf_template)` — inspects a GGUF metadata template string and
  returns the matching impl (falling back to ChatML when nothing matches).
- `template_from_name(name)` — resolves a manual-override name (with common
  aliases, e.g. `llama-2`, `phi-3`, `cohere`) to a template, or `None` for an
  unimplemented family so the caller can fall back to auto-detection.
- `Agent::with_template(config, template)` / `agent.set_template(template)` —
  set or swap the template at runtime.

Every template also implements the per-message and per-system formatting hooks
the context pipeline needs for accurate token-budget accounting, and advertises
tools through the same `tool_call` convention (see below).

### `tools` — Give the Model Abilities

`Agent::prompt` drives the full cycle automatically: it injects your tool
schemas into the system prompt, parses the model's tool calls out of its reply,
runs the matching tool, appends the result to the conversation, and loops back
to the model until it returns a tool-free answer (bounded by
`AgentConfig::max_tool_iterations`, default 8). Each step emits
`ToolExecStart` / `ToolExecUpdate` / `ToolExecEnd` events.

**Tool-call convention.** Templates advertise — and [`parse_tool_calls`] reads —
a fenced JSON block:

````text
```tool_call
{"name": "read_file", "arguments": {"path": "Cargo.toml"}}
```
````

A JSON array invokes several tools in one turn. Parsing is lenient: a ` ```json `
fence or a whole-message bare JSON object carrying both `name` and `arguments`
also count, so smaller models still trigger tools when they drift from the exact
format. Register tools with `agent.set_tools(vec![Box::new(MyTool)])`; with no
tools registered, parsing is skipped entirely and replies pass through verbatim.

Define tools the model can call. Each tool has a name, description, JSON Schema for parameters, and an async `execute` function.

```rust
use orion_core::{Tool, ToolOutput, ToolSchema, CoreResult};
use orion_core::tools::ToolUpdateCallback;
use async_trait::async_trait;

struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }
    fn label(&self) -> &str { "Read File" }
    fn description(&self) -> &str { "Read a file from disk" }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to read" }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        args: serde_json::Value,
        _on_update: Option<ToolUpdateCallback>,
    ) -> CoreResult<ToolOutput> {
        let path = args["path"].as_str().unwrap_or("");
        let content = std::fs::read_to_string(path)
            .map_err(|e| orion_core::CoreError::Tool(e.to_string()))?;
        Ok(ToolOutput {
            content,
            details: serde_json::json!({"path": path}),
        })
    }
}

// Register with the agent
agent.set_tools(vec![Box::new(ReadFileTool)]);
```

Tool schemas are automatically injected into the system prompt when formatting context, and `Agent::prompt` runs the full tool call → execute → feed result → LLM responds cycle for you (see the section intro above).

**Opting out.** The `Tool` trait and the execution loop live behind the default
`tools` feature, which pulls in [`async-trait`](https://crates.io/crates/async-trait).
Minimal consumers that only need plain chat can drop it:

```toml
orion-core = { version = "0.2", default-features = false }
```

Tool-call *parsing* (`parse_tool_calls`, `ParsedToolCall`) and `ToolSchema` stay
available either way — only the `Tool` trait, `ToolOutput`, `ToolUpdateCallback`,
and `Agent::set_tools` require the feature.

> The code snippets in this README are mirrored by compile-checked [doctests](https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html) on the corresponding API items, so they can't silently drift from the real signatures. Run them with `cargo test --doc`.

### `error` — Error Types

```rust
use orion_core::{CoreError, CoreResult};

// Error variants
CoreError::Backend("No model loaded".into())    // LLM backend issues
CoreError::Context("Token limit exceeded".into()) // Context pipeline issues
CoreError::Tool("File not found".into())          // Tool execution issues
CoreError::Agent("Empty message".into())          // Agent logic issues
CoreError::Aborted                                // User cancelled
```

All errors are serializable (implements `Serialize`) for easy transport over IPC.

## Architecture

```sh
┌──────────────────────────────────────────────────┐
│  Your Application (OrionPod, CLI, server, etc.)  │
├──────────────────────────────────────────────────┤
│  Agent                                           │
│  ├── prompt("Hello")                             │
│  ├── Conversation state (Vec<Message>)           │
│  ├── System prompt                               │
│  ├── Registered tools                            │
│  └── AgentConfig (inference params, context cfg) │
├──────────────────────────────────────────────────┤
│  Context Pipeline                                │
│  └── prepare_context() — prune + template format │
├──────────────────────────────────────────────────┤
│  LlmBackend (trait) ← you implement this         │
│  ├── generate() — run inference, stream tokens   │
│  ├── tokenize_count() — count tokens             │
│  └── is_ready() — check model status             │
├──────────────────────────────────────────────────┤
│  Your inference engine                           │
│  (llama.cpp, MLX, ONNX, cloud API, etc.)         │
└──────────────────────────────────────────────────┘
```

Events flow upward through an unbounded channel (`tokio::sync::mpsc`). Your UI or application layer subscribes to `AgentEvent`s and reacts in real time.

## Stability

orion-core follows [SemVer](https://semver.org/). While `0.x`, a minor bump may
carry breaking changes and a patch bump is additive/fixes only. `CoreError` and
`AgentEvent` are `#[non_exhaustive]` — match them with a wildcard arm so new
variants don't break your build. The MSRV is **Rust 1.85** (raised only in a
minor release), and that guarantee covers the default feature set; optional
example features like `openai-example` may need a newer toolchain. See
[CONTRIBUTING.md](CONTRIBUTING.md#stability--versioning) for the full policy.

## License

MIT © Kumar Anirudha. See [LICENSE](LICENSE).
