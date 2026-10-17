# tau-memory

The interface half of tau-memory, tau's long-term memory plugin. Memory is
a Zettelkasten of typed, linked Markdown notes that runs search and write.
This crate holds what every interface needs to show it: the note format,
the records a run publishes, and the pages that draw the notes. It has no
search, no tools and no plugin; those are in `tau-memory-host`.

## What it provides

| Item                    | What it is                                                                 |
| ----------------------- | -------------------------------------------------------------------------- |
| `NAME`                  | `"tau-memory"`, the name the plugin goes by in events and records          |
| `note::Note`            | One idea: TOML front matter between `+++` lines, then a Markdown body      |
| `note::NoteType`        | The closed set of kinds: fact, convention, decision, gotcha, and so on     |
| `note::LinkType`        | The closed set of links: `relates`, `refines`, `supersedes`, `about`, ...  |
| `note::Link`, `Source`  | A typed link to another note, and who wrote a note (`By`) and from where   |
| `Note::parse`, `render` | Read and write a note file; `validate` checks it                           |
| `note::wiki_links`      | The bare `[[id]]` links in a body, which count as `relates`                |
| `note::slug`, `is_id`   | Make an id from a title, and check one                                     |
| `record::Record`        | What a run publishes: `Recalled`, `Saved`, `Error` and `Starting`          |
| `record::USER`          | `"user:"`, the prefix of ids from the user's scope                         |
| `MemoryUi`              | The `UiPlugin`: notes pages, sidebar entries, transcript marks, run status |
| `ui::Notebook`          | A scope's notes as their page shows them (`NoteView`, `LinkView`)          |
| `ui::State`, `ui::Mark` | A run's folded records: how many notes, and what was recalled or saved     |
| `ui::notes_link`        | The link to a repository's notes page                                      |
| `ui::MemoryData`        | Its data across repositories: the person's notes, and `Imports`            |
| `ui::review_link`       | The review page of what imports propose                                    |
| `ui::READS`             | What it reads outside repositories: `~/.claude`, `~/.codex`                |
| `import::Proposal`      | A note an import proposes, with its scope, origins and flags               |
| `import::Place`         | A folder and its remote; `matches` tells the repository that claims it     |
| `import::Act`, `Request` | What its pages ask the host half: import, stop, tick, keep, drop, not now |
| `import::Imports`       | What the interface shows of imports: found, pending, banners, progress     |
| `note::Imported`        | Where an imported note came from: tool, path, session                      |

A note file looks like this:

```text
+++
id = "retry-after-http-date"
title = "retry-after can be an HTTP date"
description = "Parse both forms; a bad one falls back to backoff"
type = "gotcha"
created = 1790000000000
updated = 1790000000000
valid_from = 1790000000000

[source]
by = "agent"

[[links]]
to = "retry-policy"
type = "refines"
+++
The body, in full prose: exact versions, flags, paths and errors.
```

Besides the notes pages, its UI holds imports from Claude Code and Codex
(decision 0044): a Memories step in first-run setup (`ui::setup`), the
review page (`ui::review`), and on the Memory tab an import button and a
banner when the tools know a repository (`ui::banner`). All of them draw
from `MemoryData` and act through the host half, so the review page and
the banner work on a phone.

## How it fits

This is the interface half (decisions 0006, 0017 and 0030). Its partner,
`tau-memory-host`, keeps the notes on disk, indexes and searches them,
and builds the `tau_agent::plugin::Plugin` each run gets. The host half
re-exports `NAME`, `note` and `record` from here.

It builds on `tau-ui-plugin` and `tau-ui-kit` for its UI, and on gpui.
It does not depend on `tau-agent`, so it stays light.

Used by:

- `tau-memory-host`, which writes the records and fills the `Notebook`;
- `tau-ui-remote`, which lists `MemoryUi` among its plugins, so a phone
  can draw notes without linking the host half;
- `tau-ui`, which reads its UI types in its demo tests.

## Usage

Reading a note file and walking its links:

```rust
use tau_memory::note::Note;

let note = Note::parse(&std::fs::read_to_string(path)?)?;
for link in note.all_links() {
    println!("{} -> {} ({})", note.id, link.to, link.kind.as_str());
}
```

`all_links` adds the bare `[[id]]` links in the body to those in the front
matter.

## Testing

The crate tests its settings. Its note format, records and imports are
tested through the host half, and its import pages through
`tau-ui-remote`:

```sh
cargo nextest run --release -p tau-memory -p tau-memory-host
cargo nextest run --release -p tau-ui-remote --test memory_import
```

## Further reading

- [docs/reference/plugins.md](../../../docs/reference/plugins.md), section
  "`tau-memory`: a zettelkasten on docbert"
- [docs/research/memory.md](../../../docs/research/memory.md), the research
  behind the design
- [ADR 0017: plugins bring their UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [ADR 0044: memory imports what other agents learned](../../../docs/decisions/0044-memory-imports-what-other-agents-learned.md)
- [ADR 0030: host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
