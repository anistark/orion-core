---
title: Tools
description: Define tools the model can call, and let the agent run the loop.
---

`Agent::prompt` drives the full tool cycle automatically: it injects your tool
schemas into the system prompt, parses the model's tool calls out of its reply,
runs the matching tool, appends the result to the conversation, and loops back
to the model until it returns a tool-free answer (bounded by
`AgentConfig::max_tool_iterations`, default 8). Each step emits
`ToolExecStart` / `ToolExecUpdate` / `ToolExecEnd` [events](../events/).

## Defining a tool

Each tool has a name, label, description, a JSON Schema for its parameters, and
an async `execute` function.

```rust
use orion_core::{Tool, ToolOutput, CoreResult, CoreError};
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
            .map_err(|e| CoreError::Tool(e.to_string()))?;
        Ok(ToolOutput {
            content,
            details: serde_json::json!({ "path": path }),
        })
    }
}

// Register with the agent.
agent.set_tools(vec![Box::new(ReadFileTool)]);
```

Tool schemas are injected into the system prompt automatically when the context
is formatted, so you only register tools - the agent handles the rest.

## The tool-call convention

Templates advertise - and `parse_tool_calls` reads - a fenced JSON block:

````text
```tool_call
{"name": "read_file", "arguments": {"path": "Cargo.toml"}}
```
````

A JSON **array** invokes several tools in one turn. Parsing is lenient: a
` ```json ` fence, or a whole-message bare JSON object carrying both `name` and
`arguments`, also counts - so smaller models still trigger tools when they drift
from the exact format. With no tools registered, parsing is skipped entirely and
replies pass through verbatim.

## Gating tool calls

By default the agent runs each tool the moment the model calls it. Install an
`ApprovalHook` to authorize calls first - for sandboxing, permission tiers, or a
human "the model wants to run `delete_file` - allow?" confirmation. The hook is
consulted once per parsed call, after parsing and **before** execution:

```rust
use std::sync::Arc;
use orion_core::{ApprovalDecision, ApprovalHook, ToolCall};
use async_trait::async_trait;

struct ConfirmDestructive;

#[async_trait]
impl ApprovalHook for ConfirmDestructive {
    async fn review(&self, call: &ToolCall) -> ApprovalDecision {
        if call.name.starts_with("delete_") {
            ApprovalDecision::Deny {
                reason: format!("{} needs confirmation before it can run", call.name),
            }
        } else {
            ApprovalDecision::Approve
        }
    }
}

agent.set_approval_hook(Arc::new(ConfirmDestructive));
```

The hook is `async` and may block for as long as it needs - including awaiting a
human decision - so it's never wrapped in an internal timeout. A `Deny` does
**not** abort the run: the tool is skipped, the `reason` is appended as an error
tool result (so the model sees the refusal and can adapt), and a
[`ToolDenied` event](../events/) fires - distinct from an execution failure. The
loop then continues. With no hook installed, the tool loop behaves exactly as
before.

## Opting out of the `tools` feature

The `Tool` trait and the execution loop live behind the default `tools`
feature, which pulls in [`async-trait`](https://crates.io/crates/async-trait).
Minimal consumers that only need plain chat can drop it:

```toml
orion-core = { version = "0.2", default-features = false }
```

Tool-call *parsing* (`parse_tool_calls`, `ParsedToolCall`) and `ToolSchema` stay
available either way - only the `Tool` trait, `ToolOutput`,
`ToolUpdateCallback`, and `Agent::set_tools` require the feature.

:::note
The code snippets across these docs mirror compile-checked doctests on the
matching API items, so they can't silently drift from the real signatures. Run
them in the repo with `cargo test --doc`.
:::
