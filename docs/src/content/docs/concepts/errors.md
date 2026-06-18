---
title: Errors
description: The CoreError type and its variants.
---

orion-core uses one error type, `CoreError`, and a `CoreResult<T>` alias
(`Result<T, CoreError>`) throughout the API.

## Variants

```rust
use orion_core::{CoreError, CoreResult};

CoreError::Backend("No model loaded".into())      // LLM backend issues
CoreError::Context("Token limit exceeded".into()) // Context pipeline issues
CoreError::Tool("File not found".into())          // Tool execution issues
CoreError::Agent("Empty message".into())          // Agent logic issues
CoreError::Aborted                                // User cancelled (see agent.abort())
```

## Serializable

All errors implement `Serialize`, so you can transport them over IPC — handy
when the agent runs in a separate process from your UI and you want to forward
failures across the boundary.

## Match with a wildcard arm

`CoreError` is `#[non_exhaustive]`. Always include a `_ =>` arm so new variants
in a future minor release don't break your build:

```rust
match err {
    CoreError::Aborted => { /* user cancelled */ }
    CoreError::Backend(msg) => eprintln!("backend: {msg}"),
    _ => eprintln!("other: {err}"),
}
```

See the [stability policy](https://github.com/anistark/orion-core/blob/main/CONTRIBUTING.md#stability--versioning)
for the full `#[non_exhaustive]` and SemVer guarantees.
