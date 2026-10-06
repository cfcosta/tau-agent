# tau-skills

Skills: instructions an agent loads when a task calls for them. A skill
is a folder under the person's skills folder (`~/.agents/skills`, the
one other agents read too) holding a `SKILL.md`: YAML frontmatter with a
`name` and a `description`, then the instructions. A run's instructions
list each skill by name and description. The model loads one with the
`skill` tool when a task fits it, and reads or runs the folder's other
files with the usual tools. The list costs a line per skill; a skill's
instructions cost nothing until it is loaded.

## What it provides

| Item                                      | What it is                                                                                    |
| ----------------------------------------- | --------------------------------------------------------------------------------------------- |
| `Skill`                                   | A skill the model can load: name, description, folder, file count, ignored keys               |
| `Problem`                                 | A folder that looks like a skill but is not offered, and why                                  |
| `Skills`                                  | What the folders hold: the skills by name, and the problems                                   |
| `Loaded`                                  | What a `skill` call returns in its details, for its card                                      |
| `SkillsDir`                               | A host service: where the host reads skills from                                              |
| `NAME`, `TOOL`                            | `"tau-skills"` and `"skill"`                                                                  |
| `SKILL_FILE`, `SKILLS_DIR`, `BUILTIN_DIR` | `SKILL.md`, `.agents/skills` under home, and `skills` under tau's data directory              |
| `SkillsUi`                                | The plugin's `UiPlugin`: the Skills screen, the `skill` card, `/name` commands                |
| `scan`                                    | Reading a folder: `scan`, `scan_all`, `section` and the name and size limits (feature `host`) |
| `host::SkillsPlugin`                      | The agent plugin: the list in the run's instructions, and the tool (feature `host`)           |
| `SkillsHost`                              | The `HostHalf` for tau's plugin registry (feature `host`)                                     |
| `demo::seed`                              | Writes the demo's skills folder (feature `demo`)                                              |

A message that starts with `/name` asks for that skill. Frontmatter
keys tau does not act on, such as `allowed-tools`, are kept in
`Skill::ignored`. Plugin crates can ship built-in skills under tau's
data directory (`BUILTIN_DIR`); a skill of the person's with the same
name is offered instead.

## How it fits

tau-skills is a single crate holding both halves. The shapes and the
views always build. The host half (reading the folder, the list, the
tool) sits behind the default `host` feature, so `tau-ui-remote` and a
phone build it with `default-features = false` (decisions 0017, 0030).

- Builds on `tau-agent` (`Plugin`, `AgentTool`), `tau-ui-plugin` and
  `tau-ui-kit`.
- Used by `tau-ui` (which registers `SkillsHost` and gives it a
  `SkillsDir`), `tau-ui-remote` (which lists `SkillsUi`), and
  `tau-luau-plugins-host`, which installs its built-in skill under
  `BUILTIN_DIR`.

`SkillsHost` reads the folders as each run starts. With no skills, the
run gets neither the list nor the tool. A skill added during a run is
listed the next time a run starts.

## Usage

```rust
use std::path::Path;
use tau_agent::agent::Agent;
use tau_skills::{host::SkillsPlugin, scan};

let skills = scan::scan(Path::new("/home/me/.agents/skills"));
for problem in &skills.problems {
    eprintln!("{}: {}", problem.dir.display(), problem.reason);
}
let agent = Agent::new(llm).plugin(SkillsPlugin::new(skills));
```

A missing folder holds no skills. `scan::scan_all` adds the built-in
skills from a second folder.

## Features

| Feature | Default | Effect                                                                        |
| ------- | ------- | ----------------------------------------------------------------------------- |
| `host`  | on      | The agent half: `scan`, `host` and `SkillsHost`, with serde_yaml_ng and tokio |
| `demo`  | off     | `demo::seed`, the skills tau-ui's demo screens show                           |

## Testing

```sh
cargo nextest run --release -p tau-skills
```

- `tests/scan.rs` is a Hegel property test: every folder written is a
  skill or a problem, never both and never lost, however the YAML
  writes the frontmatter.
- `tests/tool.rs` runs the tool inside real runs with
  `ScriptedModel`, an in-memory store and a temporary skills folder.

## Further reading

- [Plugins: a plugin's UI](../../../docs/reference/plugins.md)
- [0027: Luau plugins, written and rewritten through tau](../../../docs/decisions/0027-luau-plugins.md),
  for built-in skills
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
