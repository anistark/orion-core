---
title: Architecture
description: How the agent, context pipeline, backend, and event stream fit together.
---

Orion sits between your application and your inference engine. You own the
top (UI/app) and the bottom (the model runtime); Orion owns the middle —
the orchestration that turns a prompt into a streamed, tool-augmented answer.

```text
┌──────────────────────────────────────────────────┐
│  Your application (CLI, server, desktop app, …)   │
├──────────────────────────────────────────────────┤
│  Agent                                            │
│  ├── prompt("Hello")                              │
│  ├── Conversation state (Vec<Message>)            │
│  ├── System prompt                                │
│  ├── Registered tools                             │
│  └── AgentConfig (inference params, context cfg)  │
├──────────────────────────────────────────────────┤
│  Context pipeline                                 │
│  └── prepare_context() — prune + template format  │
├──────────────────────────────────────────────────┤
│  LlmBackend (trait) ← you implement this          │
│  ├── generate() — run inference, stream tokens    │
│  ├── tokenize_count() — count tokens              │
│  └── is_ready() — check model status              │
├──────────────────────────────────────────────────┤
│  Your inference engine                            │
│  (llama.cpp, MLX, ONNX, cloud API, etc.)          │
└──────────────────────────────────────────────────┘
```

## The loop

A single call to `Agent::prompt` drives the whole cycle:

1. **Append** the user message to the conversation.
2. **Prepare context** — the pipeline prunes old messages to fit the token
   budget and formats the survivors into a prompt string using the active chat
   template (injecting tool schemas if any tools are registered).
3. **Generate** — the backend streams tokens, which surface as `MessageDelta`
   events in real time.
4. **Tool loop** — if the model emitted tool calls, the agent runs the matching
   tools, appends their results, and loops back to step 2. This repeats until
   the model returns a tool-free answer (bounded by
   `AgentConfig::max_tool_iterations`, default 8).
5. **Finish** — the final assistant message lands and the call returns.

## Events flow upward

Every meaningful step emits an [`AgentEvent`](../events/) through an unbounded
channel (`tokio::sync::mpsc`). Your UI or application layer subscribes and
reacts in real time — streaming tokens to the screen, showing tool progress, or
rendering the live context-budget gauge. The agent never touches your UI
directly; it only emits events.

## Backend-agnostic by design

Orion knows nothing about your model runtime. It calls three methods on
the [`LlmBackend`](../backend/) trait and orchestrates everything else. That
keeps the harness identical whether you run a local GGUF model through
llama.cpp, an MLX model on Apple silicon, or a remote OpenAI-compatible
endpoint.
