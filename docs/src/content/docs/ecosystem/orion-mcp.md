---
title: Orion MCP
description: Give an agent the tools of any MCP server with the orion-mcp crate.
---

[Orion MCP](https://github.com/anistark/orion-mcp), published as the `orion-mcp` crate, gives an agent the tools of any [Model Context Protocol](https://modelcontextprotocol.io) server. It connects over Streamable HTTP, lists the server's tools, and offers each one as a [`Tool`](../../concepts/tools/). The agent calls them like its own: each call goes to the server as `tools/call`, and its answer comes back to the model.

It is a separate crate, built on [`rmcp`](https://crates.io/crates/rmcp), the official Rust SDK. That keeps `orion-core` free of a network client and lets MCP support move at its own pace. It uses only the public `orion-core` API.

## Install

```toml
[dependencies]
orion-core = "0.9"
orion-mcp = "0.1"
```

`orion-mcp` re-exports `orion_core`. Use that re-export, or the same `orion-core` version it depends on, so its tools fit your agent.

## Connect and hand over the tools

```rust
use orion_core::Tool;
use orion_mcp::McpServer;

let docs = McpServer::builder("https://mcp.example.com/mcp")
    .prefix("docs")
    .bearer_token(token)
    .connect()
    .await?;

let mut tools: Vec<Box<dyn Tool>> = vec![Box::new(MyTool)];
tools.extend(docs.tools().await?);
agent.set_tools(tools);
```

`McpServer::connect(url)` is the short form with every default. `tools()` sends `tools/list` and returns the tools ready for `set_tools`. Call it again when the server's tools change.

The server is cheap to clone. The connection closes once the server and every tool taken from it are dropped.

## Options

| Builder method | What it does | Default |
| --- | --- | --- |
| `prefix(name)` | Offers each tool as `{name}__{tool}` | No prefix |
| `bearer_token(token)` | Sends `Authorization: Bearer {token}` | None |
| `header(name, value)` | Sends a header with every request, such as an API key | None |
| `connect_timeout(duration)` | Limits connecting and listing tools | 10 seconds |
| `call_timeout(duration)` | Limits each tool call | 60 seconds |

Tokens and header values come from the host. They are kept only as long as the connection and never appear in errors or `Debug` output.

## Names and prefixes

Each tool keeps the server's name, description and input schema, so the model sees what the server wrote. Its label is the tool's title, if the server gives one.

Two servers, or a server and your own tools, can offer the same name. A prefix keeps them apart: with `.prefix("docs")`, the server's `search` becomes `docs__search`. The separator is `orion_mcp::PREFIX_SEPARATOR`.

## Results

- **Text** comes through as it is.
- **Images, audio and binary resources** are noted by type, such as `[image: image/png]`.
- **Embedded text resources** come through as their text.
- **Structured content** goes to the result's `details` for your UI. When the server sends no text, the model reads the structured content as JSON.

## Errors and stopping

Every failure is a `CoreError::Tool`, which the agent hands back to the model as an error result:

- the server cannot be reached, or refuses the MCP handshake
- the server answers a call with an error result or a protocol error
- no answer comes within the connect or call timeout
- the arguments are not a JSON object

Nothing panics or hangs. When the agent's run is stopped, a running call stops with `CoreError::Aborted` and the server is told the request was cancelled.

## Servers from plugins

[Plugins](../../concepts/plugins/) declare MCP servers but never run them. To use a plugin's HTTP servers, connect to the ones in its `PluginContents` that have a `url`:

```rust
let mut tools: Vec<Box<dyn Tool>> = Vec::new();
for (name, server) in &contents.mcp_servers {
    let Some(url) = &server.url else { continue }; // a stdio server
    let mut builder = McpServer::builder(url).prefix(name);
    for (key, value) in &server.headers {
        builder = builder.header(key, value);
    }
    tools.extend(builder.connect().await?.tools().await?);
}
```

Header values in a plugin's files may hold placeholders such as `${API_KEY}`. Fill them in from your own settings before connecting.

:::caution
A server's tool names, descriptions and results reach the model as written. Treat every server as untrusted input, and gate its tools with an [approval hook](../../concepts/tools/#gating-tool-calls) where it matters.
:::

## Not supported yet

- stdio servers (a local process) and servers that only speak the older SSE transport
- OAuth sign-in (pass a token you already have with `bearer_token`)
- sampling, elicitation and progress events
