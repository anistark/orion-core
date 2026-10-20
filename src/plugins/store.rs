//! The plugin store: plugins installed from git into one directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::catalog::{Catalog, GitSource, PluginEntry};
use super::contents::PluginContents;
use crate::error::{CoreError, CoreResult};
use crate::skills::{is_plain_relative, Skill};

const MANIFEST: &str = "installed.json";
const STAGING: &str = ".staging";
const CATALOGS: &str = "catalogs";
/// Staged installs older than this are left over from an earlier run.
const STALE_STAGING: Duration = Duration::from_secs(60 * 60);

/// A plugin in the store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledPlugin {
    /// The marketplace it came from.
    pub marketplace: String,
    /// Its catalog entry at install time.
    pub entry: PluginEntry,
    /// The commit installed.
    pub commit: String,
    /// The plugin's directory (inside the checkout, for a plugin in a subfolder).
    pub dir: PathBuf,
    /// When it was installed or last updated.
    pub installed_at: DateTime<Utc>,
}

impl InstalledPlugin {
    /// Its name.
    pub fn name(&self) -> &str {
        &self.entry.name
    }

    /// What it ships, read from disk.
    pub fn contents(&self) -> PluginContents {
        PluginContents::read(&self.dir, Some(&self.entry))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    plugins: Vec<InstalledPlugin>,
}

/// A plugin fetched into the staging area, waiting for [`PluginStore::commit`]
/// or [`PluginStore::cancel`]. Fetching runs none of the plugin's code.
#[derive(Debug, Clone)]
pub struct PreparedInstall {
    /// The marketplace it comes from.
    pub marketplace: String,
    /// Its catalog entry.
    pub entry: PluginEntry,
    /// The commit fetched.
    pub commit: String,
    /// What it ships, with paths as they will be once installed.
    pub contents: PluginContents,
    /// The installed version this replaces, for an update.
    pub previous: Option<InstalledPlugin>,
    /// Where the plugin will live once committed.
    pub dir: PathBuf,
    staged: PathBuf,
}

/// Plugins installed from git into one directory, by default
/// `~/.agents/plugins`, a location other agents can read too:
///
/// ```text
/// <root>/installed.json             every installed plugin
/// <root>/<marketplace>/<plugin>/    its git checkout, at a pinned commit
/// <root>/catalogs/<marketplace>.json the last fetched catalog
/// ```
///
/// Operations block on `git` and the file system; from async code, run them
/// with `tokio::task::spawn_blocking`. Installs are two steps:
/// [`prepare`](Self::prepare) fetches into a staging area so the caller can
/// show what the plugin ships, then [`commit`](Self::commit) moves it into
/// place.
#[derive(Debug, Clone)]
pub struct PluginStore {
    root: PathBuf,
    git: PathBuf,
}

impl PluginStore {
    /// A store rooted at `root`, running `git` from `PATH`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            git: PathBuf::from("git"),
        }
    }

    /// `~/.agents/plugins`, or `None` when there is no home directory.
    pub fn default_root() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))?;
        Some(PathBuf::from(home).join(".agents").join("plugins"))
    }

    /// Run `git` from `path` rather than looking it up on `PATH`.
    pub fn with_git(mut self, path: impl Into<PathBuf>) -> Self {
        self.git = path.into();
        self
    }

    /// The store's directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every installed plugin.
    pub fn installed(&self) -> CoreResult<Vec<InstalledPlugin>> {
        Ok(self.load_manifest()?.plugins)
    }

    /// The installed plugin `name` from `marketplace`.
    pub fn get(&self, marketplace: &str, name: &str) -> CoreResult<Option<InstalledPlugin>> {
        Ok(self
            .installed()?
            .into_iter()
            .find(|p| p.marketplace == marketplace && p.entry.name == name))
    }

    /// Skills of every installed plugin, each marked with its plugin's name.
    pub fn skills(&self) -> CoreResult<Vec<Skill>> {
        Ok(self
            .installed()?
            .iter()
            .flat_map(|p| p.contents().skills)
            .collect())
    }

    /// Fetch `entry` into the staging area and read what it ships.
    ///
    /// The commit is the entry's `sha`, else the tip of its `ref`, else of the
    /// default branch. Only `https://` and `file://` repositories are fetched.
    pub fn prepare(&self, marketplace: &str, entry: &PluginEntry) -> CoreResult<PreparedInstall> {
        check_segment("marketplace", marketplace)?;
        check_segment("plugin", &entry.name)?;
        let source = git_source(entry)?;
        let staging = self.root.join(STAGING);
        std::fs::create_dir_all(&staging).map_err(io("create the staging area"))?;
        self.clean_staging(&staging);

        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let staged = staging.join(format!("{marketplace}--{}-{nonce}", entry.name));
        let final_dir = self.plugin_dir(marketplace, &entry.name, &source);
        let result = self.fetch(&staged, &source).and_then(|commit| {
            let staged_dir = match &source.path {
                Some(p) => staged.join(p),
                None => staged.clone(),
            };
            if !staged_dir.is_dir() {
                return Err(CoreError::Plugin(format!(
                    "{} has no folder '{}' at {commit}",
                    source.url,
                    source.path.as_deref().unwrap_or_default()
                )));
            }
            let contents = PluginContents::read_as(&staged_dir, &final_dir, Some(entry));
            Ok((commit, contents))
        });
        let (commit, contents) = match result {
            Ok(ok) => ok,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staged);
                return Err(e);
            }
        };
        Ok(PreparedInstall {
            marketplace: marketplace.to_string(),
            entry: entry.clone(),
            commit,
            contents,
            previous: self.get(marketplace, &entry.name)?,
            dir: final_dir,
            staged,
        })
    }

    /// Move a prepared plugin into place, replacing any installed version.
    pub fn commit(&self, prepared: PreparedInstall) -> CoreResult<InstalledPlugin> {
        let source = git_source(&prepared.entry)?;
        let checkout = self.checkout_dir(&prepared.marketplace, &prepared.entry.name);
        if let Some(parent) = checkout.parent() {
            std::fs::create_dir_all(parent).map_err(io("create the plugin folder"))?;
        }
        let old = prepared.staged.with_extension("old");
        let had_old = checkout.exists();
        if had_old {
            std::fs::rename(&checkout, &old).map_err(io("move the old version aside"))?;
        }
        if let Err(e) = std::fs::rename(&prepared.staged, &checkout) {
            if had_old {
                let _ = std::fs::rename(&old, &checkout);
            }
            return Err(io("install the plugin")(e));
        }
        if had_old {
            let _ = std::fs::remove_dir_all(&old);
        }

        let installed = InstalledPlugin {
            dir: self.plugin_dir(&prepared.marketplace, &prepared.entry.name, &source),
            marketplace: prepared.marketplace,
            entry: prepared.entry,
            commit: prepared.commit,
            installed_at: Utc::now(),
        };
        let mut manifest = self.load_manifest()?;
        manifest.plugins.retain(|p| {
            !(p.marketplace == installed.marketplace && p.entry.name == installed.entry.name)
        });
        manifest.plugins.push(installed.clone());
        manifest
            .plugins
            .sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
        self.save_manifest(&manifest)?;
        Ok(installed)
    }

    /// Throw away a prepared plugin.
    pub fn cancel(&self, prepared: PreparedInstall) {
        let _ = std::fs::remove_dir_all(&prepared.staged);
    }

    /// Uninstall `name` from `marketplace`, deleting its files.
    pub fn remove(&self, marketplace: &str, name: &str) -> CoreResult<InstalledPlugin> {
        let mut manifest = self.load_manifest()?;
        let index = manifest
            .plugins
            .iter()
            .position(|p| p.marketplace == marketplace && p.entry.name == name)
            .ok_or_else(|| CoreError::Plugin(format!("'{name}' is not installed")))?;
        let removed = manifest.plugins.remove(index);
        self.save_manifest(&manifest)?;
        let checkout = self.checkout_dir(marketplace, name);
        if checkout.exists() {
            std::fs::remove_dir_all(&checkout).map_err(io("delete the plugin's files"))?;
        }
        Ok(removed)
    }

    /// The commit an install of `entry` would fetch now: its `sha`, or the
    /// current tip of its `ref` or default branch. Compare with
    /// [`InstalledPlugin::commit`] to see whether an update is out.
    pub fn latest_commit(&self, entry: &PluginEntry) -> CoreResult<String> {
        let source = git_source(entry)?;
        if let Some(sha) = &source.sha {
            return Ok(sha.clone());
        }
        let target = source.git_ref.as_deref().unwrap_or("HEAD");
        let out = self.git(None, &["ls-remote", "--", &source.url, target])?;
        let lines: Vec<(&str, &str)> = out.lines().filter_map(|l| l.split_once('\t')).collect();
        lines
            .iter()
            .find(|(_, r)| r.ends_with("^{}"))
            .or_else(|| lines.first())
            .map(|(sha, _)| sha.to_string())
            .ok_or_else(|| CoreError::Plugin(format!("{} has no '{target}'", source.url)))
    }

    /// Save `catalog` as the last fetched copy of its marketplace.
    pub fn save_catalog(&self, catalog: &Catalog) -> CoreResult<()> {
        check_segment("marketplace", &catalog.name)?;
        let path = self
            .root
            .join(CATALOGS)
            .join(format!("{}.json", catalog.name));
        write_atomic(&path, &serde_json::to_vec(catalog).map_err(json)?)
    }

    /// The last fetched catalog of `marketplace`, if there is one.
    pub fn load_catalog(&self, marketplace: &str) -> Option<Catalog> {
        check_segment("marketplace", marketplace).ok()?;
        let path = self.root.join(CATALOGS).join(format!("{marketplace}.json"));
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn checkout_dir(&self, marketplace: &str, name: &str) -> PathBuf {
        self.root.join(marketplace).join(name)
    }

    fn plugin_dir(&self, marketplace: &str, name: &str, source: &GitSource) -> PathBuf {
        let checkout = self.checkout_dir(marketplace, name);
        match &source.path {
            Some(p) => checkout.join(p),
            None => checkout,
        }
    }

    fn fetch(&self, dir: &Path, source: &GitSource) -> CoreResult<String> {
        check_url(&source.url)?;
        let dir_str = dir.to_string_lossy();
        self.git(None, &["init", "-q", &dir_str])?;
        self.git(Some(dir), &["remote", "add", "origin", &source.url])?;
        if let Some(path) = &source.path {
            self.git(Some(dir), &["sparse-checkout", "set", "--", path])?;
        }
        let target = source
            .sha
            .as_deref()
            .or(source.git_ref.as_deref())
            .unwrap_or("HEAD");
        self.git(
            Some(dir),
            &[
                "fetch",
                "-q",
                "--depth",
                "1",
                "--filter=blob:none",
                "origin",
                target,
            ],
        )?;
        self.git(Some(dir), &["checkout", "-q", "FETCH_HEAD"])?;
        Ok(self
            .git(Some(dir), &["rev-parse", "HEAD"])?
            .trim()
            .to_string())
    }

    fn git(&self, cwd: Option<&Path>, args: &[&str]) -> CoreResult<String> {
        let mut cmd = Command::new(&self.git);
        // Never ask for credentials, never run a transport that executes commands.
        cmd.args(["-c", "credential.helper=", "-c", "protocol.ext.allow=never"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null());
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().map_err(|e| {
            CoreError::Plugin(format!("cannot run git ({e}); install git and try again"))
        })?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let reason = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no output")
                .trim();
            return Err(CoreError::Plugin(format!(
                "git {} failed: {reason}",
                args[0]
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn clean_staging(&self, staging: &Path) {
        let Ok(entries) = std::fs::read_dir(staging) else {
            return;
        };
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_STAGING);
            if stale {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    fn load_manifest(&self) -> CoreResult<Manifest> {
        let path = self.root.join(MANIFEST);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| CoreError::Plugin(format!("{} is not valid: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(e) => Err(io("read the plugin list")(e)),
        }
    }

    fn save_manifest(&self, manifest: &Manifest) -> CoreResult<()> {
        let bytes = serde_json::to_vec_pretty(manifest).map_err(json)?;
        write_atomic(&self.root.join(MANIFEST), &bytes)
    }
}

fn git_source(entry: &PluginEntry) -> CoreResult<GitSource> {
    let source = entry.source.git().ok_or_else(|| {
        CoreError::Plugin(format!(
            "'{}' comes from {}, which cannot be installed yet",
            entry.name,
            entry.source.describe()
        ))
    })?;
    if let Some(path) = &source.path {
        if !is_plain_relative(Path::new(path)) {
            return Err(CoreError::Plugin(format!(
                "'{}' has an unsafe folder path '{path}'",
                entry.name
            )));
        }
    }
    Ok(source)
}

fn check_url(url: &str) -> CoreResult<()> {
    if url.starts_with("https://") || url.starts_with("file://") {
        Ok(())
    } else {
        Err(CoreError::Plugin(format!(
            "only https:// repositories can be installed, not '{url}'"
        )))
    }
}

/// A name used as one folder: no separators, no dots-only names.
fn check_segment(what: &str, name: &str) -> CoreResult<()> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(CoreError::Plugin(format!(
            "'{name}' is not a valid {what} name"
        )))
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io("create the store"))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io("write the store"))?;
    std::fs::rename(&tmp, path).map_err(io("write the store"))
}

fn io(what: &'static str) -> impl Fn(std::io::Error) -> CoreError {
    move |e| CoreError::Plugin(format!("cannot {what}: {e}"))
}

fn json(e: serde_json::Error) -> CoreError {
    CoreError::Plugin(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments() {
        assert!(check_segment("plugin", "a11y-audit").is_ok());
        assert!(check_segment("plugin", "v1.2_x").is_ok());
        for bad in ["", ".", "..", ".hidden", "a/b", "a\\b", "a b"] {
            assert!(check_segment("plugin", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn urls() {
        assert!(check_url("https://github.com/a/b.git").is_ok());
        assert!(check_url("file:///tmp/repo").is_ok());
        for bad in [
            "http://x",
            "ext::sh -c x",
            "git@github.com:a/b",
            "/tmp/repo",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }
}
