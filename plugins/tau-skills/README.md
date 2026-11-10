# tau-skills

Skills: instructions an agent loads when a task calls for them. A skill
is a folder in the person's skills folder (`~/.agents/skills`, the one
other agents read too) holding a `SKILL.md`: YAML frontmatter with a
`name` and a `description`, then the instructions. A run's instructions
list each skill by name and description. The model loads one with the
`skill` tool when a task fits it, and reads or runs the folder's other
files with the usual tools. The list costs a line per skill; a skill's
instructions cost nothing until it is loaded.

A Luau package of tau's own (ADR 0050, gap K).

| File                  | What it is                                                                                     |
| --------------------- | ---------------------------------------------------------------------------------------------- |
| `src/plugin.luau`     | The plugin: `start`'s list, the `skill` tool and its card, `app`, `page`, the `/name` commands |
| `src/scan.luau`       | Reading the folders: what makes a skill, why a folder is not one, and the list                 |
| `skills/`             | The skills tau ships, read where the package is installed                                      |
| `tests/scan.luau`     | The folders' rules, as cases and properties, on fake files                                     |
| `tests/plugin.luau`   | The run, the page and the commands, on fake files                                              |
| `tests/loop.luau`     | The list and the tool in a real run                                                            |
| `tests/fixtures.luau` | The demo's run that loaded a skill                                                             |

Skills named `principle-*` and `playbook-*` are listed apart, under
their own headings after the rest: the principles as an index the model
reads before it applies one, the playbooks as procedures it follows
step by step (`scan.KINDS`). A message that starts with `/name` asks for
that skill, and each skill but the principles is a `/name` command in
the composer. Frontmatter keys tau does not act on, such as
`allowed-tools`, show on the Skills page.

## Where skills come from

`uses.files` names three folders: `skills`, the person's, as the host
has it; `package/skills`, those this package ships; and `data/skills`,
those other plugins put in tau's data folder (tau-plugins' and
tau-agents' own). A skill of the person's with the same name as one tau
ships is offered instead. The folders are read as each run starts, so a
skill added during a run is listed the next time; `app` reads them again
for the sidebar's count, the page and the commands, at most once a
second.

## The skills it ships

`skills/` holds 24 engineering principles (`principle-*`), playbooks for
kinds of task (`playbook-*`), and procedures such as `how`, `why`,
`architect`, `arena`, `interrogate` and `humanizer`. They are adapted
from pstack and humanizer (see THIRD_PARTY_NOTICES.md) to tau's tools:
sub-agents, commits and landing, `ask`, the canvas, `schedule` and
`/goal`. A skill is added by adding its folder.

## Testing

```sh
target/release/tau-plugin test plugins/tau-skills   # its Luau tests, no compiling
target/release/tau-plugin test plugins/tau-skills --bless   # writes its fixtures anew
cargo nextest run --release -p tau-built-in-tests -E 'test(/^skills::/)'
```

The first runs its cases, properties and loops, and checks the fixture
`tests/fixtures.luau` makes. The second set reads real folders through
the host's YAML parser: every folder written is a skill or a problem,
never both and never lost, however the frontmatter is written; every
skill the package ships is offered, every skill and reference it names
is there, and none assumes one language or another harness.

## Further reading

- [0050: tau's own plugins, in Luau](../../docs/decisions/0050-tau-s-plugins-in-luau.md), gap K
- [Plugins: Luau packages](../../docs/reference/plugins.md)
