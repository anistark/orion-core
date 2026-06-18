---
title: Backend
description: Implement the LlmBackend trait to plug in any inference engine.
---

Orion is backend-agnostic. You implement one trait — `LlmBackend` — for
your inference engine, and the agent handles the orchestration above it.

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

- **`generate`** — runs inference. The `prompt` is already fully formatted (the
  chat template and any tool schemas have been applied). Sample tokens, calling
  `on_token(text, count, tokens_per_sec)` for each one so they stream upward as
  `MessageDelta` events. Check `abort` each token to support cancellation.
  Return a `GenerationResult` with the final stats.
- **`tokenize_count`** — counts tokens in a string *without* running inference.
  The [context pipeline](../context/) uses it to manage the token budget.
- **`is_ready`** — reports whether a model is loaded and ready.

## No async required

The backend runs on a blocking thread, so `generate` is a plain synchronous
function — feed the prompt, loop over sampled tokens, return. Orion owns
all the async orchestration, so you never write `async` in your backend.

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

## A real backend over the wire

The repository's [`openai_backend` example](../../reference/examples/)
implements `LlmBackend` against a streaming OpenAI-compatible
`/v1/completions` endpoint — which covers OpenAI, llama.cpp's `server`, vLLM,
LM Studio, and Ollama. It's the best reference for a production backend.
