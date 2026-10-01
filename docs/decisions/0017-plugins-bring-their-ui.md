# 0017: Plugins bring their own UI

- Status: accepted. Amends [0006](0006-plugin-crates.md) and
  [0007](0007-gpui-interface.md): a plugin crate exports its UI along
  with its agent plugin, and cannot be used without it.
  [0001](0001-library-not-product.md) and [0005](0005-plugins.md) still
  hold for `tau-agent`.
- Date: 2026-10-01

## Context

Each plugin's interface lives in `tau-ui`, spread over seven places:

- **Wiring:** `host.rs` builds each plugin for a run, from the saved
  settings and the TypeSafe key.
- **Folding:** `view.rs` turns each plugin's reports into state, in a
  branch per plugin name. `stored_view` and `RunView::from_timeline`
  do the same for history, with their own rules.
- **Shared surfaces:** notes in the transcript, marks on tool cards,
  plan fields, plugin status lines, the context meter's trigger.
- **Screens and panels:** Constitution, Ledger, Plan, Memory, the goal
  banner and tab.
- **Settings and controls:** a section of the Models screen, `/goal`,
  the rule editor.
- **The catalog entry** on the Plugins screen.
- **Actions:** `WorkspaceEvent`s such as `AddRule`, `Goal` and
  `ResetRules`, which the host carries out.

`view.rs` names the plugins about 70 times and `host.rs` about 40. A
review of the four plugins that ask Jev (2026-09-30) found about 30
gaps between what they do and what the interface shows. Most were of
two kinds:

- **Live and stored runs drifted apart.** A plugin reported something
  but did not record it, or tau-ui folded it live and dropped it from
  history. Errors, ledgers and goal notes went missing when a run was
  opened again.
- **The interface lagged behind the plugin.** Settings the plugin took
  had no control (`redecide`, `on_error`, `max_holds`). A plugin was
  missing from the catalog. Values only the demo filled in.

Both come from keeping a plugin's interface away from the plugin.
Adding a plugin, or changing what one reports, means editing tau-ui in
several places that nothing ties together.

The phone ([0013](0013-phones-connect-to-a-running-tau.md)) adds one
constraint. It gets `RunView`s and `RunEvent`s over the wire and draws
them with the same gpui code, but it has no store, no Jev and no
agents.

## Decision

### Every plugin has a UI

A plugin crate in `crates/plugins` exports one value: a `UiPlugin`,
which holds the agent plugin, the plugin's view of a run, and its
screens. There is no headless form. The `tau_agent::plugin::Plugin`
it builds is an implementation detail: it is not public, and nothing
but the `UiPlugin` builds it.

This covers every plugin crate: `tau-tools`, `tau-vcs`,
`tau-compaction`, `tau-fast-compaction`, `tau-reasoning`, `tau-memory`,
`tau-constitution` and `tau-goal`. `tau-jev` is a library that plugins
use, not a plugin, and stays as it is.

`tau-agent` does not change. Its `Plugin` trait has no UI, and no core
crate depends on gpui or on a plugin. A program that uses `tau-agent`
alone writes its own plugins.

### The interface

A new crate, `tau-ui-plugin`, defines the interface that `tau-ui` and
every plugin crate share:

```rust
pub trait UiPlugin: 'static {
    fn name(&self) -> &'static str;
    /// What the plugin knows of one run. Travels in `RunView` as JSON.
    type State: Serialize + DeserializeOwned + Default + Clone;
    /// What the user set. The host saves it under the plugin's name.
    type Settings: Serialize + DeserializeOwned + Default;

    // On the machine that runs agents.
    fn agent_plugin(&self, run: &RunCtx, settings: &Self::Settings)
        -> Option<Box<dyn Plugin>>;
    fn catalog(&self, cx: &HostCx) -> PluginInfo;
    fn act(&self, action: &Value, cx: &HostCx) -> anyhow::Result<()>;

    // Wherever a run is shown: live or stored, desktop or phone.
    fn apply(&self, state: &mut Self::State, record: &Value, run: &mut Anchors);

    // How it looks.
    fn render(&self, slot: Slot<'_>, state: &Self::State, cx: &mut ViewCx)
        -> Option<AnyElement>;
    fn commands(&self) -> &[SlashCommand] { &[] }
}
```

- **`agent_plugin`** builds the plugin for one run. `RunCtx` says what
  the run has: Jev (metered) when there is a key, the repository and
  its directories, and whether it is the main chat or a sub-agent.
  `None` means the plugin is off for the run; its status says why.
- **`catalog`** is its entry on the Plugins screen, "needs a TypeSafe
  key" included. Spend comes from `plugin_costs`, the same for every
  plugin.
- **`act`** carries out what the plugin's screens ask for: add a rule,
  pause a goal, remove a constitution that cannot be read. The
  interface sends `WorkspaceEvent::Plugin { plugin, action }`, which
  replaces the per-plugin events. `HostCx` is the only part of the
  host a plugin can reach: the store (its records, across runs too),
  the run's directories, the metered Jev and the key. A failed action
  comes back to the interface as an alert.
- **`apply`** folds one of the plugin's records into its state. It is
  plain Rust, with no gpui and no store, so the phone runs it too. It
  places anchors in the run: `run.transcript(key)` puts
  `Item::Plugin { plugin, key }` at the current point of the
  transcript, `run.card(call_id).attach(key)` marks a tool card, and
  `run.status`, `run.plan_field` and `run.context_trigger` set the
  shared lines.
- **`render`** draws the plugin wherever there is a slot for it, with
  any gpui it likes. The slots are:
  - `Transcript { key }`
  - `CardBadge`, `CardBody` and `CardTab { call_id }`
  - `RunBanner`
  - `Inspector`
  - `Status`
  - `PlanField`
  - `ContextMeter`
  - `Screen { arg }`, reached through `Route::Plugin { name, arg }`
  - `Settings`, a section of the Models screen

  `tau-ui` keeps the frames: the transcript's layout, the card's
  shell, routing, and the cost lines, which come from `PluginCharged`.

- **`commands`** are its slash commands, such as `/goal`.

`tau-ui` holds a `Registry` of erased `UiPlugin`s, built in `main.rs`.
The registry converts each plugin's state between JSON and
`State`. The host builds a run's plugins only through the registry,
so a plugin reaches a run only with its UI.

### Where positions and drawing split

The fold decides **where** a plugin shows. Anchors are data:
serializable, in order, and the same in history as live. The view
decides **how** it shows, with any element. So the phone can lay out a
run it did not see live, and a plugin's look is its own.

### State as JSON

`RunView.plugins` is a `BTreeMap<String, serde_json::Value>`, one
entry per plugin, which `UiPlugin::State` reads and writes. The wire
and snapshot formats know no plugin types, so a phone build and a
desktop build can carry different plugin sets without a deserialize
failure.

### Live and stored runs are one fold

`tau-agent` gets `PluginCtx::publish(body)`, which reports a body and
records it with the run in one call. Plugins use it for everything an
interface shows. `UiPlugin::apply` folds only published bodies:

- live, from each `PluginReport` as it arrives;
- from history, from the run's records in the order they were stored.

So a stored run is a replay of the live one by construction.
`PluginCtx::report` stays for what must not outlast the run, if that
turns out to exist. Rewrites go through the same fold: the host hands
a `context` entry's details to the plugin that wrote it.

### Crates

- `tau-ui-kit`: theme tokens, components, icons and markdown, moved out
  of `tau-ui`. It keeps the rule that screens never set their own sizes
  or colors (`tests/design.rs`), and plugins follow it too.
- `tau-ui-plugin`: the interface above, `Anchors`, `Slot`, `RunCtx`,
  `HostCx`, `ViewCx`, and the shared types (`PluginInfo`,
  `PluginStatus`, `PlanField`). It depends on `tau-agent`,
  `tau-store`, gpui and `tau-ui-kit`.
- Each plugin crate depends on `tau-ui-plugin`. Its UI is part of the
  crate, not behind a feature.
- `tau-ui` depends on the plugin crates and wires them into its
  registry. No plugin depends on `tau-ui`.

## Consequences

- A plugin is written, reviewed and tested in one place. Its fold is
  tested without a window, and its screens with gpui's test app, in
  its own crate.
- Most of the per-plugin code in `view.rs`, `host.rs`, `slash.rs`,
  `rule_editor.rs`, `goal.rs` and `ui/screens/` moves into the plugins.
  `RunView` loses `constitution`, `goal`, `goal_checks`, `ledger` and
  `ran_at`. `RunUpdate` loses its plugin variants. `PluginScreen`
  gives way to `Route::Plugin`, and `ModelSettings.reasoning` to
  per-plugin settings.
- Every plugin crate compiles gpui. That includes their in-the-loop
  tests, and the phone, which builds the agent-side code it never
  runs. If that gets slow, that code can be left out of the Android
  build with `cfg`; whether to is left open.
- Code that drives a plugin without the interface changes. The
  `output-pruning` eval and `tau-reasoning`'s `replay` example go
  through the `UiPlugin`. A plugin's own tests can still reach its
  internals.
- Arbitrary gpui in shared places gives up a single look. The kit and
  the design test keep sizes and colors in common; the layout inside a
  slot is the plugin's.
- `RunView` JSON from before this change does not load. That is
  acceptable: there are no users yet, so nothing is migrated.

### Order

1. Add `PluginCtx::publish`, and make every plugin use it for what it
   reports.
2. Move the theme and components into `tau-ui-kit`, and create
   `tau-ui-plugin` with the registry.
3. Move `tau-reasoning` first. It is the smallest plugin that uses
   nearly every slot: a transcript note with its own body, a plan
   field, a status line, a settings section and a screen.
4. Then `tau-goal`, `tau-fast-compaction`, `tau-constitution` and
   `tau-memory`.
5. Then the plugins that draw tool cards: `tau-tools`, `tau-vcs` and
   `tau-compaction`.
