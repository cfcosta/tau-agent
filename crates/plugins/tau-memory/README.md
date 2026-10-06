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

The crate has no tests of its own. Its note format, records and UI are
tested through the host half:

```sh
cargo nextest run --release -p tau-memory-host
```

## Further reading

- [docs/reference/plugins.md](../../../docs/reference/plugins.md), section
  "`tau-memory`: a zettelkasten on docbert"
- [docs/research/memory.md](../../../docs/research/memory.md), the research
  behind the design
- [ADR 0017: plugins bring their UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [ADR 0030: host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
