# 0033: Plugins are live

- Status: accepted. Amends [0027](0027-luau-plugins.md): runs no longer
  keep the plugin versions they started with, and a chat in the plugins
  repository runs its own versions before they land.
- Date: 2026-10-09

## Context

A chat in the plugins repository wrote a plugin, `checkpoints`, ran its
tests (9 of 9 passed) and committed it. Asked to show it working, the
model could not: the plugin was not active. Two rules kept it out:

- A plugin was active only at a commit of the plugins repository's
  trunk. The chat had committed in its own workspace and not landed, so
  for the registry the plugin did not exist.
- A run kept the hooks and tools it started with (0027, "Behaviour is
  pinned per run"). Even once landed, a change reached a chat only on
  its next message, never during a turn.

The person wants tau to feel like a living thing: a plugin works as
soon as it changes.

## Decision

- **A run follows its plugins' versions.** Before each model request, a
  run takes the versions active then. Each hook and tool call goes to
  the plugin's version of that moment. A plugin's run state is kept by
  name, so a new version starts from the state the one before left, and
  a plugin whose version changed or that is new draws its view again.
- **New tools come through code mode.** The tools a run started with
  are declared to the model, as before. A tool that came later is
  offered through the plugin's tool source (as MCP servers' tools are,
  0018): code mode's `search_tools` finds it and scripts call it at
  once. The request's tools, which key the prompt cache (0022), stay as
  they were. From the next message it is declared like any other. A
  declared tool whose plugin changed runs the new version; one whose
  plugin dropped it answers that it is gone.
- **A chat in the plugins repository runs its own versions.** For a run
  whose workspace is in the plugins repository, each plugin folder
  there that differs from trunk is loaded and tested, once per version
  (by its files' digest). A version that passes its tests and reaches
  no further than the person allowed takes trunk's place for that run.
  Every other run gets it when it lands on trunk. A plugin the
  workspace lacks keeps trunk's version, so a chat forked before a
  plugin landed still has it; removing a plugin takes effect when the
  removal lands.
- **Allowing stays where it was.** A version that reaches further than
  the person allowed does not run from a workspace either: it waits for
  its click on the Plugins screen once it lands, as in 0027.

## Alternatives considered

- **Change the request's tools at the next turn.** A new tool would be
  declared at once, but that request would lose the prompt cache, and
  tools that come and go change the cache key run-wide. Code mode's
  catalog already holds tools that come and go.
- **Land plugin chats by themselves.** Every passing change would reach
  every run at once, with no review. The chat that writes a plugin is
  where it needs to work first; trunk stays the reviewed source.
- **Keep pinning per run.** Consistent within a run, but a plugin you
  just wrote does not work where you wrote it.

## Consequences

- A hook can change between two requests of one run. Plugins whose
  hooks keep state should read it as data a version before may have
  written.
- A run of the plugins repository reads its workspace's plugin folders
  before each request: a directory walk and a digest per plugin. Load
  and tests run once per version.
- The skill tells the model that a plugin works in its chat once its
  tests pass, and that a new tool is reached from code mode until the
  next message.
