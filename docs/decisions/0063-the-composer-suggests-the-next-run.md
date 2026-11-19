# 0063: the composer suggests the next run, from a model

- Status: accepted.
- Date: 2026-11-19

## Context

The new-run composer always says "Describe a change, paste an issue, or
@ a file" (`NEW_RUN_PLACEHOLDER` in `heiko-ui-remote`). Codex's desktop
app says something about the project instead: "Finish the VM tap
networking path for qBittorrent peer reachability".

The desktop app is closed source, but its output and plumbing are not.
Per project root it keeps `~/.codex/ambient-suggestions/<hash>/
ambient-suggestions.json`: `projectRoot`, `generatedAtMs`,
`currentSuggestionIds`, and suggestions with a `title`, a
`description`, the full `prompt` it would send, the connectors it needs
(`appIds`), a `status` and timestamps. They come from background threads
tagged `ThreadSource::Feature("ambient_suggestions")`, with a second
pass tagged `"ambient_suggestion_safety"` (codex-rs
`core/tests/suite/git_enrichment.rs:516`). The TUI keeps those threads
out of its @-mentions (`tui/src/task_mentions.rs:179`). Its suggestions
quote the uncommitted diff, the project's configuration and the open
pull requests. The TUI's own placeholder is a constant, "Ask Codex to do
anything".

Suggestions in heiko come from a model or not at all: hand-written
guesses from context were rejected for `/goal`.

What heiko has already:

- A Luau package can ask the model (`uses.infer`, as heiko-titles does)
  and keep a store per repository.
- `run_end` runs in the run, on its repository's store, and can read
  its conversation (`uses.transcript`). It cannot call the run's tools:
  `vcs_status` and the rest are refused there.
- The loop waits for every plugin's `run_end` before the run's task
  ends.
- A job that runs more often than once a minute is a tick: it runs in
  the background, one at a time, and reaches `infer` and every
  repository's store (`heiko.store.of(repo)`). Nothing redraws the
  interface after a tick.
- `repo_data` gives what a plugin keeps of a repository to the
  interface, read again with the catalog.
- A plugin can fill the composer (`heiko.composer`), draw in its place
  (`heiko.run.composer`) and draw a band over it
  (`heiko.run.composer.band`). It cannot set the placeholder.

## Decision

- **A built-in package, `heiko-suggestions`.** It is written in Luau
  and lives in `plugins/`. In each repository's store it keeps its
  suggestions, each with a `title`, the `prompt`, `why`, the `run` and
  `model` it came from, `at`, and a `status` of `pending`, `taken` or
  `dismissed`.
- **Nothing waits on the model.** `run_end` reads the run's conversation
  and keeps a request in the repository's store: what the person asked,
  the last answer, and the tools called with the start of what they
  gave. It asks no model, so the run ends as soon as it would have. A
  tick, every five seconds, takes each repository's request and asks
  the plan's small model (`luna`, at low effort) for suggestions,
  answered to a schema. They replace the repository's pending ones. A
  tick whose state changed asks the interface to draw again.
- **The rules are in the request, with no second pass.** A suggestion
  must rest on something the conversation shows, and `why` names it.
  It may not ask to delete, publish, push or spend, or to do again what
  the run just did. The request lists the titles the person took or
  dismissed, which are not suggested again.
- **The composer shows the newest pending title.** A new UI point,
  `heiko.repo.composer.hint`, gives the composer of a new run in a
  repository its placeholder and the prompt behind it.
  heiko-luau-plugins fills it from the first active plugin whose
  `repo_data` gives `composer = { placeholder, prompt }`. The composer
  shows the placeholder while it is empty, and Tab puts the prompt in
  it without sending it. A run that starts with that prompt marks the
  suggestion taken. Until the model answers, and with no suggestion,
  the composer keeps `NEW_RUN_PLACEHOLDER`. The placeholders for a live
  run, a fork and a reply stay fixed.
- **Its page lists them per repository.** Each pending suggestion shows
  its title, why, prompt, model and age, with Put in composer, Start
  and Dismiss. Suggest again asks for a new set from the last
  conversation kept. The settings are on or off and how many to keep (1
  to 5).

## Consequences

- An idle repository gets no new suggestions: they are written after a
  run there ends, or when the person asks from the page. The composer
  shows none older than seven days.
- The model reads the conversation, not the repository. A conversation
  usually holds the status and diffs its tools gave, but a change made
  outside heiko since then is not seen, nor are the forge's pull
  requests.
- Each run's end costs one small-model request, a few seconds after the
  run. The cost shows under the plugin, as titles' does.
- The new UI point is one more seam a plugin can fill. It takes text,
  not a tree.

## Alternatives

- **A timer over every repository, reading its status, diff and the
  forge, as Codex appears to.** Jobs would need to reach a repository's
  tools and the forge, a larger change to what a job may do.
- **Asking the model in `run_end`.** The loop waits for it: the run's
  task, and a reply to it, would wait on a suggestion.
- **A second pass that checks each suggestion (`decide`), as Codex's
  safety pass does.** One more request per suggestion. The rules in the
  request, and Tab only filling the composer, were judged enough.
- **Built into the interface in Rust.** Plugins are where the person
  can see, change or turn off what reaches the model, and a package
  already has a store, a page and its costs.
- **A band over the composer instead of the placeholder.** A band takes
  room on every screen; the placeholder goes away as the person types.
- **Suggestions from the last command, the open files or the branch
  name.** These are the hand-written guesses that were rejected for
  `/goal`.
