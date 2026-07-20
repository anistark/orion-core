---
title: Getting started
description: Install Orion, wire up a backend, and run your first prompt.
---

## Install

Add the crate with Cargo:

```sh
cargo add orion-core
```

Or in `Cargo.toml`:

```toml
[dependencies]
orion-core = "0.2"
```

The minimum supported Rust version is **1.85**.

## Your first prompt

Orion is backend-agnostic: you implement the `LlmBackend` trait for your
inference engine, then drive the agent. The example below uses a mock backend
that streams a canned reply, so the whole loop runs end to end.

```rust
use std::sync::Arc;
use orion_core::{Agent, AgentConfig, AgentEvent, LlmBackend};
use tokio::sync::mpsc;

// 1. Implement the backend trait for your engine (see "Backend").
let backend: Arc<dyn LlmBackend> = Arc::new(MyBackend::new());

// 2. Create an agent.
let mut agent = Agent::new(AgentConfig {
    system_prompt: "You are a helpful assistant.".into(),
    ..Default::default()
});

// 3. You supply the event channel; the agent streams events into it
//    while generation runs, then returns when the turn is done.
let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

// Consume events concurrently - forward them to your UI.
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

The agent emits an [`AgentEvent`](../../concepts/events/) stream as it works:
message deltas as tokens arrive, tool-execution events, and a context-budget
report before each model call.

## Don't want to manage the channel?

`agent.prompt_stream(text, backend)` creates the channel for you and hands back
`(receiver, future)`. Drive the future (e.g. with `tokio::join!`) while you
drain the receiver:

```rust
let (mut rx, fut) = agent.prompt_stream("What is Rust?", backend);
let (result, _) = tokio::join!(fut, async {
    while let Some(event) = rx.recv().await {
        // handle events
    }
});
result?;
```

## Run the examples

The repository ships two runnable examples:

```sh
# Mock backend - streams a canned reply, no model needed.
cargo run --example mock_backend

# Real OpenAI-compatible backend (OpenAI, llama.cpp server, vLLM, LM Studio, Ollama).
cargo run --example openai_backend --features http-backend
```

See [Examples](../../reference/examples/) for what each one demonstrates.

## Feature flags

The `tools` feature is **on by default** and pulls in
[`async-trait`](https://crates.io/crates/async-trait). Minimal consumers that
only need plain chat can drop it:

```toml
orion-core = { version = "0.2", default-features = false }
```

Tool-call *parsing* (`parse_tool_calls`, `ParsedToolCall`, `ToolSchema`) stays
available either way - only the `Tool` trait and the execution loop require the
feature. See [Tools](../../concepts/tools/) for details.

The `http-backend` feature is **off by default**. Enable it for the supported
[`OpenAiHttpBackend`](../../concepts/backend/#ready-made-the-openai-compatible-http-backend),
a streaming client for any OpenAI-compatible server; it pulls in a blocking HTTP
client, so it stays opt-in:

```toml
orion-core = { version = "0.5", features = ["http-backend"] }
```

## Next steps

- [Architecture](../../concepts/architecture/) - how the pieces fit together.
- [Backend](../../concepts/backend/) - the one trait you implement.
- [Tools](../../concepts/tools/) - give the model abilities.
