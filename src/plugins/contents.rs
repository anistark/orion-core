//! What an installed plugin ships: its MCP servers, skills and the rest.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::catalog::{Component, PluginEntry};
use crate::skills::{discover_skills, is_plain_relative, Skill};

/// The placeholder Claude Code plugins use for their own directory.
pub const PLUGIN_ROOT_VAR: &str = "CLAUDE_PLUGIN_ROOT";

/// How to start or reach one MCP server, as written in `.mcp.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerSpec {
    /// Program to run for a stdio server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Its arguments.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Extra environment variables.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Endpoint of an HTTP server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Headers sent to an HTTP server.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Transport named in the file (`stdio`, `http`, `sse`), if any.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
}

impl McpServerSpec {
    /// Replace `${CLAUDE_PLUGIN_ROOT}` with `root` everywhere, and give a stdio
    /// server the variable too, as Claude Code does.
    fn expand(mut self, root: &str) -> Self {
        let sub = |s: &str| {
            s.replace(&format!("${{{PLUGIN_ROOT_VAR}}}"), root)
                .replace(&format!("${PLUGIN_ROOT_VAR}"), root)
        };
        self.command = self.command.as_deref().map(sub);
        self.args = self.args.iter().map(|a| sub(a)).collect();
        self.url = self.url.as_deref().map(sub);
        for v in self.env.values_mut().chain(self.headers.values_mut()) {
            *v = sub(v);
        }
        if self.command.is_some() {
            self.env
                .entry(PLUGIN_ROOT_VAR.to_string())
                .or_insert_with(|| root.to_string());
        }
        self
    }

    /// The program a stdio server runs, such as `npx` or a script path, so
    /// callers can check it is installed.
    pub fn program(&self) -> Option<&str> {
        let command = self.command.as_deref()?.trim();
        (!command.is_empty()).then_some(command)
    }
}

/// `.claude-plugin/plugin.json`, the parts that matter outside Claude Code.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Manifest {
    skills: Option<StringOrList>,
    #[serde(rename = "mcpServers")]
    mcp_servers: Option<serde_json::Value>,
    commands: Option<serde_json::Value>,
    agents: Option<serde_json::Value>,
    hooks: Option<serde_json::Value>,
    #[serde(rename = "lspServers")]
    lsp_servers: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn to_vec(&self) -> Vec<String> {
        match self {
            Self::One(s) => vec![s.clone()],
            Self::Many(v) => v.clone(),
        }
    }
}

/// What a plugin directory ships.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginContents {
    /// MCP servers by name, with `${CLAUDE_PLUGIN_ROOT}` filled in.
    pub mcp_servers: BTreeMap<String, McpServerSpec>,
    /// Its skills.
    pub skills: Vec<Skill>,
    /// Components it ships that only Claude Code uses (commands, agents, hooks,
    /// language servers).
    pub claude_code_only: Vec<Component>,
}

impl PluginContents {
    /// Read the plugin in `root`.
    ///
    /// MCP servers come from `plugin.json` `mcpServers` (inline, or a path to a
    /// file), else a root `.mcp.json`, plus any the catalog `entry` declares.
    /// Skills follow `plugin.json` `skills` (default `skills/`). Unreadable
    /// parts are skipped with a warning.
    pub fn read(root: &Path, entry: Option<&PluginEntry>) -> Self {
        Self::read_as(root, root, entry)
    }

    /// Like [`read`](Self::read), for a plugin in `root` that will live at
    /// `installed_dir`: `${CLAUDE_PLUGIN_ROOT}` and skill paths point there.
    pub fn read_as(root: &Path, installed_dir: &Path, entry: Option<&PluginEntry>) -> Self {
        let manifest = read_manifest(root);
        let root_str = installed_dir.to_string_lossy();

        let mut servers = BTreeMap::new();
        let declared = match &manifest.mcp_servers {
            Some(serde_json::Value::String(path)) => read_json(root, path),
            Some(v @ serde_json::Value::Object(_)) => Some(v.clone()),
            _ => read_json(root, ".mcp.json"),
        };
        let from_catalog = entry.and_then(|e| e.mcp_servers.clone());
        for value in declared.into_iter().chain(from_catalog) {
            for (name, spec) in parse_servers(value) {
                servers
                    .entry(name)
                    .or_insert_with(|| spec.expand(&root_str));
            }
        }

        let places = manifest
            .skills
            .as_ref()
            .map(StringOrList::to_vec)
            .unwrap_or_default();
        let places: Vec<&str> = places.iter().map(String::as_str).collect();
        let source = entry.map(|e| e.name.clone());
        let skills = discover_skills(root, &places)
            .into_iter()
            .map(|mut s| {
                if let Ok(rel) = s.dir.strip_prefix(root) {
                    s.dir = installed_dir.join(rel);
                }
                match &source {
                    Some(src) => s.with_source(src.clone()),
                    None => s,
                }
            })
            .collect();

        let mut claude_code_only = Vec::new();
        let has = |declared: &Option<serde_json::Value>, path: &str| {
            declared.is_some() || root.join(path).exists()
        };
        if has(&manifest.commands, "commands") {
            claude_code_only.push(Component::Commands);
        }
        if has(&manifest.agents, "agents") {
            claude_code_only.push(Component::Agents);
        }
        if has(&manifest.hooks, "hooks/hooks.json") {
            claude_code_only.push(Component::Hooks);
        }
        if has(&manifest.lsp_servers, ".lsp.json") {
            claude_code_only.push(Component::LspServers);
        }

        Self {
            mcp_servers: servers,
            skills,
            claude_code_only,
        }
    }

    /// Programs its stdio MCP servers run, deduplicated, in name order.
    pub fn required_programs(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .mcp_servers
            .values()
            .filter_map(McpServerSpec::program)
            .map(String::from)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Whether it ships nothing an agent outside Claude Code can use.
    pub fn is_empty(&self) -> bool {
        self.mcp_servers.is_empty() && self.skills.is_empty()
    }
}

fn read_manifest(root: &Path) -> Manifest {
    match read_json(root, ".claude-plugin/plugin.json") {
        Some(v) => serde_json::from_value(v).unwrap_or_else(|e| {
            log::warn!("{}: plugin.json is not valid: {e}", root.display());
            Manifest::default()
        }),
        None => Manifest::default(),
    }
}

/// Read a JSON file at `relative` inside `root`, if it is there and valid.
fn read_json(root: &Path, relative: &str) -> Option<serde_json::Value> {
    let rel = Path::new(relative);
    if !is_plain_relative(rel) {
        log::warn!("ignoring '{relative}' outside {}", root.display());
        return None;
    }
    let path = root.join(rel);
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text)
        .map_err(|e| log::warn!("{} is not valid JSON: {e}", path.display()))
        .ok()
}

/// Servers from a `.mcp.json`-style value: `{ "mcpServers": { … } }` or the
/// map on its own.
fn parse_servers(value: serde_json::Value) -> Vec<(String, McpServerSpec)> {
    let map = match value {
        serde_json::Value::Object(mut obj) => match obj.remove("mcpServers") {
            Some(serde_json::Value::Object(inner)) => inner,
            _ => obj,
        },
        _ => return Vec::new(),
    };
    map.into_iter()
        .filter_map(
            |(name, v)| match serde_json::from_value::<McpServerSpec>(v) {
                Ok(spec) if spec.command.is_some() || spec.url.is_some() => Some((name, spec)),
                Ok(_) => {
                    log::warn!("skipping MCP server '{name}': no command or url");
                    None
                }
                Err(e) => {
                    log::warn!("skipping MCP server '{name}': {e}");
                    None
                }
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_wrapped_and_flat_server_maps() {
        let wrapped = parse_servers(json!({ "mcpServers": { "a": { "command": "node" } } }));
        assert_eq!(wrapped.len(), 1);
        let flat = parse_servers(json!({ "b": { "type": "http", "url": "https://x" } }));
        assert_eq!(flat[0].0, "b");
        assert_eq!(flat[0].1.transport.as_deref(), Some("http"));
        assert!(parse_servers(json!({ "c": { "args": [] } })).is_empty());
    }

    #[test]
    fn expands_plugin_root() {
        let spec = McpServerSpec {
            command: Some("${CLAUDE_PLUGIN_ROOT}/run.sh".into()),
            args: vec!["$CLAUDE_PLUGIN_ROOT/x".into()],
            ..Default::default()
        }
        .expand("/p");
        assert_eq!(spec.command.as_deref(), Some("/p/run.sh"));
        assert_eq!(spec.args, ["/p/x"]);
        assert_eq!(
            spec.env.get(PLUGIN_ROOT_VAR).map(String::as_str),
            Some("/p")
        );

        let http = McpServerSpec {
            url: Some("https://x".into()),
            ..Default::default()
        }
        .expand("/p");
        assert!(http.env.is_empty());
    }
}
