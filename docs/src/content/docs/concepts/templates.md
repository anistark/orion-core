---
title: Templates
description: Chat prompt formats for the supported model families.
---

Each model family wants its prompt wrapped a certain way. Orion ships a
`ChatTemplate` implementation for the common ones and picks the right one
automatically.

## Supported families

ChatML (default), Llama 3, Llama 2, Mistral / Mixtral, Gemma / Gemma 2, Phi-3,
DeepSeek (LLM chat), Command-R / Command-R+, Alpaca, and Vicuna.

## Selecting a template

- **`detect_template(gguf_template)`** - inspects a GGUF metadata template
  string and returns the matching implementation, falling back to ChatML when
  nothing matches.
- **`template_from_name(name)`** - resolves a manual-override name (with common
  aliases such as `llama-2`, `phi-3`, `cohere`) to a template, or `None` for an
  unimplemented family so the caller can fall back to auto-detection.
- **`Agent::with_template(config, template)`** / **`agent.set_template(template)`**
  - set or swap the template at runtime.

```rust
use orion_core::{Agent, AgentConfig, Llama3Template, template_from_name};

// Construct with an explicit template…
let mut agent = Agent::with_template(AgentConfig::default(), Box::new(Llama3Template));

// …or swap it later, e.g. from a user-supplied override name.
if let Some(template) = template_from_name("mistral") {
    agent.set_template(template);
}
```

## Token-accurate formatting

Every template also implements the per-message and per-system formatting hooks
the [context pipeline](../context/) needs for accurate token-budget accounting,
and advertises tools through the same `tool_call` convention used by the
[tools](../tools/) module. That keeps budget math and tool-calling consistent
no matter which family you target.
