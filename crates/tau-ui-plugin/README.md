# tau-ui-plugin

The interface a tau plugin brings its UI through. In tau, every plugin has
two halves: a `UiPlugin`, which folds what the plugin publishes into state
and says where its UI goes, and a `HostHalf`, which does the plugin's work
on the machine that runs agents. This crate defines both traits, the
extension points the interface offers, and the `Registry` that holds the
plugins. There is no plugin without a UI: `tau-ui` builds a run's plugins
only through the registry.

## What it provides

| Item                                  | What it is                                                                                     |
| ------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `UiPlugin`                            | The UI half: its `State`, `Data`, `RepoData`, `Settings` and `Ui` types, and its `manifest`    |
| `HostHalf`                            | The host half: `agent_plugins` per run, `catalog`, `data`, `repo_data`, `act`, `launcher`      |
| `Fold`                                | A plugin's state in a run, folded from its records with `apply`, live and from history         |
| `PluginHost`, `PluginUi`              | What a plugin keeps on the host, and per window; any `Default` type is both                    |
| `Manifest`, `Page`, `Point`           | Where the UI goes: pages, points a plugin declares, and contributions to points                |
| `SlashCommand`                        | A composer command a plugin adds                                                               |
| `points`                              | The points `tau-ui` declares (`STATUS`, `CARD`, `TRANSCRIPT`, `SIDEBAR`, …) and their contexts |
| `Registry`                            | The plugins, in order: `with` adds a UI half, `host` gives it its host half                    |
| `RunCtx`, `RepoCtx`, `HostCx`         | What a host half gets: the run, its repository, and the host's context                         |
| `RunCx`                               | What a fold reaches in a run: transcript anchors and tool cards                                |
| `ViewCx`, `Handle`                    | What a view gets as it draws, and a window's way back to its plugin                            |
| `PluginValue`                         | A plugin's value with its type erased: typed in use, JSON on the wire                          |
| `Services`                            | Values the host hands plugins by type                                                          |
| `PluginInfo`, `Group`, `Seam`, `Note` | A plugin's entry on the Plugins screen                                                         |
| `placed`, `PLACE`, `REWRITE`          | Keys that say where history shows a published record                                           |

## How it fits

It builds on `tau-agent` (the `Plugin` trait a host half returns),
`tau-ai`, `tau-store` and `tau-ui-kit`. Every plugin crate under
`crates/plugins` implements its traits: the plugin's own crate holds the
`UiPlugin` (and the `HostHalf` when it is light), and a `tau-<plugin>-host`
crate holds a heavy host half.

`tau-ui-remote` lists the plugins with `Registry::with`, UI halves only, so
a phone builds without any host half. `tau-ui` gives them their host halves
with `Registry::host` (`tau_ui::hosted::halves`), and the `tau` app adds the
Luau ones.

## Usage

tau-compaction's two halves, trimmed. The UI half adds a line to a run's
plugin list; the host half builds the agent plugin for each run:

```rust
use tau_agent::plugin::Plugin;
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    Group, HostCx, HostHalf, Manifest, PluginInfo, PluginStatus, RunCtx,
    Seam, UiPlugin, points::{self, AtRun},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct CompactionUi;

impl UiPlugin for CompactionUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        "tau-compaction"
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::STATUS, |_: &AtRun, _| {
            Some(PluginStatus {
                name: "tau-compaction".into(),
                state: "watching the window".into(),
                tone: Tone::Quiet,
            })
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CompactionHost;

impl HostHalf for CompactionHost {
    type Plugin = CompactionUi;
    type Host = ();

    async fn agent_plugins(
        &self,
        _host: &(),
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(vec![Box::new(tau_compaction::Compaction::default())])
    }

    async fn catalog(&self, _: &(), _: &HostCx, _: &()) -> PluginInfo {
        PluginInfo {
            group: Group::Context,
            description: "Summarizes the context when it nears the window"
                .into(),
            seams: vec![Seam::Start, Seam::Rewrite],
            ..Default::default()
        }
    }
}

let registry = tau_ui_plugin::Registry::new()
    .with(CompactionUi)
    .host(CompactionHost);
```

`Registry::host` panics when the plugin is not in the registry yet, or
already has a host half.

## Features

| Feature   | What it adds                                                                               |
| --------- | ------------------------------------------------------------------------------------------ |
| `testing` | `tau_ui_plugin::testing`: `FakeRun`, `run_ctx`, `fold` and `RepoValues`, to test a UI half |

Plugin crates turn it on in their dev-dependencies.

## Testing

```sh
cargo nextest run --release -p tau-ui-plugin
```

The tests are unit tests in `src/` (`registry.rs`, `value.rs`, `host.rs`,
`services.rs`).

## Further reading

- [docs/reference/plugins.md](../../docs/reference/plugins.md), "A plugin's
  UI"
- [docs/decisions/0017-plugins-bring-their-ui.md](../../docs/decisions/0017-plugins-bring-their-ui.md)
- [docs/decisions/0029-every-plugins-settings-in-a-pane.md](../../docs/decisions/0029-every-plugins-settings-in-a-pane.md)
- [docs/decisions/0030-host-halves-are-crates.md](../../docs/decisions/0030-host-halves-are-crates.md)
- [docs/decisions/0006-plugin-crates.md](../../docs/decisions/0006-plugin-crates.md)
- [docs/reference/testing.md](../../docs/reference/testing.md): the `testing`
  feature among the test tools
