# 0045: Everything is a plugin: a kernel, slots, and one interface

- Status: proposed. Amends [0005](0005-plugins.md): the loop is the
  kernel's default for a slot, and every seam is open to every plugin.
  Amends [0017](0017-plugins-bring-their-ui.md): plugins draw one view
  tree, not arbitrary gpui. Amends [0027](0027-luau-plugins.md) and
  [0033](0033-plugins-are-live.md): a Luau plugin reaches everything a
  Rust plugin does, behind capabilities. Amends
  [0030](0030-host-halves-are-crates.md): the host builds nothing a
  plugin could. Amends [0002](0002-openai-websocket-only.md) and
  [0012](0012-chatgpt-sign-in-only.md): OpenAI's WebSocket and ChatGPT
  sign-in are the default provider, not the only one.
- Date: 2026-11-03

## Context

tau has about 25 plugin crates, and still keeps much of what it does
out of them:

- **The host builds plugins' parts.** `tau-tools` and `tau-vcs` are
  plugins in name: their `agent_plugins` return nothing, and
  `tau-ui/src/host.rs` (`agent_for_run`, lines 851–1063) builds their
  tools, the run's jj workspace, `vcs_land` or its refusal, and
  `spawn`. The main chat's prompt names `vcs_commit` and
  `exec_command`.
- **Whole features live in core.** GitHub (sign-in, clone, push, pull
  requests, about 2.8k lines), the ChatGPT account and model screens,
  Jev's key and metering, phones (about 3k lines), desktop notices, run
  titles, idle compaction, AGENTS.md, artifact maintenance, the store's
  SQL box, and the sidebar sections for agents and the plugins
  repository.
- **Core names plugins.** About 80 places in `tau-ui` and
  `tau-ui-remote` name a plugin: `tau_vcs` in landing, lanes, pushes
  and the transcript; `tau-agents` in the sidebar and notices; `tau-ask`
  filtered out of agent runs; the memory plugin's search mode chosen by
  the host; a goal icon in the transcript.
- **Order is a comment.** `tau-ui-remote/src/plugins.rs` lists plugins
  by hand, and its comments carry the rules: pruning before summary,
  mcp before codemode. When two plugins want one role, the first to
  answer wins. Nothing says which roles exist or that one plugin fills
  each.
- **Luau plugins are a lesser tier.** They reach about a third of the
  run seams and almost none of the host's or the interface's (the gap
  table is below). Their views draw only on the status line and tool
  cards; `note`, `page` and buttons are stored and never shown.
- **Two UI models.** Rust plugins draw any gpui on shared surfaces
  (0017); Luau plugins return a 17-piece JSON tree. The phone draws
  Rust plugins' gpui because it links their crates, and cannot run Luau
  at all.

The person wants tau to be a small kernel with everything else as
plugins, and wants a plugin they write in tau to do anything a plugin
compiled into tau does.

## Decision

### The kernel

The kernel is what stays when every plugin is removed:

- **The frame and navigation:** the window, sidebar, router, search,
  composer, transcript layout, the tool card's shell, the inspector's
  frame, the Plugins screen, and onboarding's shell.
- **The lifecycle of runs and chats:** repositories as a list, the main
  chat, chats, sub-agents, steering, queues, landing a child on its
  parent, crash recovery, attention and the crew. Landing is the
  kernel's; how a change is made, merged and pushed is the VCS slot's.
- **The plugin machinery:** the registry, the slot table, the
  capability gate, the view-tree renderer and its widgets, record
  folding for live and stored runs.
- **Defaults for two slots:** the agent loop (`tau-agent`'s runner) and
  the store interface (`tau-store`'s `Backend`).

The kernel names no plugin. A test in `tau-ui-remote` and `tau-ui`
checks that no non-test source mentions a plugin crate or a plugin's
record or tool name; the registration list in `tau-plugins` is the one
place they are named. `tau-ui-remote`'s holds now, with tau-phone's
sources in it too, and checks that neither links a plugin crate;
`tau-ui`'s follows the slots.

### Slots

A **slot** is a role the kernel declares and exactly one plugin fills.
The kernel calls the slot, never the plugin. Slots are declared in
`tau-ui-plugin`, so the phone and the desktop see the same table; host
slots are filled only on the machine that runs agents.

```rust
pub struct Slot<T: ?Sized> { /* a name, such as "tau.provider" */ }

pub const PROVIDER: Slot<dyn Provider> = Slot::new("tau.provider");
pub const VCS: Slot<dyn Vcs> = Slot::new("tau.vcs");
```

| Slot             | What fills it                                                                   | Default filler                   | When it changes |
| ---------------- | ------------------------------------------------------------------------------- | -------------------------------- | --------------- |
| `tau.provider`   | sign-in, model catalog, `Llm`, plan limits and refusals                         | `tau-chatgpt` (OpenAI WebSocket) | per run         |
| `tau.side-model` | a small model plugins ask (today's `Jev`)                                       | `tau-jev`                        | per call        |
| `tau.store`      | `Backend`, opened at startup                                                    | `tau-store-sqlite`               | at startup      |
| `tau.loop`       | drives one run: turns, tool calls, seams, stored writes                         | `tau-agent`'s runner (kernel)    | per run         |
| `tau.vcs`        | open and clone repositories, a workspace per run, a commit per turn, land, push | `tau-vcs` (jj)                   | per repository  |
| `tau.compaction` | `rewrite_context`, `start_idle`, idle compaction before the cache lapses        | `tau-compaction-chain` (below)   | per run         |
| `tau.title`      | a run's title from its first message                                            | `tau-titles`                     | per run         |
| `tau.notify`     | where notices go when the window is not in front                                | `tau-notify` (desktop)           | at once         |

- **Filling a slot.** A plugin's declaration says which slots it can
  fill. If one plugin can, it does. If several can, the first in the
  slot's declared order fills it (the order is declared beside the
  registration list, in `tau-plugins`; the order plugins are registered
  in never decides), until the person picks another on the Plugins
  screen. The pick is the interface's state, so phones show it and it
  outlives a restart, and it fills the slot from then on. If none can,
  the kernel's default fills it, or the feature is off and says so (no
  VCS: chats do not land).
- **Filling a slot needs the person's Allow**, for every plugin that is
  not compiled into tau (see Capabilities), because a slot reaches
  everything its role touches.
- **A slot is a trait, not a hook list.** Its methods are object-safe
  (`async_trait` or boxed futures, ADR 0028), so the kernel holds
  `Arc<dyn Vcs>`. The Luau host implements each trait by calling the
  plugin's table.
- **Compaction keeps its chain.** Today fast pruning, the tree and the
  summary run in order, each declining when it would save too little.
  The default filler, `tau-compaction-chain`, is that chain as one
  plugin; a person who wants only the summary picks `tau-compaction`.
- **Composition is not a slot.** Roles where every plugin adds
  something stay ordered lists: tools, tool sources, launchers (0025),
  prompt fragments, `before_tool` checks, records and points.

The `Provider` slot leaves the OpenAI shape out of `tau-ai`'s traits:
`Settings` becomes a neutral type (model, effort, tools, instructions,
cache key), and the delta rule (0005, 0022) becomes the provider's
promise: given unchanged settings and a growing transcript, it may
continue its previous response. A provider that cannot, resends.

The `Loop` slot comes last and is the narrowest: a filler gets the same
`RunPlan`, plugins, store and control as the runner, and must keep the
runner's invariants: one stored write per turn, no call without a
result, steering, a rewrite costs one resend. The kernel's tests for
those invariants run against every filler.

### One plugin interface

Rust and Luau plugins implement one interface. Rust plugins implement
its traits; the Luau host implements them for each Luau plugin by
calling its table. Every seam is in both, and a table of seams in the
reference doc is checked by a test against the Luau binding, so they
cannot drift again.

A plugin **declares**, in one place:

- `name`, `description`, `after` / `before` (names it must come after or
  before; replaces the comments in `plugins.rs`; a cycle fails the
  registry with both names).
- `slots`: what it can fill.
- `provides`: services other plugins can ask for (today `HostCx`'s
  typemap, filled by the host). `uses`: the services, tools and
  capabilities it reaches.
- `settings`: a schema, with defaults.
- `manifest`: pages, points it declares, contributions, commands, keys.

It **implements**, as it needs:

| Group        | Seams                                                                                                                                                                                             |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Run assembly | `workspace` (from the VCS slot), `tools` with state per workspace that outlives a turn, `tool_source`, `prompt(run_kind)`, `launcher`                                                             |
| Run          | `start` (`RunPlan`), `before_request`, `before_tool` (may change arguments), `after_tool_result` (may rewrite), `on_event`, `rewrite_context`, `rewritten`, `before_stop`, `finish`, `start_idle` |
| Records      | `publish`, `record`, `report`; and `fold(state, record, run)` with anchors, marks, drops and cuts                                                                                                 |
| Host         | `host_state`, `catalog`, `data`, `repo_data`, `act`, `jobs`, `resume`, `setup`, `lands_itself`, `read_prompt`                                                                                     |
| App          | `on_update` (every `HostUpdate`), `send` (a `WorkspaceEvent`), window focus; needed by phones and notices                                                                                         |
| Repositories | repository kinds: list, create, first files, a sidebar section. Replaces `Own::Agent` and `Own::Plugins`                                                                                          |
| View         | `view(point, cx) -> Node`, for every point                                                                                                                                                        |

`tau-agent` keeps its `Plugin` and `PluginRun` traits for programs that
use it alone (0001); `tau-ui-plugin`'s interface builds on them.

### One view tree, for every plugin

A plugin's UI is a `Node` tree: plain data, `serde`, the same on the
desktop and the phone. The kernel's renderer draws it with
`tau-ui-kit`. No plugin draws gpui. That makes Luau's UI equal to
Rust's, lets the phone draw every plugin without linking a plugin
crate's UI, and keeps one look (the design test checks the renderer,
not every plugin).

- **Pieces.** Today's 17, plus what the plugins' UIs use now: headings,
  icons, links (`Link::page`), markdown, rich text, selectable text,
  code with syntax highlighting, tables, key-value rows, meters, chips,
  tooltips, sections that open and close, tabs, scroll areas, modals,
  popovers and menus, images by blob, and text inputs (one line,
  several lines, submit).
- **Widgets.** What a tree cannot describe well is a kernel widget,
  drawn by Rust and placed in a tree by name with its data: `terminal`
  (`tau-terminal`), `diff` (tau-vcs's diff view moves to the kit),
  `graph` (tau-memory's), `ticker` (a time that counts, for direnv and
  run clocks). A new widget is a change to the kernel, like a new point.
- **Actions.** A button, a link, a submit or a key names an action:
  either a kernel request (`navigate`, `send`, `steer`, `composer`,
  `alert`, `open_run`, `cancel`, `focus`, `record`) or the plugin's own
  (`act`, answered by `reply`). The kernel hands a plugin's own action
  to `UiPlugin::on_action`, with its arguments and the plugin's view
  read again where the tree was drawn, and carries out what it asks
  through its handle after it returns. On the phone an action travels
  to the host.
- **UI state is the kernel's, and it is data.** Which section is open,
  the chosen tab, a draft in a text input, a selection, and the page
  open are kept by the kernel as JSON, keyed by the plugin, the point
  or page, and the node's `id`. A plugin reads them in `view`
  (`cx.ui(id)`); `UiPlugin::Ui` and its gpui entities go away. See
  "The interface's state is data" below.
- **Typed outputs stay typed.** Points whose output is data, not
  drawing (`NavEntry`, `PluginStatus`, `PlanField`, `RowNote`,
  `CardView`, `PaneView`, the context trigger), keep their types, made
  `serde` so Luau returns them as tables.
- **Cost.** `view` is pure. The renderer re-asks a plugin only when its
  state, data, settings, UI state or version changed, and caches the
  tree; a Luau `view` keeps its 200 ms limit.

### The interface's state is data

Everything the interface shows comes from state that serializes, and is
saved and sent to phones whole, so a phone, or the computer after a
restart, draws the same thing from the same state:

- **One state, three parts.** The runs (`RunView`, folded from records
  as today), the plugins' data and settings, and the interface's own
  state: the route and its history, and every plugin's UI state.
  Nothing a plugin or the frame shows lives only in a gpui entity.
- **Changes are events.** A click, a keystroke in a tree's text input,
  or a tab chosen is a `UiEvent { plugin, scope, id, value }`. A
  reducer applies it to the UI state, and the same event goes to the
  phones, which apply it the same way. The renderer keeps gpui's text
  inputs only as editors of a draft: each edit is an event, and an
  input is made again from the state it shows.
- **Drawing is a function.** `view` is pure: the same state, data,
  settings and UI state give the same tree, on every device. It reads
  no clock and no randomness; time comes in as state (a ticker's start,
  not its count).
- **Saved.** The interface's state is saved with the store, so a
  restart opens where the person left it, drafts included.
- **What is not state.** What the window's layout alone decides, such
  as a scroll offset, hover, and the caret, stays on each device: a
  phone and a computer lay the same tree out differently.

### Capabilities and the Allow click

Every seam, slot, service, point and tool a plugin reaches is a
capability, named in its declaration. 0027's `uses` grows to hold them
all:

```luau
uses = {
  tools = { "bash" },
  seams = { "before_tool", "after_tool_result", "rewrite_context" },
  slots = { "tau.title" },
  services = { "tau-jev" },
  store = "own",          -- "own" | "read" (every run, read-only)
  network = { "api.github.com" },
  secrets = { "github" },
}
```

- **Luau plugins** activate as 0027 and 0033 say. A version that reaches
  further than the person allowed waits for their click, with the
  declaration's diff. Filling a slot, reading every run in the store,
  changing what the model sees (`after_tool_result`, `rewrite_context`,
  changed arguments, `start`'s context and instructions) and new network
  hosts or secrets are always new reach.
- **Rust plugins** declare the same capabilities. They are compiled in
  and so allowed, but the Plugins screen shows what each reaches, the
  same way for both.
- **The gate is the kernel's.** A seam or service a plugin did not
  declare is never called or given; a call outside its reach fails with
  the capability's name.

### Where today's hard-coded features go

| Feature                                                    | Goes to                                              |
| ---------------------------------------------------------- | ---------------------------------------------------- |
| Coding tools, processes, artifacts and their maintenance   | `tau-tools` (run assembly, `jobs`)                   |
| Run workspaces, commits, `vcs_land`, landing merge, push   | `tau-vcs`, filling `tau.vcs`                         |
| Landing intent, queue, lanes, conflict turn, startup sweep | kernel, through `tau.vcs`                            |
| `spawn` / `wait` and the child factory                     | kernel (sub-agents are lifecycle)                    |
| Main chat prompt                                           | kernel's text; tools' parts by `prompt(kind)`        |
| AGENTS.md                                                  | new `tau-instructions`                               |
| GitHub sign-in, clone, pull requests, push screens, `/pr`  | new `tau-github` (repository kinds, VCS remote)      |
| ChatGPT sign-in, models screen, plan usage                 | new `tau-chatgpt`, filling `tau.provider`            |
| Jev key, metering, its Models section                      | `tau-jev` becomes a plugin, filling `tau.side-model` |
| Idle compaction                                            | `tau.compaction`'s filler                            |
| Run titles                                                 | new `tau-titles`, filling `tau.title`                |
| Reasoning timer                                            | `tau-reasoning`                                      |
| Desktop notices                                            | new `tau-notify`, filling `tau.notify`               |
| Phones: server, pairing, the phones screen                 | new `tau-phones` (app seams); `Synced` stays         |
| Onboarding's GitHub and model stages                       | `tau-github`, `tau-chatgpt` (`setup` stages)         |
| Agents' and plugins repository's sidebar sections          | `tau-agents`, `tau-luau-plugins` (repo kinds)        |
| The store's SQL box                                        | new `tau-store-query` (`store = "read"`)             |
| Compare forks' `exec_command` summary                      | tau-tools' `CARD` output                             |
| The fake "workspace" catalog row                           | tau-vcs's catalog                                    |

### What Luau gains

Everything in the gap table is closed by the one interface above; the
largest pieces are:

| Seam                                         | Luau today            |
| -------------------------------------------- | --------------------- |
| `start` and `RunPlan`                        | missing               |
| `before_request` (effort)                    | missing               |
| `before_tool` changing arguments             | block or flag only    |
| `after_tool_result`                          | missing               |
| `rewrite_context`, `rewritten`, `start_idle` | missing               |
| `on_event`                                   | `turn_end` only       |
| `publish`, `record`, its own fold            | `ctx.state` only      |
| `ctx.tools` outside a tool's `call`          | missing               |
| `tool_source`                                | internal only         |
| `data`, `repo_data`, `act`, `jobs`, `resume` | missing               |
| Pages, notes, commands, keys, points         | status and cards only |
| Services, slots, repository kinds, app seams | Jev only              |

## Order of work

Each step lands on main by itself and leaves tau working.

1. **The tree.** Grow the view tree, its renderer, actions and the
   interface's state as data (events, a reducer, saved and sent to
   phones) in `tau-ui-plugin` and `tau-ui-kit`; move the diff and terminal
   views into kit widgets. Wire Luau's `note`, `page`, buttons and
   actions to it.
2. **Rust plugins draw trees.** Move each plugin's UI to trees, smallest
   first: compaction, tree-compaction, schedule, skills, watcher,
   direnv, reasoning, fast-compaction, goal, constitution, mcp, canvas,
   codemode, memory, agents, vcs, tools, and last tau-ask (the panel that
   replaces the composer, with focus and keys). Then remove
   `UiPlugin::Ui` and gpui from `tau-ui-plugin`'s plugin-facing API.
3. **Declarations.** `after` / `before`, `provides` / `uses`,
   capabilities on Rust plugins, the Plugins screen showing reach.
   Delete the hand order in `plugins.rs`.
   As built: `UiPlugin::declaration` (`tau_ui_plugin::Declaration`)
   holds `after` / `before` and the plugin's `Reach`: its seams, the
   slots it fills, the services it provides and uses, and the tools,
   store, network, secrets and paths it reaches. The registry orders
   plugins by it, the rest by name, and panics on a cycle; `tau-plugins`
   lists them in any order. The kernel's gate (`gate::gate`) calls only
   the declared seams of every agent plugin a host half gives; host
   services are offered by name (`Services::offer`, `services::names`)
   and each host half sees only those it declared (`Services::scoped`);
   `Fills::fill` refuses an undeclared slot. Rust plugins' other reach
   is a statement, shown on the Plugins screen from
   `PluginInfo::reach`. `Manifest::reads` moved into the declaration,
   and `PluginInfo::seams` into its reach. The Seam list grew to every
   hook of `Plugin` and `PluginRun`.
4. **Run assembly.** `workspace`, `tools` with per-workspace state,
   `prompt(kind)`; `tau-tools` and `tau-vcs` build their own parts, and
   `agent_for_run` shrinks to the kernel's lifecycle.
   As built: the VCS gives a run's workspace (`Repository::run_workspace`,
   `vcs::RunWorkspace`) and `spawn` is the kernel's
   (`tau_ui_plugin::sub_agents`). `HostHalf::run_tools` builds the tools
   with per-workspace state (tau-tools' coding tools and their commands
   left running), from the services the host puts in `RunCtx`
   (`WorkspaceDir`, `TurnHooks`, `RunCommands`, `RunWake`, `AgentRepo`);
   `HostHalf::workspace_closed` hears a workspace close; and
   `HostHalf::prompt` gives each plugin's part of the instructions,
   which the host puts between its own first and last sentences.
   `Repository::as_any` is gone: `tau-ui` names tau-vcs-host only to
   give tau-vcs its host half, and the person's `Identity`.
5. **Slots, the easy ones.** The slot table and the Plugins screen's
   choice; `tau.title`, `tau.notify`, `tau.store`, `tau.side-model`.
6. **`tau.vcs`.** A `Vcs` trait the kernel's landing, lanes, queue and
   sweep call, implemented by `tau-vcs`. Then `tau-github`.
   As built: `tau_ui_plugin::slots` (`Slot`, `Fills`, offered by
   `HostHalf::fill`; the person's pick, kept in the UI state under
   `slots::PICKS`, else the first in `Registry::slot_order`) and `tau_ui_plugin::vcs`'s `Vcs`
   (opens a repository) and `Repository` (catch up, conflicts, newest,
   land, landed, discard, forget, held, sweep). tau-vcs-host's
   `JjVcs`/`JjRepository` fill it; landing, lanes and the startup sweep
   call only it. A run's workspace, sub-agents, push, pull requests and
   clone went through it in 6b, which also added `tau.forge`
   (`tau_ui_plugin::forge::Forge`: sign-in state, remotes with their
   token, the repository list, pull requests and checks), filled by
   `tau-github-host`; `tau-github` draws sign-in as a setup step placed
   before Repositories (`SetupAt`) and a Models section.
7. **`tau.provider`.** A neutral `Settings` and catalog in `tau-ai`;
   `tau-chatgpt` with sign-in, models and plan usage.
8. **`tau.compaction`**, with the chain as its default; idle compaction
   moves into it.
9. **App and repository seams.** `tau-phones`, repository kinds for
   agents and the plugins repository, onboarding stages.
10. **Luau parity.** The binding implements every seam and slot; the
    seam table test; the skill documents all of it, with worked plugins
    for a context rewrite, a slot filler and a page.
11. **`tau.loop`**, with the runner's invariant tests run against any
    filler.
12. **The kernel test**: no non-test source in `tau-ui` or
    `tau-ui-remote` names a plugin.

## Alternatives considered

- **Keep gpui for Rust plugins, beside the tree.** Less work now, but
  Luau would stay behind on every surface Rust uses gpui for, the phone
  would keep linking every plugin's UI, and there would be two UI
  models to keep in step.
- **A Luau binding to gpui.** Rejected in 0027 and still: it would not
  run on the phone and would tie plugins to gpui's API.
- **No slots, only first-answer-wins.** What we have. Roles are
  implicit, conflicts are silent, and the kernel cannot call "the VCS"
  without naming jj.
- **Slots filled only by Rust plugins.** Safer, but a person could
  never write their own provider or VCS in tau. The Allow click on a
  slot keeps it the person's choice.
- **Make the loop and store plugins with no kernel default.** A tau with
  no loop does nothing; a default keeps the kernel usable alone and
  gives a filler something to be tested against.

## Consequences

- tau-ui's host and tau-ui-remote shrink to the frame and the
  lifecycle; most of `host.rs`, `host/*.rs`, `github.rs`, `accounts.rs`,
  `notify.rs` and `phone_server.rs` move into plugins.
- Every Rust plugin's UI is rewritten as trees. The hardest are tau-ask
  (focus, keys, a panel in the composer's place), tau-tools (the
  terminal), tau-vcs (diffs) and tau-memory (its graph); they need
  kernel widgets first.
- A plugin's look is no longer its own: what the tree and the widgets
  cannot express needs a kernel change. That is the price of one look,
  the phone, and Luau.
- The phone no longer needs plugins' UI code to draw them: it gets trees
  over the wire. A plugin on the desktop that the phone build lacks
  still shows on the phone.
- The Luau binding, the view tree and the slot traits are public
  interfaces: changing one touches the renderer, the phone, the skill
  and its tests.
- 0002 and 0012 become the default provider's decisions, not tau's.
- `RunView` and snapshot JSON from before this change do not load;
  nothing is migrated.
