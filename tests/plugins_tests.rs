//! Tests for marketplace catalogs, plugin contents and the git-backed store.
#![cfg(feature = "plugins")]

use std::fs;
use std::path::Path;
use std::process::Command;

use orion_core::plugins::{
    Catalog, Component, PluginContents, PluginEntry, PluginSource, PluginStore, RemoteSource,
    PLUGIN_ROOT_VAR,
};
use serde_json::json;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn entry(value: serde_json::Value) -> PluginEntry {
    serde_json::from_value(value).unwrap()
}

const CATALOG: &str = r#"{
  "name": "agenthub",
  "owner": { "name": "nullorder" },
  "plugins": [
    { "name": "grafana", "source": { "source": "github", "repo": "grafana/mcp-grafana" },
      "description": "Grafana dashboards", "tags": ["mcp-servers"], "category": "devops",
      "author": { "username": "grafana" } },
    { "name": "a11y-audit", "source": { "source": "git-subdir",
        "url": "https://github.com/alirezarezvani/claude-skills.git", "path": "engineering-team/a11y-audit/" },
      "description": "Accessibility audit", "tags": ["skills", "commands"] },
    { "name": "pinned", "source": { "source": "github", "repo": "a/b", "sha": "abc123" } },
    { "name": "linear", "source": { "source": "url", "url": "https://github.com/shinpr/linear.git", "ref": "v2" } },
    { "name": "iconify", "source": { "source": "npm", "package": "@anistark/iconify" } },
    { "name": "local", "source": "./plugins/local" },
    { "name": "future", "source": { "source": "s3", "bucket": "x" } },
    { "name": "slug", "source": { "source": "git-subdir", "url": "owner/repo", "path": "p" } },
    { "name": "broken" }
  ]
}"#;

#[test]
fn parses_every_source_shape() {
    let catalog = Catalog::parse("fallback", CATALOG.as_bytes()).unwrap();
    assert_eq!(catalog.name, "agenthub");
    assert_eq!(
        catalog.plugins.len(),
        8,
        "the entry without a source is skipped"
    );

    let git = |name: &str| catalog.get(name).unwrap().source.git();
    let grafana = git("grafana").unwrap();
    assert_eq!(grafana.url, "https://github.com/grafana/mcp-grafana.git");
    assert_eq!(grafana.path, None);

    let a11y = git("a11y-audit").unwrap();
    assert_eq!(a11y.path.as_deref(), Some("engineering-team/a11y-audit"));
    assert_eq!(git("pinned").unwrap().sha.as_deref(), Some("abc123"));
    assert_eq!(git("linear").unwrap().git_ref.as_deref(), Some("v2"));
    assert_eq!(
        git("slug").unwrap().url,
        "https://github.com/owner/repo.git"
    );
    assert!(git("iconify").is_none());
    assert!(git("local").is_none());
    assert!(git("future").is_none());

    let iconify = catalog.get("iconify").unwrap();
    assert!(matches!(
        iconify.source,
        PluginSource::Remote(RemoteSource::Npm { .. })
    ));
    assert_eq!(iconify.source.describe(), "npm @anistark/iconify");
    assert_eq!(
        catalog.get("a11y-audit").unwrap().source.describe(),
        "github.com/alirezarezvani/claude-skills/engineering-team/a11y-audit"
    );
}

#[test]
fn parses_a_bare_array_under_the_given_name() {
    let catalog = Catalog::parse(
        "agenthub",
        br#"[{ "name": "x", "source": { "source": "github", "repo": "a/x" } }]"#,
    )
    .unwrap();
    assert_eq!(catalog.name, "agenthub");
    assert_eq!(catalog.plugins[0].name, "x");
    assert!(Catalog::parse("bad", b"{ not json").is_err());
}

#[test]
fn searches_and_reads_components() {
    let catalog = Catalog::parse("agenthub", CATALOG.as_bytes()).unwrap();
    let names = |q: &str| -> Vec<String> {
        catalog
            .search(q)
            .into_iter()
            .map(|p| p.name.clone())
            .collect()
    };
    assert_eq!(names("GRAFANA"), ["grafana"]);
    assert_eq!(names("devops dashboards"), ["grafana"]);
    assert_eq!(names("audit"), ["a11y-audit"]);
    assert_eq!(names("").len(), 8);

    let a11y = catalog.get("a11y-audit").unwrap();
    assert_eq!(a11y.components(), [Component::Skills, Component::Commands]);
    assert!(a11y.is_portable());
    let inline = entry(json!({
        "name": "pg", "source": { "source": "github", "repo": "t/pg" },
        "mcpServers": { "docs": { "type": "http", "url": "https://mcp.example.com" } }
    }));
    assert_eq!(inline.components(), [Component::McpServers]);
    assert!(!catalog.get("pinned").unwrap().is_portable());
}

#[test]
fn reads_plugin_contents_from_each_layout() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // plugin.json points at a flat MCP file and declares skills as a string.
    write(
        &root.join(".claude-plugin/plugin.json"),
        r#"{ "name": "demo", "skills": "./more", "mcpServers": "./servers.json", "hooks": "./hooks/hooks.json" }"#,
    );
    write(
        &root.join("servers.json"),
        r#"{ "video": { "command": "npx", "args": ["-y", "video-mcp"] },
             "start": { "command": "${CLAUDE_PLUGIN_ROOT}/scripts/start.sh", "env": { "LOG": "error" } } }"#,
    );
    write(
        &root.join("more/helper/SKILL.md"),
        "---\nname: helper\ndescription: Helps.\n---\n",
    );
    write(&root.join("commands/scan.md"), "# scan");
    // A root .mcp.json is ignored when plugin.json names a file.
    write(
        &root.join(".mcp.json"),
        r#"{ "mcpServers": { "ignored": { "command": "x" } } }"#,
    );

    let catalog_entry = entry(json!({
        "name": "demo", "source": { "source": "github", "repo": "a/demo" },
        "mcpServers": { "docs": { "type": "http", "url": "https://mcp.example.com/docs" },
                        "video": { "command": "overridden" } }
    }));
    let contents = PluginContents::read(root, Some(&catalog_entry));
    let names: Vec<&str> = contents.mcp_servers.keys().map(String::as_str).collect();
    assert_eq!(names, ["docs", "start", "video"]);

    let root_str = root.to_string_lossy();
    let start = &contents.mcp_servers["start"];
    assert_eq!(
        start.command.as_deref(),
        Some(&*format!("{root_str}/scripts/start.sh"))
    );
    assert_eq!(start.env["LOG"], "error");
    assert_eq!(start.env[PLUGIN_ROOT_VAR], root_str);
    assert_eq!(
        contents.mcp_servers["video"].command.as_deref(),
        Some("npx")
    );
    assert_eq!(
        contents.mcp_servers["docs"].transport.as_deref(),
        Some("http")
    );

    assert_eq!(contents.skills.len(), 1);
    assert_eq!(contents.skills[0].source.as_deref(), Some("demo"));
    assert_eq!(
        contents.claude_code_only,
        [Component::Commands, Component::Hooks]
    );
    let expected_start = format!("{root_str}/scripts/start.sh");
    assert_eq!(
        contents.required_programs(),
        [expected_start.as_str(), "npx"]
    );
}

#[test]
fn reads_inline_servers_and_a_root_mcp_json() {
    let inline = tempfile::tempdir().unwrap();
    write(
        &inline.path().join(".claude-plugin/plugin.json"),
        r#"{ "name": "chrome", "mcpServers": { "chrome": { "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/mcp/index.js"] } } }"#,
    );
    let contents = PluginContents::read(inline.path(), None);
    let expected = format!("{}/mcp/index.js", inline.path().display());
    assert_eq!(contents.mcp_servers["chrome"].args, [expected]);

    let bare = tempfile::tempdir().unwrap();
    write(
        &bare.path().join(".mcp.json"),
        r#"{ "mcpServers": { "context7": { "type": "http", "url": "https://mcp.context7.com/mcp" } } }"#,
    );
    write(
        &bare.path().join("skills/s/SKILL.md"),
        "---\nname: s\n---\n",
    );
    let contents = PluginContents::read(bare.path(), None);
    assert!(contents.mcp_servers.contains_key("context7"));
    assert_eq!(contents.skills.len(), 1);
    assert!(contents.claude_code_only.is_empty());
    assert!(PluginContents::read(tempfile::tempdir().unwrap().path(), None).is_empty());
}

// --- Store, against a local git repository -------------------------------

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with a plugin in `plugins/demo` and an unrelated folder.
fn source_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    git(repo, &["config", "uploadpack.allowFilter", "true"]);
    write(
        &repo.join("plugins/demo/.claude-plugin/plugin.json"),
        r#"{ "name": "demo", "version": "1.0.0" }"#,
    );
    write(
        &repo.join("plugins/demo/.mcp.json"),
        r#"{ "mcpServers": { "demo": { "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/server.js"] } } }"#,
    );
    write(
        &repo.join("plugins/demo/skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: Say hello.\n---\nSay hello warmly.\n",
    );
    write(&repo.join("other/big.txt"), "not part of the plugin");
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "v1"]);
    dir
}

fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn demo_entry(repo: &Path, sha: Option<&str>) -> PluginEntry {
    let mut source =
        json!({ "source": "git-subdir", "url": file_url(repo), "path": "plugins/demo" });
    if let Some(sha) = sha {
        source["sha"] = json!(sha);
    }
    entry(
        json!({ "name": "demo", "version": "1.0.0", "source": source, "tags": ["skills", "mcp-servers"] }),
    )
}

#[test]
fn installs_updates_and_removes_from_git() {
    let repo = source_repo();
    let first = git(repo.path(), &["rev-parse", "HEAD"]);
    let home = tempfile::tempdir().unwrap();
    let store = PluginStore::new(home.path().join("plugins"));
    let entry = demo_entry(repo.path(), None);

    // Prepare fetches into staging and reports final paths.
    let prepared = store.prepare("hub", &entry).unwrap();
    assert_eq!(prepared.commit, first);
    assert!(prepared.previous.is_none());
    let final_dir = store.root().join("hub/demo/plugins/demo");
    assert_eq!(prepared.dir, final_dir);
    let server = &prepared.contents.mcp_servers["demo"];
    assert_eq!(server.args, [format!("{}/server.js", final_dir.display())]);
    assert_eq!(
        prepared.contents.skills[0].dir,
        final_dir.join("skills/greet")
    );
    assert!(!final_dir.exists(), "nothing is installed before commit");

    let installed = store.commit(prepared).unwrap();
    assert_eq!(installed.dir, final_dir);
    assert!(final_dir.join("skills/greet/SKILL.md").is_file());
    assert!(
        !store.root().join("hub/demo/other/big.txt").exists(),
        "only the plugin's folder is checked out"
    );
    assert_eq!(store.installed().unwrap().len(), 1);
    assert_eq!(store.skills().unwrap()[0].name, "greet");
    assert_eq!(
        store.skills().unwrap()[0].body().unwrap(),
        "Say hello warmly.\n"
    );
    assert_eq!(store.latest_commit(&entry).unwrap(), first);

    // A new commit upstream is an update.
    write(
        &repo.path().join("plugins/demo/skills/bye/SKILL.md"),
        "---\nname: bye\n---\nBye.\n",
    );
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "v2"]);
    let second = git(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(store.latest_commit(&entry).unwrap(), second);

    let update = store.prepare("hub", &entry).unwrap();
    assert_eq!(update.previous.as_ref().unwrap().commit, first);
    let updated = store.commit(update).unwrap();
    assert_eq!(updated.commit, second);
    assert_eq!(store.installed().unwrap().len(), 1);
    assert_eq!(store.skills().unwrap().len(), 2);

    // Pinning to the first commit installs it even though main moved on.
    let pinned = store
        .prepare("hub", &demo_entry(repo.path(), Some(&first)))
        .unwrap();
    assert_eq!(pinned.commit, first);
    assert_eq!(pinned.contents.skills.len(), 1);
    store.cancel(pinned);
    let staging = store.root().join(".staging");
    assert_eq!(
        fs::read_dir(&staging).unwrap().count(),
        0,
        "cancel cleans staging"
    );

    let removed = store.remove("hub", "demo").unwrap();
    assert_eq!(removed.commit, second);
    assert!(!store.root().join("hub/demo").exists());
    assert!(store.installed().unwrap().is_empty());
    assert!(store.remove("hub", "demo").is_err());
}

#[test]
fn refuses_unsupported_sources_and_bad_names() {
    let home = tempfile::tempdir().unwrap();
    let store = PluginStore::new(home.path());
    let npm = entry(json!({ "name": "iconify", "source": { "source": "npm", "package": "x" } }));
    assert!(store
        .prepare("hub", &npm)
        .unwrap_err()
        .to_string()
        .contains("cannot be installed"));

    let ssh = entry(
        json!({ "name": "x", "source": { "source": "url", "url": "git@github.com:a/b.git" } }),
    );
    assert!(store.prepare("hub", &ssh).is_err());

    let escape = entry(
        json!({ "name": "x", "source": { "source": "git-subdir", "url": "https://github.com/a/b.git", "path": "../.." } }),
    );
    assert!(store
        .prepare("hub", &escape)
        .unwrap_err()
        .to_string()
        .contains("unsafe"));

    let repo = source_repo();
    assert!(store
        .prepare("../hub", &demo_entry(repo.path(), None))
        .is_err());
    let missing = entry(
        json!({ "name": "demo", "source": { "source": "git-subdir", "url": file_url(repo.path()), "path": "nope" } }),
    );
    assert!(store
        .prepare("hub", &missing)
        .unwrap_err()
        .to_string()
        .contains("no folder"));
    assert_eq!(
        fs::read_dir(home.path().join(".staging")).unwrap().count(),
        0
    );
}

#[test]
fn caches_catalogs() {
    let home = tempfile::tempdir().unwrap();
    let store = PluginStore::new(home.path());
    assert!(store.load_catalog("agenthub").is_none());
    let catalog = Catalog::parse("agenthub", CATALOG.as_bytes()).unwrap();
    store.save_catalog(&catalog).unwrap();
    let loaded = store.load_catalog("agenthub").unwrap();
    assert_eq!(loaded.plugins, catalog.plugins);
}
