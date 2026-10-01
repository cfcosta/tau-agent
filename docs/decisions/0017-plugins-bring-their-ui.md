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
    /// What it knows of one repository. Travels in the catalog as JSON.
    type RepoData: Serialize + DeserializeOwned + Default + Clone;
    /// What the user set. The host saves it under the plugin's name.
    type Settings: Serialize + DeserializeOwned + Default;

    // On the machine that runs agents.
    fn agent_plugin(&self, run: &RunCtx, settings: &Self::Settings)
        -> Option<Box<dyn Plugin>>;
    fn catalog(&self, cx: &HostCx) -> PluginInfo;
    fn repo_data(&self, repo: &RepoCtx, cx: &HostCx) -> Self::RepoData;
    fn act(&self, action: &Value, cx: &HostCx) -> anyhow::Result<()>;

    // Wherever a run is shown: live or stored, desktop or phone.
    fn apply(&self, state: &mut Self::State, record: &Value, run: &mut Anchors);

    // Where its UI goes, and what it is.
    fn ui(&self) -> Manifest<Self>;
}
```

- **`agent_plugin`** builds the plugin for one run. `RunCtx` says what
  the run has: Jev (metered) when there is a key, the repository and
  its directories, and whether it is the main chat or a sub-agent.
  `None` means the plugin is off for the run; its status says why.
- **`catalog`** is its entry on the Plugins screen, "needs a TypeSafe
  key" included. Spend comes from `plugin_costs`, the same for every
  plugin.
- **`repo_data`** is what the plugin shows about one repository: the
  notes, the rules and their settings, a reason they cannot be read.
  It replaces `Catalog.repos[i].memory` and `.constitution`, which
  become `Repo.plugins[name]`.
- **`act`** carries out what the plugin's pages ask for: add a rule,
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
  transcript, and `run.card(call_id).attach(key)` marks a tool card.
- **`ui`** returns the plugin's manifest: everything it adds to the
  interface, and where.

`tau-ui` holds a `Registry` of erased `UiPlugin`s, built in `main.rs`.
The registry converts each plugin's state and repository data between
JSON and their types, and gathers the manifests. The host builds a
run's plugins only through the registry, so a plugin reaches a run only
with its UI.

### Extension points

The interface is extended only at **extension points**. Nothing in it
is a closed list: not the places a plugin can draw, not the navigation,
not the commands. A point has a name and the context each contribution
gets:

```rust
pub struct Point<Cx, Out = AnyElement> { /* a name, such as "tau.sidebar.repo" */ }

pub const SIDEBAR_REPO: Point<RepoNavCx, NavEntry> = Point::new("tau.sidebar.repo");
pub const CARD_BADGE: Point<CardCx> = Point::new("tau.run.card.badge");
```

- **Whoever owns a surface declares its points.** `tau-ui` declares its
  own: the sidebar's main nav and each repository's rows, the phone's
  tabs and Runs list, search, the transcript anchors, a tool card's
  badge, body and tabs, the run banner, the inspector, the status line,
  the plan, the context meter, the title bar's actions, the composer's
  commands, the Models screen's sections. A plugin can declare points
  on its own pages, such as `tau-constitution.rule.sections`, and other
  plugins contribute to them like they do to `tau-ui`'s.
- **A contribution** is the plugin's content for one point: a nav entry,
  an element, a command. It carries an `order` and a `when`, a test on
  its context (for example, only for runs that have a goal), so the
  point decides nothing about who shows.
- **A new surface is a new point.** Adding one is a change to whoever
  owns it, not to every plugin. A contribution to a point nobody
  declares (its owner is not registered) is dropped, and the registry
  logs it once.
- The `Anchors` the fold places are context for the transcript and card
  points: the point gets the anchor's key, and the plugin that placed it
  draws it.

### The manifest

`ui()` declares the plugin's pages, the points it adds, and anything
it contributes, in one place:

```rust
// tau-constitution
Manifest::new()
    .page(Page::new("rules", RulesPage::new).param("repo"))
    .page(Page::new("rule", RulesPage::focused).params(["repo", "rule"]))
    .point(RULE_SECTIONS)                       // others add to a rule's page
    .contribute(SIDEBAR_REPO, |cx| NavEntry::new("Constitution")
        .icon(Icon::Blocked)
        .detail(format!("{} rules", cx.data::<Rules>().rules.len()))
        .badge(waiting(cx))
        .to(Link::page("rules").param("repo", cx.repo)))
    .contribute(CARD_BADGE, checks_badge)
    .contribute(TRANSCRIPT, verdict_note)
    .contribute(INSPECTOR, run_checks)

// tau-memory
Manifest::new()
    .page(Page::new("notes", NotesPage::new).param("repo"))
    .page(Page::new("note", NotePage::new).params(["repo", "note"]))
    .contribute(SIDEBAR_REPO, |cx| NavEntry::new("Memory")
        .icon(Icon::Memory)
        .detail(format!("{} notes", cx.data::<Notes>().notes.len()))
        .to(Link::page("notes").param("repo", cx.repo)))
    .contribute(SIDEBAR, |_| NavEntry::new("Your notes").to(Link::page("notes")))
    .contribute(TRANSCRIPT, suggested_note)
```

- **A nav entry** is shown wherever its point is drawn. `tau-ui` draws
  the sidebar, the phone's tabs and Runs list, and search from the same
  entries, so a plugin is never listed in one place and missing from
  another.
- **A page** is a gpui view the plugin owns (an `Entity` shown as an
  `AnyView`), named within the plugin and opened with parameters. It
  keeps its own UI state: the rule draft, the open tab and the reset
  confirmation leave `Workspace` for `RulesPage`. `tau-ui` makes a page
  the first time it is opened with those parameters and keeps it while
  the window is open, so a tab or a draft survives going back and
  forth. A page supplies its title, for the title bar and history.
- **A link** names a page and its parameters:
  `Link::page("rule").param("repo", r).param("rule", "R2")`, from the
  plugin's own name, or `Link::to("tau-memory", "note")` across
  plugins. Every route a plugin adds is `Route::Plugin { plugin, page,
params }`. It replaces `Route::Memory`, `Route::Constitution`,
  `Route::Plan` and `Route::Ledger`; back, history and the title bar
  treat it like any route. A link to a page that no registered plugin
  has resolves to nothing.
- **`PageCx` and `ViewCx`** are what a page or a contribution can reach:
  its parameters or context, the plugin's repository data and run
  states (for a review queue across runs), `emit(action)` to the host,
  `navigate(link)`, and the kit.
- **Commands** are contributions too: a slash command such as `/goal`,
  or a search action, at the composer's and search's points.

`tau-ui` keeps the frames, and fills them from the manifests: the
sidebar, the tab bar, search, the router, the title bar, the
transcript's layout, the card's shell, and the cost lines, which come
from `PluginCharged`.

### Where positions and drawing split

The fold decides **where** a plugin shows in a run. Anchors are data:
serializable, in order, and the same in history as live. The view
decides **how** it shows, with any element. So the phone can lay out a
run it did not see live, and a plugin's look is its own. Outside a run,
the manifest decides where, and the page or contribution how.

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
- `tau-ui-plugin`: the interface above, `Anchors`, `Point`,
  `Manifest`, `Page`, `NavEntry`, `Link`, `RunCtx`, `RepoCtx`, `HostCx`,
  `PageCx`, `ViewCx`, the points `tau-ui` declares, and the shared
  types (`PluginInfo`, `PluginStatus`, `PlanField`). It depends on `tau-agent`,
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
  `ran_at`, and `Repo` loses `memory` and `constitution`. `RunUpdate`
  loses its plugin variants. `PluginScreen` and the per-plugin routes
  give way to `Route::Plugin`, and `ModelSettings.reasoning` to
  per-plugin settings. The sidebar, search and the phone's Runs tab
  stop naming plugins.
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
  contribution or page is the plugin's.
- Plugins can depend on each other's points by name. A point is part of
  its owner's public API: renaming one, or changing its context, is a
  breaking change for whoever contributes to it.
- `RunView` JSON from before this change does not load. That is
  acceptable: there are no users yet, so nothing is migrated.

### Order

1. Add `PluginCtx::publish`, and make every plugin use it for what it
   reports.
2. Move the theme and components into `tau-ui-kit`, and create
   `tau-ui-plugin` with the registry.
3. Move `tau-reasoning` first. It is the smallest plugin that uses
   most kinds of point: a transcript note with its own body, a plan
   field, a status line, a Models section and a page.
4. Then `tau-goal`, `tau-fast-compaction`, `tau-constitution` and
   `tau-memory`. The last two bring the sidebar's repository rows and
   pages that keep their own state.
5. Then the plugins that draw tool cards: `tau-tools`, `tau-vcs` and
   `tau-compaction`.

## As built

All eight plugin crates now bring their UI. The interface differs from
the sketch above in these places:

- **The methods.** `UiPlugin` has `host`, `agent_plugins`, `starting`,
  `catalog`, `data`, `repo_data`, `act`, `apply`, `new_ui`, `reply`,
  `read_prompt` and `manifest`. `agent_plugins` returns
  `anyhow::Result<Vec<Box<dyn Plugin>>>`: none when the plugin is off
  for the run, and an error fails the run (rules that cannot be read).
  `starting` gives bodies the host folds as a run starts or goes on,
  such as whether the plugin is on and why not. `read_prompt` reads a
  run's prompt as the plugin's own command, for the run's title
  (`/goal`).
- **Window state.** Each plugin has one `Ui` entity per window, made by
  `new_ui` in its own context so it can subscribe to what it makes,
  such as a field's Enter. Pages are functions that draw from a
  `ViewCx`, not entities of their own. Drafts, open tabs and the
  reset confirmation live in the plugin's `Ui`.
- **What a fold reaches.** `RunCx` places anchors (`transcript`,
  `attach`) and marks cards: `mark` (blocked or flagged), `dropped`
  (what a context rewrite dropped), `cut` (a result a plugin cut), and
  `rewrite`, which names one of the plugin's context rewrites so it
  draws it at the `REWRITE` point. History starts at a run's last
  context rewrite; tau-ui folds its stored details as
  `{ "rewrite": details }` once the transcript after it is in place.
- **Where history places a body.** A run's messages are stored when
  its turn ends, after what plugins published during it. A body can
  say where history shows it with the `place` key (`placed`): with
  its turn (no key), before the turn's messages (`now`), or after the
  message the run started on (`message`).
- **Tool cards.** tau-ui keeps a card's frame and what the call sent
  and returned (`CallData`: arguments, updates, result). The tool's
  own plugin draws the rest at the `CARD` point (`CardView`: its head,
  label, edge, body, and whether it folds). tau-tools and tau-vcs draw
  only cards: the host still builds their tools with each run's
  workspace, where they act, which keeps the order of the vcs and
  workspace hooks around jj as it was. Their `agent_plugins` return
  none.
- **Services.** `RunCtx` and `HostCx` carry a typemap of services: the
  metered Jev, how memory searches, and `TurnHooks`, through which a
  plugin hears each turn's commit in the run's workspace (memory marks
  notes stale with it). `HostCx` also reaches the plugins' saved
  settings, which a host half reads and saves when an action changes
  them (tau-mcp's approvals), and tau's config directory.
- **Commands.** A manifest's own slash commands are fixed
  (`SlashCommand`, `/goal`); `Manifest::listed_commands` adds commands a
  plugin lists from its data and the composer's repository's, which
  come and go with it (tau-mcp's prompts).
- **Actions.** A plugin's UI asks through a `Handle`: `act` (its host
  half, answered through `reply`, which gets the UI's context to ask
  for more), `record` (fold and store a change
  the interface makes), `navigate`, `send`, `steer`, `composer`,
  `alert`, `open_run`, `ask_jev_key`, `refresh`, `focus` (give the
  keys to an element the plugin draws, as the window draws next) and
  `cancel` (a run going on).
- **What stays in tau-ui.** The Plan screen, generic plugin notes for
  `Continued` and `PluginError` from plugins without a UI, and the
  landing cards of ADR 0009, which draw changes with tau-vcs's pieces.
  The demo's note suggestions, which no plugin produced, are gone.
