---
title: Skills
description: Offer the model Agent Skills (SKILL.md folders) it loads on demand.
---

Feature `skills` (off by default; adds a YAML parser).

A skill is a folder holding a `SKILL.md`: YAML frontmatter with a `name` and a
`description`, then Markdown instructions, often next to scripts and reference
files. It is the format Claude Code plugins and other agents share.

The model only sees each skill's name and description, so a large collection
stays cheap. When a task matches, it calls the `load_skill` tool to read the
instructions, or one of the skill's files.

```rust
use orion_core::skills::{discover_skills, SkillSet};

let skills = SkillSet::new(discover_skills("/home/me/.agents/skills".as_ref(), &["."]));
let mut system_prompt = String::from("You are a helpful assistant.");
if let Some(section) = skills.system_section() {
    system_prompt.push_str("\n\n");
    system_prompt.push_str(&section);
}
agent.set_system_prompt(system_prompt);
agent.set_tools(vec![Box::new(skills.tool())]); // with the `tools` feature
```

## Finding skills

`discover_skills(root, places)` reads skills the way Claude Code reads a
plugin's `skills` setting:

- each place (relative to `root`, `skills/` when none are given) is a skill
  folder itself, or a folder of skill folders;
- a `SKILL.md` directly in `root` counts too;
- places that leave `root` are ignored, and a skill that fails to load is
  skipped with a warning.

A skill without frontmatter is named after its folder. `Skill::body` returns the
instructions without the frontmatter, `Skill::files` the bundled files, and
`Skill::read_file` reads one, refusing any path that leaves the folder, through
`..` or a symlink.

## The `load_skill` tool

`SkillSet::tool()` returns a `SkillTool` named `load_skill`, with two
arguments:

- `name` - the skill, listed as an enum in the schema so small models pick a
  real one;
- `file` - optional; a file the skill lists, to read instead of the
  instructions.

Loading a skill returns its instructions followed by its file list and folder,
so the model can run its scripts with its other tools. Names are unique within
a `SkillSet`: when two skills share one, the first is kept.

## `/skill` commands

Let users pick a skill themselves: `SkillSet::parse_command("/uv add requests")`
finds the skill a leading `/name` names (exact name, or the same name in
another case) and the request after it. Anything else, such as `/etc/hosts` or
an unknown name, is not a command.

`SkillSet::expand_command(text)` turns such a message into a `SkillCommand`
whose `message` holds the skill's instructions in a `<skill name="…">` block,
then the request. Send it as the user turn: the model follows the skill without
calling `load_skill`, which also helps small models that are unreliable at tool
calls. `skills::parse_expanded(message)` returns the skill and request again,
for showing a saved message the way the user typed it.
