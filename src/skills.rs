//! Agent Skills: folders of instructions the model loads when a task calls for one.
//!
//! A skill is a directory holding a `SKILL.md`: YAML frontmatter with a `name`
//! and a `description`, then Markdown instructions, often next to scripts and
//! reference files. This is the format Claude Code plugins and other agents
//! share.
//!
//! The model sees only each skill's name and description, through
//! `SkillSet::system_section`, so a large collection stays cheap. When a task
//! matches, it calls the `load_skill` tool from `SkillSet::tool` (feature
//! `tools`) to read the instructions, and any bundled file they point to.
//!
//! ```no_run
//! use orion_core::skills::{discover_skills, SkillSet};
//!
//! // `~/.agents/skills/<name>/SKILL.md`, one skill per folder.
//! let skills = SkillSet::new(discover_skills("/home/me/.agents/skills".as_ref(), &["."]));
//! let mut system_prompt = String::from("You are a helpful assistant.");
//! if let Some(section) = skills.system_section() {
//!     system_prompt.push_str("\n\n");
//!     system_prompt.push_str(&section);
//! }
//! // agent.set_system_prompt(system_prompt);
//! // agent.set_tools(vec![Box::new(skills.tool())]);
//! ```

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// The file that marks a directory as a skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// Longest description shown in `SkillSet::system_section`, in characters.
const MAX_INDEX_DESCRIPTION: usize = 1024;
/// Most bundled files listed when a skill is loaded.
const MAX_LISTED_FILES: usize = 200;
/// Largest bundled file `load_skill` returns, in bytes.
const MAX_FILE_BYTES: u64 = 256 * 1024;

/// One skill: a directory with a `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// The name the model loads it by (frontmatter `name`, or the folder's name).
    pub name: String,
    /// When to use it (frontmatter `description`).
    pub description: String,
    /// The skill's directory.
    pub dir: PathBuf,
    /// Tools the skill expects (frontmatter `allowed-tools`). Informational.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Where it came from, such as the plugin that ships it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "allowed-tools")]
    allowed_tools: Option<StringOrList>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(s) => s
                .split([',', ' '])
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect(),
            Self::Many(v) => v,
        }
    }
}

impl Skill {
    /// Load the skill in `dir`, reading its `SKILL.md` frontmatter.
    ///
    /// A missing `name` falls back to the folder's name and a missing
    /// `description` to an empty one.
    pub fn load(dir: &Path) -> CoreResult<Self> {
        let file = dir.join(SKILL_FILE);
        let text = std::fs::read_to_string(&file)
            .map_err(|e| CoreError::Skill(format!("cannot read {}: {e}", file.display())))?;
        let front = match split_frontmatter(&text) {
            Some((yaml, _)) => serde_norway::from_str::<Option<Frontmatter>>(yaml)
                .map_err(|e| CoreError::Skill(format!("{}: bad frontmatter: {e}", file.display())))?
                .unwrap_or_default(),
            None => Frontmatter::default(),
        };
        let folder = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = front
            .name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or(folder);
        if name.is_empty() {
            return Err(CoreError::Skill(format!("{} has no name", file.display())));
        }
        Ok(Self {
            name,
            description: front
                .description
                .map(|d| d.trim().to_string())
                .unwrap_or_default(),
            dir: dir.to_path_buf(),
            allowed_tools: front
                .allowed_tools
                .map(StringOrList::into_vec)
                .unwrap_or_default(),
            source: None,
        })
    }

    /// The same skill, marked as coming from `source`.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// The instructions: `SKILL.md` without its frontmatter.
    pub fn body(&self) -> CoreResult<String> {
        let file = self.dir.join(SKILL_FILE);
        let text = std::fs::read_to_string(&file)
            .map_err(|e| CoreError::Skill(format!("cannot read {}: {e}", file.display())))?;
        Ok(match split_frontmatter(&text) {
            Some((_, body)) => body.trim_start().to_string(),
            None => text,
        })
    }

    /// Files bundled with the skill, relative to its directory, sorted. Hidden
    /// entries and `SKILL.md` itself are left out.
    pub fn files(&self) -> Vec<String> {
        let mut out = Vec::new();
        collect_files(&self.dir, &self.dir, &mut out);
        out.sort();
        out
    }

    /// Read a bundled file by its path relative to the skill's directory.
    ///
    /// Refuses paths that leave the directory, through `..` or a symlink.
    pub fn read_file(&self, relative: &str) -> CoreResult<String> {
        let rel = Path::new(relative);
        if !is_plain_relative(rel) {
            return Err(CoreError::Skill(format!(
                "'{relative}' is not a path inside the skill"
            )));
        }
        let missing =
            || CoreError::Skill(format!("skill '{}' has no file '{relative}'", self.name));
        let base = self.dir.canonicalize().map_err(|_| missing())?;
        let path = base.join(rel).canonicalize().map_err(|_| missing())?;
        if !path.starts_with(&base) || !path.is_file() {
            return Err(missing());
        }
        let len = path.metadata().map(|m| m.len()).unwrap_or(0);
        if len > MAX_FILE_BYTES {
            return Err(CoreError::Skill(format!(
                "'{relative}' is too large to load ({len} bytes)"
            )));
        }
        let bytes = std::fs::read(&path).map_err(|_| missing())?;
        String::from_utf8(bytes)
            .map_err(|_| CoreError::Skill(format!("'{relative}' is not a text file")))
    }
}

/// Split `---`-fenced YAML frontmatter from the rest of a Markdown file.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first = text.lines().next()?;
    if first.trim_end() != "---" {
        return None;
    }
    let start = first.len()
        + if text[first.len()..].starts_with("\r\n") {
            2
        } else {
            1
        };
    let mut offset = start.min(text.len());
    for line in text[offset..].split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&text[start..offset], &text[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

/// Whether `path` is relative and never steps outside its base.
pub(crate) fn is_plain_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= MAX_LISTED_FILES {
            return;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_files(base, &path, out);
        } else if kind.is_file() && !(dir == base && name == SKILL_FILE) {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// Find the skills under `root`.
///
/// Each entry of `places` (relative to `root`; `["skills"]` when empty) is
/// either a skill directory itself or a directory of skill directories. A
/// `SKILL.md` directly in `root` counts too. This matches how Claude Code reads
/// a plugin's `skills` setting. Places that leave `root` are ignored, and
/// skills that fail to load are skipped with a warning.
pub fn discover_skills(root: &Path, places: &[&str]) -> Vec<Skill> {
    let defaults = ["skills"];
    let places = if places.is_empty() {
        &defaults[..]
    } else {
        places
    };
    let mut dirs = Vec::new();
    for place in places {
        let rel = Path::new(place);
        if !is_plain_relative(rel) {
            log::warn!("skipping skills path '{place}' outside {}", root.display());
            continue;
        }
        let dir = root.join(rel);
        if dir.join(SKILL_FILE).is_file() {
            dirs.push(dir);
        } else if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut children: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.join(SKILL_FILE).is_file())
                .collect();
            children.sort();
            dirs.extend(children);
        }
    }
    if root.join(SKILL_FILE).is_file() {
        dirs.push(root.to_path_buf());
    }

    let mut seen = HashSet::new();
    dirs.into_iter()
        .filter(|d| seen.insert(d.canonicalize().unwrap_or_else(|_| d.clone())))
        .filter_map(|d| match Skill::load(&d) {
            Ok(skill) => Some(skill),
            Err(e) => {
                log::warn!("skipping skill: {e}");
                None
            }
        })
        .collect()
}

/// The skills an agent is offered, by unique name.
#[derive(Debug, Clone, Default)]
pub struct SkillSet {
    skills: Vec<Skill>,
}

impl SkillSet {
    /// A set of `skills`. When two share a name, the first is kept.
    pub fn new(skills: impl IntoIterator<Item = Skill>) -> Self {
        let mut names = HashSet::new();
        let skills = skills
            .into_iter()
            .filter(|s| {
                let fresh = names.insert(s.name.clone());
                if !fresh {
                    log::warn!(
                        "skipping duplicate skill '{}' in {}",
                        s.name,
                        s.dir.display()
                    );
                }
                fresh
            })
            .collect();
        Self { skills }
    }

    /// The skills, in order.
    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    /// Whether the set has no skills.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// The skill called `name`.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// A system-prompt section listing each skill's name and description and
    /// telling the model to load one with `load_skill`. `None` for an empty set.
    pub fn system_section(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let mut out = String::from(
            "# Skills\n\nSkills are instructions for specific tasks. They are not tools: \
             when a request matches a skill's description, call the `load_skill` tool with \
             the skill's name as `name` (for example `{\"name\": \"<skill>\"}`) before you \
             answer, then follow what it says.\n\nAvailable skills:\n",
        );
        for skill in &self.skills {
            let description = one_line(&skill.description, MAX_INDEX_DESCRIPTION);
            if description.is_empty() {
                out.push_str(&format!("- {}\n", skill.name));
            } else {
                out.push_str(&format!("- {}: {description}\n", skill.name));
            }
        }
        Some(out.trim_end().to_string())
    }

    /// The `load_skill` tool for this set.
    #[cfg(feature = "tools")]
    pub fn tool(&self) -> SkillTool {
        SkillTool {
            skills: self.clone(),
        }
    }

    /// What `load_skill` returns: the instructions of skill `name` followed by
    /// its file list, or, with `file`, that bundled file.
    pub fn load(&self, name: &str, file: Option<&str>) -> CoreResult<String> {
        let skill = self
            .get(name)
            .ok_or_else(|| CoreError::Skill(format!("no skill named '{name}'")))?;
        if let Some(file) = file.filter(|f| !f.trim().is_empty()) {
            return skill.read_file(file.trim());
        }
        let mut out = skill.body()?;
        let files = skill.files();
        if !files.is_empty() {
            out.push_str(&format!(
                "\n\n---\nFiles in this skill (directory: {}). Load one with `load_skill` \
                 and `file` set to its path:\n",
                skill.dir.display()
            ));
            for f in files {
                out.push_str(&format!("- {f}\n"));
            }
        }
        Ok(out)
    }

    /// The skill a `/name request` message names, and the request after it.
    ///
    /// `None` unless `text` starts with `/` and a skill's exact name (or the
    /// same name in another case) followed by whitespace or the end, so
    /// `/etc/hosts` or `/unknown` stay plain text.
    pub fn parse_command<'a>(&self, text: &'a str) -> Option<(&Skill, &'a str)> {
        let rest = text.trim_start().strip_prefix('/')?;
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (name, request) = rest.split_at(end);
        let skill = self.get(name).or_else(|| {
            self.skills
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(name))
        })?;
        Some((skill, request.trim()))
    }

    /// Turn a `/name request` message into one that carries the skill's
    /// instructions, so the model follows them without having to call
    /// `load_skill`. `Ok(None)` when `text` is not a skill command (see
    /// [`parse_command`](Self::parse_command)).
    ///
    /// The message is the skill's instructions in a `<skill name="…">` block,
    /// then the request; [`parse_expanded`] reads it back.
    pub fn expand_command(&self, text: &str) -> CoreResult<Option<SkillCommand>> {
        let Some((skill, request)) = self.parse_command(text) else {
            return Ok(None);
        };
        let instructions = self.load(&skill.name, None)?;
        let tag = skill.name.replace('"', "");
        let ask = if request.is_empty() {
            NO_REQUEST
        } else {
            request
        };
        Ok(Some(SkillCommand {
            skill: skill.name.clone(),
            request: request.to_string(),
            message: format!(
                "<skill name=\"{tag}\">\n{SKILL_PREAMBLE}\n\n{}\n</skill>\n\n{ask}",
                instructions.trim_end()
            ),
        }))
    }
}

/// First line inside an expanded command's `<skill>` block.
const SKILL_PREAMBLE: &str = "Follow these instructions for the request after this block.";
/// The request line of a command sent without one.
const NO_REQUEST: &str = "Follow the skill's instructions.";

/// A `/name request` message, expanded by [`SkillSet::expand_command`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCommand {
    /// The skill's name.
    pub skill: String,
    /// What the user asked, without the command; may be empty.
    pub request: String,
    /// The message to send: the skill's instructions, then the request.
    pub message: String,
}

/// The skill name and request of a message made by
/// [`SkillSet::expand_command`], for showing it as the user typed it.
pub fn parse_expanded(message: &str) -> Option<(&str, &str)> {
    let rest = message.strip_prefix("<skill name=\"")?;
    let (name, _) = rest.split_once("\">\n")?;
    let close = "\n</skill>\n\n";
    let at = message.rfind(close)?;
    let request = &message[at + close.len()..];
    Some((name, if request == NO_REQUEST { "" } else { request }))
}

/// Collapse whitespace and cut to `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match joined.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &joined[..cut]),
        None => joined,
    }
}

#[cfg(feature = "tools")]
pub use tool::SkillTool;

#[cfg(feature = "tools")]
mod tool {
    use async_trait::async_trait;
    use serde_json::json;

    use super::SkillSet;
    use crate::error::{CoreError, CoreResult};
    use crate::tools::{Tool, ToolContext, ToolOutput};

    /// The `load_skill` tool: returns a skill's instructions, or one of its files.
    #[derive(Debug, Clone)]
    pub struct SkillTool {
        pub(super) skills: SkillSet,
    }

    impl SkillTool {
        /// The tool's name, `load_skill`.
        pub const NAME: &'static str = "load_skill";
    }

    #[async_trait]
    impl Tool for SkillTool {
        fn name(&self) -> &str {
            Self::NAME
        }

        fn label(&self) -> &str {
            "Load skill"
        }

        fn description(&self) -> &str {
            "Load a skill's instructions by name. Set `file` to read one of the files the \
             skill lists instead."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            let names: Vec<&str> = self
                .skills
                .skills()
                .iter()
                .map(|s| s.name.as_str())
                .collect();
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "enum": names, "description": "The skill to load." },
                    "file": { "type": "string", "description": "A file of the skill, by the path it lists. Leave out to load the instructions." }
                },
                "required": ["name"]
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
            _ctx: &ToolContext,
        ) -> CoreResult<ToolOutput> {
            let name = args["name"]
                .as_str()
                .ok_or_else(|| CoreError::Tool("missing 'name'".into()))?;
            let file = args["file"].as_str();
            let skills = self.skills.clone();
            let (n, f) = (name.to_string(), file.map(String::from));
            let content = tokio::task::spawn_blocking(move || skills.load(&n, f.as_deref()))
                .await
                .map_err(|e| CoreError::Tool(e.to_string()))?
                .map_err(|e| CoreError::Tool(e.to_string()))?;
            Ok(ToolOutput::new(content).with_details(json!({ "skill": name, "file": file })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter() {
        let (yaml, body) = split_frontmatter("---\nname: a\n---\n# Body\n").unwrap();
        assert_eq!(yaml, "name: a\n");
        assert_eq!(body, "# Body\n");
        let (yaml, body) = split_frontmatter("---\r\nname: a\r\n---\r\nbody").unwrap();
        assert_eq!(yaml, "name: a\r\n");
        assert_eq!(body, "body");
        assert!(split_frontmatter("# no frontmatter").is_none());
        assert!(split_frontmatter("---\nname: a\n").is_none());
    }

    #[test]
    fn allowed_tools_accepts_string_or_list() {
        assert_eq!(
            StringOrList::One("Read Grep, Bash".into()).into_vec(),
            ["Read", "Grep", "Bash"]
        );
        assert_eq!(StringOrList::Many(vec!["Read".into()]).into_vec(), ["Read"]);
    }

    #[test]
    fn one_line_collapses_and_cuts() {
        assert_eq!(one_line("a\n  b\tc", 10), "a b c");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }

    #[test]
    fn plain_relative_paths() {
        assert!(is_plain_relative(Path::new("skills")));
        assert!(is_plain_relative(Path::new("./skills/a")));
        assert!(!is_plain_relative(Path::new("../x")));
        assert!(!is_plain_relative(Path::new("/etc")));
        assert!(!is_plain_relative(Path::new("")));
    }
}
