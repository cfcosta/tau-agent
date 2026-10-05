# 0030: Host halves are crates

- Status: accepted. Amends [0017](0017-plugins-bring-their-ui.md): a
  plugin's `UiPlugin` no longer carries what it does on the host; a
  `HostHalf` does, and the heavy ones live in crates of their own.
  Amends [0003](0003-sqlite-via-sqlx-macros.md): `tau-store` holds the
  interface, `tau-store-sqlite` the database.
- Date: 2026-10-05

## Context

A clean release build took 3 min, and most of its last minute was
waiting:

- `tau-ui-remote` draws runs and starts none, so it asked for each
  plugin without its `host` feature. Cargo turns on a feature for the
  whole build when any crate asks for it, and `tau-ui` asks for every
  plugin's `host`. So a desktop build compiled one copy of each plugin,
  with its host half, and `tau-ui-remote` waited for it: for Luau's C++
  (mlua, 105 s on its own path), jj-lib, rmcp and candle, none of which
  it uses.
- `tau-agent` took `tau-store`, and with it bundled SQLite, a single C
  file that took 57 s. Every plugin waits for `tau-agent`, so no plugin
  started before a minute had gone.

A feature cannot keep a dependency out of a build that has a crate
asking for it. A crate boundary can.

## Decision

### A plugin has two halves, as two traits

- `UiPlugin` is what every interface has: its state's fold, its data
  and settings types, its UI (`manifest`, `reply`), `read_prompt` and
  `rewrites_keep_transcript`.
- `HostHalf` is what the machine that runs agents has: its host state
  (`type Host`), `agent_plugins`, `starting`, `launcher`, `catalog`,
  `data`, `repo_data` and `act`. `type Plugin` names the `UiPlugin` it
  is the half of, so its data and settings are that plugin's types.
- The registry holds both. `Registry::with(plugin)` adds a plugin
  without its host half (`NoHost`): asked to make its host state, it
  fails, naming the plugin. `Registry::host(half)` gives a plugin its
  half, in its place.

### Heavy host halves are crates of their own

A host half that brings what the interface does not need is a
`tau-<plugin>-host` crate, which depends on its plugin's crate:
tau-codemode (Luau), tau-luau-plugins (codemode), tau-vcs (jj-lib,
gix), tau-mcp (rmcp), tau-tools (the tools), tau-memory (candle) and
tau-constitution (its sqlx database). The plugin's crate keeps its
records, their fold, its views and the shapes they read. A host crate
re-exports the shapes its API returns.

A plugin whose host half is light keeps it in its own crate, as
`XHost` beside `XUi`: behind the `host` feature for tau-ask,
tau-direnv and tau-skills, which a phone builds without, and always for
the rest.

### One list of plugins, given their halves by the host

`tau-ui-remote` lists the plugins in order (`plugins::plugins()`),
each without its host half: that is what a phone runs, and what
`plugins::registry()` gives until a host installs its own. `tau-ui`
gives each plugin its half and installs the result
(`plugins::install`) before it makes any plugin's host state. Both
draw the same; only a host asks a plugin for what its host half does.

### The app links the Luau host halves

The desktop binary is a crate of its own, `tau`. It gives the plugins
tau-ui has no half for, tau-codemode and tau-luau-plugins, theirs, and
installs the result before anything starts a host. `tau-ui` itself
depends on neither, so it builds while Luau's C++ compiles. Without an
installed registry, a host gives the plugins the halves tau-ui has
(`hosted::install`), as tau-ui's own tests do unless they install the
app's (`tau::install`).

### tau-store has no database driver

`tau-store` holds the run and entry types and a `Backend` trait;
`Store` is `Arc<dyn Backend>`. `tau-store-sqlite` implements it with
sqlx and keeps the migrations and the `.sqlx/` metadata. Only what
opens a store depends on it: `tau-ui`, the evaluations, and tests.

## Consequences

- `tau-ui-remote`, and the agent loop and every plugin's interface
  half, build without Luau, jj, rmcp, candle or SQLite.
- Moving a module between halves crosses a crate: an item the host half
  reads must be public in the plugin's crate.
- A plugin's tests that exercise its host half live in its host crate.
