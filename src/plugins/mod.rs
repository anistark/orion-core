//! Plugin marketplaces: find plugins, install them, and use what they ship.
//!
//! A marketplace publishes a catalog in the Claude Code `marketplace.json`
//! format, such as [AgentHub](https://agenthub.nullorder.org). Each entry
//! points at a git repository holding the plugin. Two of the things a plugin
//! can ship work in any agent, not only Claude Code: **MCP servers** and
//! **skills**. This module fetches catalogs (`Catalog`), installs plugins
//! into a shared store (`PluginStore`, `~/.agents/plugins` by default) and
//! reads their MCP servers and skills (`PluginContents`).
//!
//! ```no_run
//! # async fn demo() -> orion_core::CoreResult<()> {
//! use orion_core::plugins::{Catalog, PluginStore};
//!
//! let catalog = Catalog::fetch("agenthub", "https://agenthub.nullorder.org/plugins.json").await?;
//! let entry = catalog.get("superpowers-chrome").expect("listed");
//!
//! let store = PluginStore::new(PluginStore::default_root().expect("home directory"));
//! // Blocking: run under `spawn_blocking` in real async code.
//! let prepared = store.prepare(&catalog.name, entry)?;
//! for (name, server) in &prepared.contents.mcp_servers {
//!     println!("MCP server {name}: {:?} {:?}", server.command, server.args);
//! }
//! let installed = store.commit(prepared)?;
//! println!("installed {} at {}", installed.name(), installed.commit);
//! # Ok(()) }
//! ```
//!
//! Installing runs nothing from the plugin. Its MCP servers run only when the
//! caller starts them, so show the user what they run before you do.

mod catalog;
mod contents;
mod store;

pub use catalog::{Author, Catalog, Component, GitSource, PluginEntry, PluginSource, RemoteSource};
pub use contents::{McpServerSpec, PluginContents, PLUGIN_ROOT_VAR};
pub use store::{InstalledPlugin, PluginStore, PreparedInstall};
