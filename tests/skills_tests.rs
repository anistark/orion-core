//! Tests for loading Agent Skills and the `load_skill` tool.
#![cfg(feature = "skills")]

use std::fs;
use std::path::Path;

use orion_core::skills::{discover_skills, Skill, SkillSet};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A plugin-like folder: two skills under `skills/`, one with a block-scalar
/// description and bundled files.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("skills/uv/SKILL.md"),
        "---\nname: \"uv\"\ndescription: |\n  Manage Python projects\n  with uv.\nallowed-tools: Read Bash\n---\n\n# uv\n\nRun `uv sync`.\n",
    );
    write(
        &root.join("skills/uv/references/commands.md"),
        "uv add <pkg>\n",
    );
    write(&root.join("skills/uv/.hidden"), "secret");
    write(
        &root.join("skills/ruff/SKILL.md"),
        "---\nname: ruff\ndescription: Lint Python with ruff.\n---\nRun `ruff check`.\n",
    );
    write(&root.join("skills/not-a-skill/README.md"), "nothing here");
    dir
}

#[test]
fn discovers_skills_in_the_default_place() {
    let dir = fixture();
    let skills = discover_skills(dir.path(), &[]);
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["ruff", "uv"]);

    let uv = &skills[1];
    assert_eq!(uv.description, "Manage Python projects\nwith uv.");
    assert_eq!(uv.allowed_tools, ["Read", "Bash"]);
    assert_eq!(uv.body().unwrap(), "# uv\n\nRun `uv sync`.\n");
    assert_eq!(uv.files(), ["references/commands.md"]);
}

#[test]
fn discovers_declared_places_and_a_root_skill() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("SKILL.md"),
        "---\nname: root-skill\ndescription: At the root.\n---\nHi\n",
    );
    write(&root.join("extra/one/SKILL.md"), "---\nname: one\n---\n");
    write(&root.join("single/SKILL.md"), "no frontmatter at all\n");

    let skills = discover_skills(root, &["./extra", "single", "../outside"]);
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["one", "single", "root-skill"]);
    assert_eq!(skills[1].description, "");
    assert_eq!(skills[1].body().unwrap(), "no frontmatter at all\n");
}

#[test]
fn skips_a_skill_with_broken_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("skills/bad/SKILL.md"),
        "---\nname: [unclosed\n---\n",
    );
    write(
        &dir.path().join("skills/good/SKILL.md"),
        "---\nname: good\n---\n",
    );
    let names: Vec<String> = discover_skills(dir.path(), &[])
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["good"]);
}

#[test]
fn read_file_stays_inside_the_skill() {
    let dir = fixture();
    let uv = Skill::load(&dir.path().join("skills/uv")).unwrap();
    assert_eq!(
        uv.read_file("references/commands.md").unwrap(),
        "uv add <pkg>\n"
    );
    assert!(uv.read_file("../ruff/SKILL.md").is_err());
    assert!(uv.read_file("/etc/hosts").is_err());
    assert!(uv.read_file("missing.md").is_err());

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            dir.path().join("skills/ruff/SKILL.md"),
            dir.path().join("skills/uv/link.md"),
        )
        .unwrap();
        assert!(uv.read_file("link.md").is_err());
    }
}

#[test]
fn skill_set_indexes_unique_names() {
    let dir = fixture();
    let mut skills = discover_skills(dir.path(), &[]);
    skills.push(skills[0].clone().with_source("dupe"));
    let set = SkillSet::new(skills);
    assert_eq!(set.skills().len(), 2);
    assert!(set.get("ruff").unwrap().source.is_none());

    let section = set.system_section().unwrap();
    assert!(section.contains("`load_skill`"));
    assert!(section.contains("\n- ruff: Lint Python with ruff.\n"));
    assert!(section.ends_with("\n- uv: Manage Python projects with uv."));
    assert!(SkillSet::default().system_section().is_none());

    let loaded = set.load("uv", None).unwrap();
    assert!(loaded.starts_with("# uv"));
    assert!(loaded.contains("- references/commands.md"));
    assert!(set.load("nope", None).is_err());
}

#[test]
fn slash_commands_carry_the_skill_into_the_message() {
    use orion_core::skills::parse_expanded;

    let dir = fixture();
    let set = SkillSet::new(discover_skills(dir.path(), &[]));

    let (skill, request) = set.parse_command("  /uv add requests\nplease").unwrap();
    assert_eq!(skill.name, "uv");
    assert_eq!(request, "add requests\nplease");
    assert_eq!(set.parse_command("/RUFF").unwrap().0.name, "ruff");
    for plain in ["uv add", "/etc/hosts", "/unknown thing", "/", "/uvx run"] {
        assert!(set.parse_command(plain).is_none(), "{plain}");
    }

    let cmd = set.expand_command("/uv add requests").unwrap().unwrap();
    assert_eq!(cmd.skill, "uv");
    assert_eq!(cmd.request, "add requests");
    assert!(cmd.message.starts_with("<skill name=\"uv\">\n"));
    assert!(cmd.message.contains("Run `uv sync`."));
    assert!(cmd.message.contains("- references/commands.md"));
    assert!(cmd.message.ends_with("</skill>\n\nadd requests"));
    assert_eq!(parse_expanded(&cmd.message), Some(("uv", "add requests")));

    let bare = set.expand_command("/ruff").unwrap().unwrap();
    assert_eq!(bare.request, "");
    assert_eq!(parse_expanded(&bare.message), Some(("ruff", "")));

    assert!(set.expand_command("hello").unwrap().is_none());
    assert!(parse_expanded("hello").is_none());
}

#[cfg(feature = "tools")]
#[tokio::test]
async fn load_skill_tool_returns_instructions_and_files() {
    use orion_core::{Tool, ToolContext};
    use serde_json::json;

    let dir = fixture();
    let tool = SkillSet::new(discover_skills(dir.path(), &[])).tool();
    assert_eq!(tool.name(), "load_skill");
    assert_eq!(
        tool.parameters_schema()["properties"]["name"]["enum"],
        json!(["ruff", "uv"])
    );

    let ctx = ToolContext::detached("call-1", "load_skill");
    let out = tool.execute(json!({ "name": "ruff" }), &ctx).await.unwrap();
    assert_eq!(out.content, "Run `ruff check`.\n");

    let file = tool
        .execute(
            json!({ "name": "uv", "file": "references/commands.md" }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(file.content, "uv add <pkg>\n");

    assert!(tool
        .execute(json!({ "name": "uv", "file": "../ruff/SKILL.md" }), &ctx)
        .await
        .is_err());
    assert!(tool.execute(json!({}), &ctx).await.is_err());
}

#[cfg(feature = "tools")]
mod agent_loop {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use orion_core::skills::{discover_skills, SkillSet};
    use orion_core::{
        Agent, AgentConfig, CoreResult, GenerationResult, InferenceParams, LlmBackend, Role,
        TokenCallback,
    };
    use tokio::sync::mpsc;

    /// Replies with each scripted line in turn and checks the last prompt.
    struct Scripted {
        replies: Vec<&'static str>,
        call: AtomicUsize,
        first_prompt: std::sync::Mutex<String>,
    }

    impl LlmBackend for Scripted {
        fn generate(
            &self,
            prompt: &str,
            _params: &InferenceParams,
            _abort: Arc<AtomicBool>,
            mut on_token: TokenCallback,
        ) -> CoreResult<GenerationResult> {
            let idx = self.call.fetch_add(1, Ordering::SeqCst);
            if idx == 0 {
                *self.first_prompt.lock().unwrap() = prompt.to_string();
            }
            let reply = self.replies[idx.min(self.replies.len() - 1)].to_string();
            on_token(&reply, 1, 1.0);
            Ok(GenerationResult {
                text: reply,
                tokens_generated: 1,
                prompt_tokens: 0,
                tokens_per_sec: 1.0,
                time_to_first_token_ms: 1.0,
                generation_time_ms: 1.0,
            })
        }

        fn tokenize_count(&self, text: &str) -> CoreResult<u32> {
            Ok(text.split_whitespace().count() as u32)
        }

        fn is_ready(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn the_agent_loads_a_skill_and_answers_with_it() {
        let dir = super::fixture();
        let skills = SkillSet::new(discover_skills(dir.path(), &[]));
        let mut agent = Agent::new(AgentConfig {
            system_prompt: format!("Be brief.\n\n{}", skills.system_section().unwrap()),
            ..AgentConfig::default()
        });
        agent.set_tools(vec![Box::new(skills.tool())]);

        let backend = Arc::new(Scripted {
            replies: vec![
                "```tool_call\n{\"name\": \"load_skill\", \"arguments\": {\"name\": \"uv\"}}\n```",
                "Run `uv sync`.",
            ],
            call: AtomicUsize::new(0),
            first_prompt: Default::default(),
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let dyn_backend: Arc<dyn LlmBackend> = backend.clone();
        agent
            .prompt("How do I install deps?", dyn_backend, tx)
            .await
            .unwrap();

        let prompt = backend.first_prompt.lock().unwrap().clone();
        assert!(prompt.contains("- uv: Manage Python projects with uv."));
        assert!(prompt.contains("load_skill"));
        let result = agent
            .messages()
            .iter()
            .find(|m| m.role == Role::ToolResult)
            .expect("a tool result");
        assert!(result.content.starts_with("# uv\n\nRun `uv sync`."));
        assert_eq!(agent.messages().last().unwrap().content, "Run `uv sync`.");
    }
}
