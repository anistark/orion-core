---
title: Examples
description: The runnable examples shipped in the repository.
---

The repository ships two runnable examples that exercise the full agent loop.

## `mock_backend`

A complete, self-contained backend that streams a canned reply - no model or
network needed. It's the fastest way to see the event stream and tool loop run
end to end.

```sh
cargo run --example mock_backend
```

Start here to understand the shape of an [`LlmBackend`](../../concepts/backend/)
implementation and how [events](../../concepts/events/) flow.

## `openai_backend`

A thin demo that drives the supported
[`OpenAiHttpBackend`](../../concepts/backend/#ready-made-the-openai-compatible-http-backend)
against a streaming OpenAI-compatible endpoint. The same code works against:

- OpenAI
- llama.cpp's `llama-server`
- vLLM
- LM Studio
- Ollama

```sh
# Chat endpoint (default):
OPENAI_BASE_URL=http://localhost:8080/v1 OPENAI_MODEL=local \
    cargo run --example openai_backend --features http-backend

# Completion endpoint (sends the already-templated prompt verbatim):
OPENAI_BASE_URL=http://localhost:8080/v1 OPENAI_MODEL=local OPENAI_ENDPOINT=completions \
    cargo run --example openai_backend --features http-backend
```

The backend lives behind the `http-backend` feature so its HTTP dependencies
don't weigh on the core crate. Set `OPENAI_API_KEY` for a cloud endpoint.

:::note
Optional features like `http-backend` may require a newer toolchain than the
crate's MSRV (Rust 1.85), which covers only the default feature set.
:::

## Browse the source

Both examples live in the
[`examples/` directory](https://github.com/anistark/orion-core/tree/main/examples)
on GitHub.
