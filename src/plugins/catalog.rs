//! Marketplace catalogs: the list of plugins a marketplace offers.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str = concat!("orion-core/", env!("CARGO_PKG_VERSION"));

/// A marketplace's catalog, in the Claude Code `marketplace.json` format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    /// The marketplace's name, such as `agenthub`.
    pub name: String,
    /// Every plugin it lists, in the marketplace's order.
    pub plugins: Vec<PluginEntry>,
}

/// One plugin in a [`Catalog`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginEntry {
    /// Unique name within the marketplace.
    pub name: String,
    /// Where the plugin's files come from.
    pub source: PluginSource,
    /// What it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its version, matching the plugin's own manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Who made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Author>,
    /// Marketplace category, such as `development`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Tags, including the component types it ships (`skills`, `mcp-servers`, …).
    #[serde(default)]
    pub tags: Vec<String>,
    /// SPDX licence id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Project homepage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// MCP servers declared in the catalog rather than in the plugin itself.
    #[serde(
        default,
        rename = "mcpServers",
        skip_serializing_if = "Option::is_none"
    )]
    pub mcp_servers: Option<serde_json::Value>,
}

/// A plugin author.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Author {
    /// Display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// GitHub username.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Contact email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Profile or homepage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Where a plugin's files come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginSource {
    /// A path inside the marketplace's own repository.
    Path(String),
    /// A repository or package elsewhere.
    Remote(RemoteSource),
}

/// A plugin source outside the marketplace's repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case")]
pub enum RemoteSource {
    /// A GitHub repository, `owner/name`.
    Github {
        /// `owner/name`.
        repo: String,
        /// Branch or tag to install.
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        /// Exact commit to install.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    /// A git repository by URL.
    Url {
        /// Clone URL.
        url: String,
        /// Branch or tag to install.
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        /// Exact commit to install.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    /// A folder inside a git repository.
    GitSubdir {
        /// Clone URL (or `owner/name` on GitHub).
        url: String,
        /// The plugin's folder within the repository.
        path: String,
        /// Branch or tag to install.
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        /// Exact commit to install.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    /// An npm package.
    Npm {
        /// Package name.
        package: String,
        /// Version or range.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// A source type this version does not know.
    #[serde(other)]
    Unknown,
}

/// A plugin source that can be fetched with git.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSource {
    /// Clone URL.
    pub url: String,
    /// The plugin's folder within the repository, if not its root.
    pub path: Option<String>,
    /// Branch or tag; the default branch when `None`.
    pub git_ref: Option<String>,
    /// Exact commit, which wins over `git_ref`.
    pub sha: Option<String>,
}

impl PluginSource {
    /// The git repository to fetch, or `None` for npm, catalog-relative and
    /// unknown sources.
    pub fn git(&self) -> Option<GitSource> {
        let Self::Remote(remote) = self else {
            return None;
        };
        let (url, path, git_ref, sha) = match remote {
            RemoteSource::Github { repo, git_ref, sha } => (github_url(repo), None, git_ref, sha),
            RemoteSource::Url { url, git_ref, sha } => (repo_url(url), None, git_ref, sha),
            RemoteSource::GitSubdir {
                url,
                path,
                git_ref,
                sha,
            } => (
                repo_url(url),
                Some(path.trim_matches('/').to_string()),
                git_ref,
                sha,
            ),
            RemoteSource::Npm { .. } | RemoteSource::Unknown => return None,
        };
        Some(GitSource {
            url,
            path: path.filter(|p| !p.is_empty() && p != "."),
            git_ref: git_ref.clone(),
            sha: sha.clone(),
        })
    }

    /// A short human description, such as `github.com/owner/repo/plugins/x`.
    pub fn describe(&self) -> String {
        if let Some(git) = self.git() {
            let base = git
                .url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches(".git")
                .to_string();
            return match git.path {
                Some(p) => format!("{base}/{p}"),
                None => base,
            };
        }
        match self {
            Self::Path(p) => format!("marketplace path {p}"),
            Self::Remote(RemoteSource::Npm { package, version }) => match version {
                Some(v) => format!("npm {package}@{v}"),
                None => format!("npm {package}"),
            },
            Self::Remote(_) => "unknown source".into(),
        }
    }
}

fn github_url(repo: &str) -> String {
    format!("https://github.com/{}.git", repo.trim_matches('/'))
}

/// A clone URL, reading a bare `owner/name` as a GitHub repository.
fn repo_url(url: &str) -> String {
    let looks_like_slug = !url.contains("://")
        && !url.contains('@')
        && url.split('/').filter(|s| !s.is_empty()).count() == 2;
    if looks_like_slug {
        github_url(url)
    } else {
        url.to_string()
    }
}

/// The kinds of things a plugin can ship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Component {
    /// MCP servers: tools any MCP client can call.
    McpServers,
    /// Agent Skills (`SKILL.md`).
    Skills,
    /// Claude Code slash commands.
    Commands,
    /// Claude Code subagents.
    Agents,
    /// Claude Code hooks.
    Hooks,
    /// Language servers.
    LspServers,
}

impl Component {
    /// Whether an agent outside Claude Code can use it: MCP servers and skills.
    pub fn is_portable(self) -> bool {
        matches!(self, Self::McpServers | Self::Skills)
    }

    fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "mcp-servers" | "mcp" => Self::McpServers,
            "skills" => Self::Skills,
            "commands" => Self::Commands,
            "agents" => Self::Agents,
            "hooks" => Self::Hooks,
            "lsp-servers" => Self::LspServers,
            _ => return None,
        })
    }
}

impl PluginEntry {
    /// The components its tags (and catalog MCP servers) say it ships.
    pub fn components(&self) -> Vec<Component> {
        let mut out: Vec<Component> = Vec::new();
        let tagged = self.tags.iter().filter_map(|t| Component::from_tag(t));
        let inline = self.mcp_servers.is_some().then_some(Component::McpServers);
        for c in tagged.chain(inline) {
            if !out.contains(&c) {
                out.push(c);
            }
        }
        out
    }

    /// Whether it ships anything an agent outside Claude Code can use.
    pub fn is_portable(&self) -> bool {
        self.components().iter().any(|c| c.is_portable())
    }

    /// Whether every word of `query` appears in its name, description, tags,
    /// category or author.
    pub fn matches(&self, query: &str) -> bool {
        let haystack = self.search_text();
        query
            .to_lowercase()
            .split_whitespace()
            .all(|word| haystack.contains(word))
    }

    fn search_text(&self) -> String {
        let author = self.author.as_ref();
        [
            Some(self.name.as_str()),
            self.description.as_deref(),
            self.category.as_deref(),
            author.and_then(|a| a.name.as_deref()),
            author.and_then(|a| a.username.as_deref()),
        ]
        .into_iter()
        .flatten()
        .chain(self.tags.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CatalogDoc {
    Marketplace {
        #[serde(default)]
        name: Option<String>,
        plugins: Vec<serde_json::Value>,
    },
    List(Vec<serde_json::Value>),
}

impl Catalog {
    /// Parse a catalog: a `marketplace.json` object, or a bare array of plugin
    /// entries (named `name`). Entries that do not parse are skipped with a
    /// warning, so one bad entry does not hide the rest.
    pub fn parse(name: &str, bytes: &[u8]) -> CoreResult<Self> {
        let doc: CatalogDoc = serde_json::from_slice(bytes)
            .map_err(|e| CoreError::Plugin(format!("catalog '{name}' is not valid: {e}")))?;
        let (doc_name, raw) = match doc {
            CatalogDoc::Marketplace { name, plugins } => (name, plugins),
            CatalogDoc::List(plugins) => (None, plugins),
        };
        let plugins = raw
            .into_iter()
            .filter_map(|v| match serde_json::from_value::<PluginEntry>(v) {
                Ok(entry) => Some(entry),
                Err(e) => {
                    log::warn!("skipping a plugin in catalog '{name}': {e}");
                    None
                }
            })
            .collect();
        Ok(Self {
            name: doc_name
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| name.to_string()),
            plugins,
        })
    }

    /// Download and parse the catalog at `url`, naming it `name` if it carries
    /// no name of its own.
    pub async fn fetch(name: &str, url: &str) -> CoreResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(FETCH_TIMEOUT)
            .build()
            .map_err(|e| CoreError::Plugin(format!("failed to build HTTP client: {e}")))?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::Plugin(format!("cannot reach {url}: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(CoreError::Plugin(format!("{url} returned HTTP {status}")));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| CoreError::Plugin(format!("reading {url} failed: {e}")))?;
        Self::parse(name, &bytes)
    }

    /// The plugin called `name`.
    pub fn get(&self, name: &str) -> Option<&PluginEntry> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// Plugins matching `query` (see [`PluginEntry::matches`]), those whose
    /// name matches first, otherwise in catalog order.
    pub fn search(&self, query: &str) -> Vec<&PluginEntry> {
        let q = query.trim().to_lowercase();
        let mut hits: Vec<&PluginEntry> = self.plugins.iter().filter(|p| p.matches(&q)).collect();
        if !q.is_empty() {
            hits.sort_by_key(|p| !p.name.to_lowercase().contains(&q));
        }
        hits
    }
}
