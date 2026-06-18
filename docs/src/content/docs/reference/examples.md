---
title: Examples
description: The runnable examples shipped in the repository.
---

The repository ships two runnable examples that exercise the full agent loop.

## `mock_backend`

A complete, self-contained backend that streams a canned reply — no model or
network needed. It's the fastest way to see the event stream and tool loop run
end to end.

```sh
cargo run --example mock_backend
```

Start here to understand the shape of an [`LlmBackend`](../../concepts/backend/)
implementation and how [events](../../concepts/events/) flow.

## `openai_backend`

A real, over-the-wire backend that implements `LlmBackend` against a streaming
OpenAI-compatible `/v1/completions` endpoint. The same code works against:

- OpenAI
- llama.cpp's `server`
- vLLM
- LM Studio
- Ollama

```sh
cargo run --example openai_backend --features openai-example
```

It lives behind the `openai-example` feature so its HTTP dependencies don't
weigh on the core crate. This is the best reference for a production backend.

:::note
Optional example features like `openai-example` may require a newer toolchain
than the crate's MSRV (Rust 1.85), which covers only the default feature set.
:::

## Browse the source

Both examples live in the
[`examples/` directory](https://github.com/anistark/orion-core/tree/main/examples)
on GitHub.
