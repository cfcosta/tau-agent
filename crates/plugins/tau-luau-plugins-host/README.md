# tau-luau-plugins-host

The host half of Luau plugins. It reads tau's plugins repository, loads
each plugin folder, tests it, and runs its hooks at tau-agent's seams,
each call in a fresh codemode VM with codemode's limits. It also ships
the `tau-plugins` skill, which teaches a model to write, test and land
a plugin, and the `plugin_test` tool a run in the plugins repository
uses.

## What it provides

| Item                        | What it is                                                       |
| --------------------------- | ---------------------------------------------------------------- |
| `LuauPluginsHost`           | The `HostHalf` the app registers: the plugins for each run       |
| `agent::LuauPlugins`        | The run's active Luau plugins, as one tau-agent `Plugin`         |
| `agent::Active`             | A loaded plugin with the person's settings                       |
| `runtime::Files`            | A plugin folder: `plugin.luau`, `lib/`, `tests/`, README         |
| `runtime::load`             | Reads what a folder declares, in a fresh VM, and checks it       |
| `runtime::Loaded`           | A checked plugin: `call` runs a hook, `test` runs its tests      |
| `runtime::Hook`             | A hook as the host calls it, with its time limit (`Hook::limit`) |
| `runtime::Reach`, `NoReach` | What a hook may reach beyond its VM: tools, Jev                  |
| `registry::Registry`        | The active plugins, read from the repository's trunk             |
| `testing::PluginTesting`    | Adds the `plugin_test` tool to a run                             |
| `skill`                     | The `tau-plugins` skill: `render`, `install`, `EXAMPLES`         |

`LuauPlugins` applies each hook's answer. A block stops the call, a
continue keeps the run going (at most `MAX_CONTINUATIONS` times), and
the plugin's state, log lines, errors and view are published as
`tau_luau_plugins::Record`s. A failing hook counts as allowing or
stopping. After `MAX_FAILURES` failures its plugin is off for the rest
of the run.

`Registry` watches trunk. When it moves, each plugin is loaded and
tested again. A version activates by itself unless its tests fail, it
does not load, or it reaches further than the version the person
allowed; then the version before stays active.

## How it fits

This is the host half (decisions 0027 and 0030). Its partner is
`tau-luau-plugins`, the interface half, which holds `Declaration`,
`Record`, the Plugins page and the view drawing.

It builds on `tau-agent`, `tau-codemode` and `tau-codemode-host` (the
sandbox), `tau-jev`, `tau-ui-plugin`, `tau-vcs-host` (reading trunk)
and `tau-skills` (where the skill is installed). Because it links Luau
through `tau-codemode-host`, only the `tau` app depends on it: it
registers `LuauPluginsHost` in `crates/tau/src/lib.rs`.

`LuauPluginsHost` gives each run the active plugins with the person's
settings over their defaults, and Jev when the run has it. A run in the
plugins repository also gets `PluginTesting`.

## Usage

Load one plugin folder and add it to an agent:

```rust
use std::path::PathBuf;
use serde_json::json;
use tau_agent::agent::Agent;
use tau_luau_plugins_host::{
    agent::{Active, LuauPlugins},
    runtime::{Files, load},
};

// `Files::read` blocks, so it runs on the blocking pool.
let dir = PathBuf::from("plugins/no-friday-deploys");
let files = tokio::task::spawn_blocking(move || Files::read(&dir)).await??;
// The plugin's `name` must be its folder's.
let loaded = load("no-friday-deploys", files).await?;
let settings = loaded.declaration.default_settings();

let plugins = LuauPlugins::new(
    vec![Active { loaded, settings }],
    json!({ "kind": "main", "repo": "my-repo", "model": "gpt-5.5" }),
    None, // no Jev
);
let agent = Agent::new(llm.clone()).plugin(plugins);
```

Build one `LuauPlugins` per run. `Loaded::test` runs the folder's
`tests/` files against fake runs and returns a `TestResult` per case.

## Testing

```sh
cargo nextest run --release -p tau-luau-plugins-host
```

- `tests/runtime.rs`: loading a plugin and calling its hooks, state,
  reach and failures.
- `tests/agent.rs`: plugins in real runs, driven by `tau-testing`'s
  `ScriptedModel` and an in-memory `tau-store-sqlite` store.
- `tests/registry.rs`: activation, waiting and fallback against a
  temporary jj repository.
- `tests/skill.rs`: the skill's three worked plugins in
  `skill/examples` load and pass their own tests.

Nothing needs network or credentials. The first build compiles Luau.

## Further reading

- [0027: Luau plugins, written and rewritten through tau](../../../docs/decisions/0027-luau-plugins.md)
- [0029: Every plugin's settings in a pane of their own](../../../docs/decisions/0029-every-plugins-settings-in-a-pane.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
- [Codemode reference](../../../docs/reference/codemode.md), for the
  sandbox the hooks run in
- [`skill/SKILL.md`](skill/SKILL.md), the skill tau installs
