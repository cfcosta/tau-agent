# 0044: Memory imports what other agents learned, through the plugin interface

- Status: accepted. Amends [0005](0005-plugins.md): a host half can
  call the person's model outside a run. Amends
  [0017](0017-plugins-bring-their-ui.md): plugins contribute setup
  steps, the status bar and a repository page's actions. Builds on
  [0006](0006-plugin-crates.md), [0028](0028-async-all-the-way-blocking-only-in-spawn-blocking.md),
  [0030](0030-host-halves-are-crates.md) and
  [0043](0043-local-repositories-in-place.md).
- Date: 2026-10-17

## Context

People come to tau with months of work in Claude Code and Codex. Both
keep what they learned on this computer: Claude Code as memory notes
per folder (`~/.claude/projects/<folder>/memory/*.md`) and chat
transcripts (`*.jsonl` beside them); Codex as a memory folder
(`~/.codex/memories/`, grouped by working directory, with rollout
summaries) and session transcripts (`~/.codex/sessions/**/*.jsonl`).
tau's memory starts empty.

The import belongs to the memory plugin: its notes, its checks, its
review. Today a plugin cannot do it alone. First-run setup is a closed
enum; a host half has no model but Jev; background work has no owner
and no progress; the status bar shows only tau's own news; a
repository is known by its sidebar name alone; and core names the
memory plugin in the repository menu, against ADR 0006.

The screens were iterated on a design canvas, beside the local
repository screens of ADR 0043.

## Decision

### What the person sees

- First-run setup gains a Memories step, between Repositories and
  Ready, when Claude Code or Codex is on this computer. For each tool
  it offers what the tool saved and its chats, both on by default, for
  the folders that match the repositories just picked. Notes about the
  person go to their own scope. A checkbox keeps notes for folders tau
  does not have yet, offered when such a repository is added.
- The person's model turns what was saved and what was said into
  tau's notes. It merges notes that say the same thing. A note that
  disagrees with one tau has, or that held a secret, waits unticked
  under "Need a look".
- A review page lists the proposed notes per scope, each with its type
  and where it came from. Nothing is kept until the person keeps it.
  Reading chats goes on after setup, and new proposals join the list.
- The Memory tab offers "Import from Claude Code or Codex…" at any
  time, and a banner when a repository those tools know is added.
- No cost is shown: tau runs on the person's subscription.
- tau only reads those tools' files. What they wrote is data, never
  instructions: every imported note goes through `Memory::write`, so
  it is redacted and refused when it reads as an injection.

### What the plugin interface gains

All of it is generic. Core gains points and services, and never names
the memory plugin.

1. **Setup steps.** A `points::SETUP` point. Its context carries the
   repositories picked so far: name, id, folder and remote.
   `SetupStep` gains `Plugin { plugin, step }`, and the step bar's
   stages are computed from the contributions. A step ends with
   `Request::SetupDone`, or the person skips it. The host half hears
   when setup finishes.
2. **The person's model outside a run.** A `Model` service in
   `HostCx.services`: `ask(settings, input, tools)` on the plan's
   client. Usage is booked to the plugin, for the plan's limits.
   Memory's `distill` loop runs on it unchanged.
3. **Jobs owned by the host.** A `Jobs` service: a plugin starts a
   named job; the host keeps it running past setup and across
   windows, reports its progress (a fraction and a line), cancels it,
   and resumes it after a restart from a checkpoint the plugin keeps in
   its `plugin_dir`. A job reads the repositories as they are now, not
   as they were when it started.
4. **The status bar.** A `points::ACTIVITY` point, fed by job progress
   and open to any plugin. Local repository setup (ADR 0043) uses it
   too.
5. **Repository identity.** `RepoCtx` gains a stable `RepoId`, the
   remote URL, and the folder for a local repository. Plugin data keys
   on the id, so it survives a relink. Memory matches a folder Claude
   Code or Codex worked in to a repository by that folder's remote.
6. **Core forgets memory.** The repository menu's "Memory" item becomes
   memory's own `REPO_MENU` contribution, and `open_memory` goes. A
   `points::REPO_ACTIONS` point lets a plugin add actions to a
   repository page's header.
7. **Declared reach.** A plugin's manifest lists the paths it reads
   outside repositories (memory: `~/.claude`, `~/.codex`). Its setup
   step and its plugin page show them. Rust plugins are not sandboxed,
   so this is a statement, not a fence.

### What memory gains

- An `importers` module with a reader for Claude Code and one for
  Codex. Adding a tool is a change to memory.
- A note's `Source` gains `Imported { from, path, session }`.
- Proposals live under memory's `plugin_dir` until kept or dropped.
- Memory's host half implements `act` (keep, skip, edit, start an
  import) and its UI `reply`.

## Consequences

- Built in order: identity and core forgetting memory; then the Model
  and Jobs services; then the ACTIVITY and SETUP points; then the
  import.
- Setup's length now depends on the plugins: the step bar counts what
  is contributed, and setup without Claude Code or Codex looks as it
  did.
- A plugin can spend the person's plan without a chat to show it. The
  plugin's ledger shows that use; the plan's limits still apply.
- Setup stays on the desktop. The review page and the banner render
  from catalog data, so they work on the phone.
- A tool's file format changes without notice. The importers read
  only what they need, and a file they cannot read is skipped and
  named on the review page.

## Alternatives

- **A hidden plugin run for the model calls.** It would reuse
  `PluginCtx::ask` and run charging, but a run without a chat is a
  run that History and landing must learn to ignore.
- **A chat in a repository of tau's own.** Visible and auditable, but
  it fills History with work the person did not ask to watch.
- **Plugins spawn their own background tasks.** They can today, but
  nothing shows progress, survives a restart, or sees repositories
  added since.
- **A `MEMORY_SOURCE` point for importers in other plugins.** More
  open, but nothing else needs it yet.
