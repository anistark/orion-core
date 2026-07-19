---
title: Errors
description: The CoreError type, what each variant means, and how to handle it.
---

Orion uses one error type, `CoreError`, and a `CoreResult<T>` alias
(`Result<T, CoreError>`) throughout the API.

```rust
use orion_core::{CoreError, CoreResult};

CoreError::Backend("No model loaded".into())       // Backend failed, or a reachable
                                                   //   endpoint returned an error status
CoreError::BackendUnreachable("refused".into())    // Endpoint unreachable - no response
CoreError::Context("Token limit exceeded".into())  // Prompt won't fit the token budget
CoreError::Tool("File not found".into())           // A tool failed or wasn't found
CoreError::Agent("Empty message".into())           // Invalid request or internal failure
CoreError::Aborted                                 // Cancelled via agent.abort()
```

| Variant | Carries | Retryable as-is? |
|---------|---------|------------------|
| `Backend` | message | No - fix the model/request first |
| `BackendUnreachable` | message | Yes - no response arrived |
| `Context` | message | No - shrink the prompt or grow the budget |
| `Tool` | message | Depends on the tool |
| `Agent` | message | No - fix the call |
| `Aborted` | - | Expected; not a failure |

## How errors reach you

Where an error shows up depends on how you call the agent. This matters more
than the variants themselves:

- **`agent.prompt(...)` / `prompt_stream(...)`** returns `Err` **only** for an
  invalid request - today that's an empty prompt (`CoreError::Agent`). Anything
  that goes wrong *during* the run (backend not ready, context overflow, an
  endpoint failure) is delivered as an [`AgentEvent::Error`](../events/) on the
  stream, and the call still returns `Ok(())` after emitting `AgentEnd`. So a UI
  driving `prompt` watches the event stream for `Error`, not the return value.
- **Cancellation** via `agent.abort()` is *not* surfaced as an error at all: the
  run records an empty assistant turn, emits `AgentEnd`, and returns `Ok(())`.
  You only ever see `CoreError::Aborted` when you call a backend's `generate`
  directly.
- **Tool failures and denials never stop the run.** A tool that errors, or a
  call an [approval hook](../tools/#gating-tool-calls) denies, is appended to
  the conversation as an *error tool result* (with a `ToolExecEnd`/`ToolDenied`
  event) so the model can adapt. You observe these in the conversation, not as a
  thrown `CoreError`.
- **Calling a backend's `generate(...)` directly** (custom orchestration) hands
  you every `CoreError` as a returned `Err` - including `Aborted`,
  `BackendUnreachable`, and `Backend`.

## The variants

### `CoreError::Backend`

**The inference backend failed, or a reachable endpoint returned an error
status.** The reachable-but-errored case is the key contrast with
[`BackendUnreachable`](#coreerrorbackendunreachable) below - here a response
*did* arrive, it just wasn't a success.

Occurs when:

- No model is loaded - `is_ready()` returned `false`, so the agent reports
  `"No model loaded"` before generating.
- An OpenAI-compatible endpoint answered with a non-2xx status, e.g.
  `"endpoint returned HTTP 401: invalid api key"` or `HTTP 500`.
- The HTTP backend's client couldn't be built (`OpenAiHttpBackend::new`).

How to fix:

- Load a model and confirm `is_ready()` is `true` before prompting.
- Read the message - it carries the HTTP status and response body. `401`/`403`
  means a missing or wrong `api_key`; `404` usually means a wrong `base_url` or
  `model`; `400` means a malformed request; `5xx` is a server-side fault.
- Because a real response came back, retrying the identical request rarely helps
  until you change the model, credentials, or request.

### `CoreError::BackendUnreachable`

**The backend endpoint could not be reached - no response arrived.** Distinct
from `Backend` precisely so a host can safely retry or fail over.

Occurs when (HTTP backend):

- The request never connected - connection refused, DNS failure, TLS handshake
  failure, or a timeout - reported as `"request to <url> failed: …"`.
- The connection dropped mid-stream while reading tokens -
  `"stream read failed: …"`.

How to fix:

- Verify the server is running and reachable at `base_url`; check the port,
  scheme (`http` vs `https`), firewall, and any proxy.
- Raise `OpenAiConfig::with_timeout(...)` if a slow server is timing out.
- Safe to retry or fail over to another endpoint - no response was received.
  Note that a mid-stream drop may mean the model already generated (and possibly
  billed) some tokens, even though you got nothing usable back.

See the [HTTP backend](../backend/#distinguishing-failures) for how this split
drives retry/failover policy.

### `CoreError::Context`

**The conversation can't be formatted within the token budget.** The budget is
`max_context_tokens − max_response_tokens`; the message says which part
overflowed.

Occurs when:

- The **system prompt plus tool schemas** alone exceed the budget -
  `"System prompt and tools (N tokens) exceed available context budget (M
  tokens)"`. Nothing fits around them.
- The **latest message** (plus that fixed overhead) doesn't fit -
  `"Latest message (N tokens) … exceeds context budget"`. Older turns are pruned
  automatically, but the newest turn must fit.
- **Pinned messages** together exceed the budget -
  `"Pinned messages (N tokens) exceed the available context budget"`. Pins never
  get pruned, so too many pins can starve the budget.

How to fix:

- Raise `max_context_tokens` (or lower `max_response_tokens`) in
  [`ContextConfig`](../context/) to widen the budget.
- Trim the system prompt or reduce the number/size of tool schemas.
- Shorten the offending user message, or split it into smaller turns.
- Clear or compress history: `agent.clear()`, or switch to the `Summarize`
  [prune strategy](../context/) so old turns fold into a summary instead of
  overflowing.
- Unpin messages with `agent.set_pinned(id, false)` if pins are the cause.

:::note
During a `prompt()` run this arrives as an `AgentEvent::Error`, not a returned
`Err`. The agent drops the just-added user message so a retry with a smaller
budget starts clean.
:::

### `CoreError::Tool`

**A tool couldn't run, or the model named a tool that isn't registered.**

Occurs when:

- The model calls a tool name that has no match - `"unknown tool: <name>"`.
- A tool's own `execute` returned `Err(CoreError::Tool(...))`.

How to fix:

- Register every tool the model might call with `agent.set_tools(...)`, and give
  each a precise `description` so the model uses the right name and arguments.
- Inside `execute`, map failures to a clear `CoreError::Tool(...)` - the text
  becomes what the model sees and reasons about.

:::tip
A tool error almost never surfaces as a returned `CoreError`. The agent feeds it
back to the model as an *error tool result* (`is_error: true`) and continues the
loop, so the model can retry, pick a different tool, or explain the failure. Look
for it in the conversation and in `ToolExecEnd { result }`, not in a `Result`.
:::

### `CoreError::Agent`

**The request itself was invalid, or an internal task failed.** This is the one
variant `prompt` can return directly.

Occurs when:

- The prompt is empty or whitespace-only - `"Empty message"`, returned straight
  from `prompt`.
- The blocking inference task panicked - `"Inference task failed: …"`. This
  points to a bug in the backend's `generate` or tokenizer, not in your call.

How to fix:

- Guard against empty input before calling `prompt`.
- For a panic, check the backend's logs; a well-behaved backend should return a
  `CoreError` rather than panicking.

### `CoreError::Aborted`

**Generation was cancelled via the abort flag** - the expected outcome of
`agent.abort()`, not a failure to handle defensively.

Occurs when:

- You called `agent.abort()` (or flipped the shared `AtomicBool` from
  `agent.abort_flag()`), and the backend observed it between tokens/chunks and
  stopped.

What to expect:

- Through `prompt`, you generally **won't** see this error - the run ends
  cleanly with an empty assistant turn and an `AgentEnd` event.
- Calling `generate` directly returns `Err(CoreError::Aborted)`. Treat it as
  "stopped on request" and move on; there's nothing to fix.

## Serializable

All errors implement `Serialize`, so you can transport them over IPC - handy
when the agent runs in a separate process from your UI and you want to forward
failures across the boundary. Each serializes to its display string.

## Match with a wildcard arm

`CoreError` is `#[non_exhaustive]`. Always include a `_ =>` arm so new variants
in a future minor release don't break your build:

```rust
match err {
    CoreError::Aborted => { /* user cancelled - expected */ }
    CoreError::BackendUnreachable(msg) => { /* retry or fail over */ eprintln!("unreachable: {msg}") }
    CoreError::Backend(msg) => eprintln!("backend: {msg}"),
    _ => eprintln!("other: {err}"),
}
```

See the [stability policy](https://github.com/anistark/orion-core/blob/main/CONTRIBUTING.md#stability--versioning)
for the full `#[non_exhaustive]` and SemVer guarantees.
