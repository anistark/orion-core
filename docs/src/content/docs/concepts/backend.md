---
title: Backend
description: Implement LlmBackend or ChatBackend to plug in any inference engine.
---

Orion is backend-agnostic. You implement one backend trait for your inference
engine, and the agent handles the orchestration above it:

- **`LlmBackend`** for a local engine (llama.cpp, MLX, ONNX), which is handed a
  prompt with the chat template already applied.
- **`ChatBackend`** for a hosted chat API, which is handed the system prompt and
  the messages and applies the model's own template. See
  [Hosted chat APIs](#hosted-chat-apis-chatbackend).

## The trait

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
        prompt: &str,             // Fully formatted (chat template applied)
        params: &InferenceParams, // max_tokens, temperature, context_size
        abort: Arc<AtomicBool>,   // Check this each token to support cancellation
        on_token: TokenCallback,  // Call with (token_text, count, tokens_per_sec)
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

## The three methods

- **`generate`** - runs inference. The `prompt` is already fully formatted (the
  chat template and any tool schemas have been applied). Sample tokens, calling
  `on_token(text, count, tokens_per_sec)` for each one so they stream upward as
  `MessageDelta` events. Check `abort` each token to support cancellation.
  Return a `GenerationResult` with the final stats.
- **`tokenize_count`** - counts tokens in a string *without* running inference.
  The [context pipeline](../context/) uses it to manage the token budget.
- **`is_ready`** - reports whether a model is loaded and ready.

## No async required

The backend runs on a blocking thread, so `generate` is a plain synchronous
function - feed the prompt, loop over sampled tokens, return. Orion owns
all the async orchestration, so you never write `async` in your backend.

## Hosted chat APIs: `ChatBackend`

*Feature `chat-backend`, on by default.* A hosted API takes a list of messages
and applies the model's own chat template, so handing it Orion's formatted
prompt either templates twice or collapses every turn into one user message.
Implement `ChatBackend` instead and the agent skips its own templating for it.
Pruning, the tool loop and the event stream are unchanged.

```rust
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use async_trait::async_trait;
use orion_core::{ChatBackend, CoreResult, GenerationResult, InferenceParams, Message, TokenCallback};

struct HostedApi; // your HTTP client

#[async_trait]
impl ChatBackend for HostedApi {
    async fn chat(
        &self,
        system: &str,          // System prompt, tool instructions folded in
        messages: &[Message],  // The turns that survived pruning, no system message
        params: &InferenceParams,
        abort: Arc<AtomicBool>, // Check between chunks to support cancellation
        mut on_token: TokenCallback,
    ) -> CoreResult<GenerationResult> {
        // Send `system` + `messages` to the API, call on_token per chunk,
        // and return the usage the response reports.
        todo!()
    }
}

let backend: Arc<dyn ChatBackend> = Arc::new(HostedApi);
let reply = agent.send("Hello", backend).await?;
```

`chat` is async, since the work is I/O rather than compute. `tokenize_count`
and `is_ready` have defaults: a four-characters-a-token estimate (a hosted API
has no cheap tokenizer; real counts come back in the `GenerationResult`) and
`true`.

`agent.prompt`, `agent.resume` and `agent.send` all take either kind: an
`Arc<dyn LlmBackend>` and an `Arc<dyn ChatBackend>` both convert into the
`Backend` enum.

## GenerationResult

`generate` returns the final stats for the turn:

```rust
GenerationResult {
    text,                    // the full generated text
    tokens_generated,        // number of tokens produced
    prompt_tokens,           // tokens in the formatted prompt
    tokens_per_sec,          // throughput
    time_to_first_token_ms,  // latency to the first token
    generation_time_ms,      // total generation time
}
```

## Ready-made: the OpenAI-compatible HTTP backend

Don't want to implement the trait yourself? Enable the `http-backend` feature
for `backends::OpenAiHttpBackend`, a supported streaming client for any
OpenAI-compatible server - OpenAI, llama.cpp's `llama-server`, vLLM, LM Studio,
and Ollama all speak the protocol, so one implementation targets any of them by
changing the base URL.

```rust
use std::sync::Arc;
use orion_core::backends::{OpenAiConfig, OpenAiEndpoint, OpenAiHttpBackend};
use orion_core::LlmBackend;

// Local server (no key); add `.with_api_key("sk-...")` for a cloud endpoint.
let config = OpenAiConfig::new("http://localhost:8080/v1", "local-model");
let backend: Arc<dyn LlmBackend> = Arc::new(OpenAiHttpBackend::new(config).unwrap());
// agent.prompt("Hello", backend, tx).await?;
```

Tokens stream through `on_token` as they arrive, and the returned
`GenerationResult` carries the **real** token counts from the response's
`usage` block (not an estimate). The abort flag is honored between chunks.
Because `generate` is synchronous, the backend blocks on I/O - the agent loop
already drives it on a blocking thread, so no extra work is needed.

### Choosing the endpoint

`OpenAiHttpBackend` implements both traits, and which one you hand the agent
decides what goes over the wire:

- **As a `ChatBackend`** (`Arc<dyn ChatBackend>`) it posts a real message list,
  system prompt included, to `/v1/chat/completions`, and the server applies the
  model's own template. Use this against hosted APIs and chat servers.
  `with_endpoint` is not consulted on this path.
- **As an `LlmBackend`** (`Arc<dyn LlmBackend>`) it sends Orion's formatted
  prompt, chosen with `OpenAiConfig::with_endpoint(...)`.
  `OpenAiEndpoint::Completions` posts it **verbatim** to `/v1/completions`, the
  right choice for a local instruct model on a completion endpoint.
  `OpenAiEndpoint::Chat` (the default) posts it to `/v1/chat/completions` as a
  single user message, which templates twice; prefer the `ChatBackend` path
  there.

### Distinguishing failures

Transport failures - a refused connection, DNS failure, timeout, or a dropped
connection mid-stream - surface as
[`CoreError::BackendUnreachable`](../errors/): no response arrived, so the
request may be safe to retry or fail over. An endpoint that answers with an
error status surfaces as `CoreError::Backend` instead. Your host decides the
retry/failover policy; the backend just reports which happened.

The [`openai_backend` example](../../reference/examples/) drives this backend
end to end and is the best reference for wiring it up.
