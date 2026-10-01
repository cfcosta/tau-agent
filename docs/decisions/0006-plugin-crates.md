# 0006: Plugins live under crates/plugins

- Status: accepted. Amended by
  [0017](0017-plugins-bring-their-ui.md): a plugin crate exports its UI
  with its agent plugin, and has no headless form.
- Date: 2026-09-28

## Context

Plugins ([0005](0005-plugins.md)) extend an agent from their own
crates. The coding tools and compaction are plugins now, and more are
planned: memory, reasoning selection, rule checks and pruning.

## Decision

- Core crates stay in `crates/`: `tau-ai`, `tau-agent`, `tau-store` and
  `tau-testing`.
- Each plugin is a crate in `crates/plugins/<plugin>`, named
  `tau-<plugin>`. A library that only plugins use, such as the Jev
  client `tau-jev`, lives there too.
- Core crates never depend on a plugin. A plugin depends on `tau-agent`
  and uses only its public API, the same API a plugin outside this
  repository gets.
- The workspace lists `crates/tau-*` and `crates/plugins/*`.

## Consequences

- If a plugin in this repository needs something from the core, the
  core makes it public API for every plugin.
- `tau-agent` has no shorthands for plugins: an agent adds compaction,
  like any plugin, with `Agent::plugin`. Context plugins are offered the
  context in the order they were added.
- A user takes only the plugins they add as dependencies.
