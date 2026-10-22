---
title: Plugins
description: Install MCP servers and skills from Claude Code plugin marketplaces.
---

Feature `plugins` (off by default; implies `skills`, adds an async HTTP client,
and runs `git`).

A marketplace publishes a catalog in the Claude Code `marketplace.json` format,
such as [AgentHub](https://agenthub.nullorder.org). Each entry points at a git
repository holding the plugin. Two of the things a plugin can ship work in any
agent, not only Claude Code: **MCP servers** and **skills**.

```rust
use orion_core::plugins::{Catalog, PluginStore};

let catalog = Catalog::fetch("agenthub", "https://agenthub.nullorder.org/plugins.json").await?;
let entry = catalog.get("superpowers-chrome").expect("listed");

let store = PluginStore::new(PluginStore::default_root().expect("home directory"));
let prepared = store.prepare(&catalog.name, entry)?; // blocking
for (name, server) in &prepared.contents.mcp_servers {
    println!("MCP server {name}: {:?} {:?}", server.command, server.args);
}
let installed = store.commit(prepared)?;
```

## Catalogs

`Catalog::parse` takes a `marketplace.json` object or a bare array of entries,
and `Catalog::fetch` downloads one. Entries keep every field the format
defines, and every source shape: `github`, `url` and `git-subdir` (each with an
optional `ref` or `sha`), `npm`, and a path inside the marketplace. An entry
that does not parse is skipped, so one bad entry does not hide the rest.

`Catalog::search` matches every word of a query against names, descriptions,
tags, categories and authors. `PluginEntry::components` reads the component
types from the tags, and `is_portable` says whether a plugin ships anything an
agent outside Claude Code can use.

## The store

`PluginStore` installs plugins into one directory, `~/.agents/plugins` by
default, which any tool can read:

```text
installed.json               every installed plugin, with its pinned commit
<marketplace>/<plugin>/      its git checkout
catalogs/<marketplace>.json  the last fetched catalog
```

Installs take two steps. `prepare` fetches the plugin into a staging area (a
shallow checkout, sparse for a plugin in a subfolder) at the entry's `sha`, or
the tip of its `ref` or default branch, and reads what it ships. Nothing from
the plugin runs, `git` never asks for credentials, and only `https://` (and
`file://`) repositories are fetched. Show the user what the plugin's MCP
servers run, then `commit` to move it into place, or `cancel`.

`latest_commit` reports the commit an install would fetch now; compare it with
`InstalledPlugin::commit` to offer updates. `remove` uninstalls. Store
operations block, so run them under `spawn_blocking` in async code.

## Contents

`PluginContents::read` reads a plugin folder:

- **MCP servers** from `plugin.json` `mcpServers` (inline, or a path to a
  file), else a root `.mcp.json`, plus any the catalog entry declares. Files
  may wrap the map in `mcpServers` or not. `${CLAUDE_PLUGIN_ROOT}` is filled in
  and passed to stdio servers as an environment variable, as Claude Code does.
- **Skills** through [`discover_skills`](../skills/).
- **Claude Code-only parts** (commands, agents, hooks, language servers), so you
  can say what is left out.

`required_programs` lists what the stdio servers run (`npx`, `node`, `uvx`, …)
so you can check they are installed first.
