---
title: Messages
description: The conversation data types — roles, constructors, and tool records.
---

A conversation is a `Vec<Message>`. Messages support five roles that cover the
full agent lifecycle.

## Constructors

```rust
use orion_core::Message;

// Standard conversation
let sys  = Message::system("msg-1", "You are helpful.");
let user = Message::user("msg-2", "Hello!");
let asst = Message::assistant("msg-3", "Hi there!");

// Tool interaction
let result = Message::tool_result(
    "msg-4",            // message id
    "call-1",           // tool_call_id (links to the assistant's request)
    "read_file",        // tool name
    "file contents...", // result content
    false,              // is_error
);
```

## Roles

`System`, `User`, `Assistant`, `ToolCall`, and `ToolResult`. Assistant messages
can carry `tool_calls`; tool-result messages carry a `tool_result` that links
back to the originating call via `tool_call_id`.

## Fields

Every message has:

- an **`id`**,
- a **`timestamp`**, and
- an optional **`token_count`** (populated after tokenization).

## Pinning

Any message with `pinned == true` always survives pruning, regardless of budget
or strategy:

```rust
let pinned = Message::user("msg-5", "Remember: the project is called Orion.").pinned();
// or toggle an existing message on the agent:
agent.set_pinned("msg-5", true);
```

Pruning is turn-aware, so a pinned message keeps its whole turn — no orphaned
call/result pairs. See [Context & budgets](../context/) for how pruning works.

## Persisting conversations

Because the conversation is a plain `Vec<Message>`, you can serialize it, store
it, and restore it later with `agent.replace_messages(saved_messages)`.
