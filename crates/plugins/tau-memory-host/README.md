# tau-memory-host

Long-term memory for tau agents: a Zettelkasten of typed, linked Markdown
notes that runs search and write. This is tau-memory's host half. It keeps
the notes on disk, indexes and searches them, and builds the plugin each
run gets: four tools, the index note and a few search hits at the start of
a run (only those the conversation was not given yet), a memory-only request when compaction drops the transcript, and
stale marks when a file a note is about changes. It also imports what
Claude Code and Codex learned on this computer, as proposals the person
keeps or drops (decision 0044).

## What it provides

| Item                       | What it is                                                                         |
| -------------------------- | ---------------------------------------------------------------------------------- |
| `MemoryPlugin`             | The `Plugin`; share one across an agent's runs                                     |
| `plugin::write_notes`      | One notes request and the writes it asks for, on any `Asker`                       |
| `plugin::Asker`            | Who it asks: a run's `PluginCtx`, or the person's model outside a run (`Outside`)  |
| `Scopes`                   | The repository's scope, the user's (optional) and the clock                        |
| `Memory`                   | One scope: its notes and their index, kept in step                                 |
| `memory::Draft`            | A note to write: new, a new version (`id`), or a replacement (`supersedes`)        |
| `store::Notes`             | The note files of one directory, at `<type>/<id>.md`, with history and backlinks   |
| `index::Bm25`              | Keyword search: BM25 over each note's title, description, tags and body (ADR 0052) |
| `recall`                   | Ranking: superseded notes rank lower, and one hop along links                      |
| `safety`                   | `redact` removes secrets before a write; `refusal` catches prompt injection        |
| `MemoryHost`, `Host`       | The `HostHalf` tau's interface registers; opens each scope once per session        |
| `Memories`                 | The open scopes                                                                    |
| `stale_on_turn`            | A turn hook that marks notes about a commit's paths as possibly stale              |
| `notebook`, `now`          | A scope's notes as the page lists them; the clock in milliseconds                  |
| `importers`                | Readers for Claude Code's and Codex's files under a given home (`Roots`)           |
| `importers::remote`        | A folder's Git remote, from its config, without running Git                        |
| `import::Importer`         | Proposals, the checkpoint and the last scan, kept in memory's plugin directory     |
| `import::Reach`            | The `import` job, and the acts: import, stop, tick, keep, drop, not now            |
| `import::Proposals`        | The proposals and the tally; `admit` checks a draft as `Memory::write` would       |
| `plugin::write_notes_from` | `write_notes` on input fenced its own way, as an import's material is              |

The model gets four tools: `memory_write`, `memory_search`, `memory_read`
and `memory_link`. Ids from the user's scope read `user:<id>`.
`MemoryPlugin::consolidate(true)` adds a pass at the end of each run that
asks the model what the run taught; it is off by default.

### Imports

An import reads `~/.claude` and `~/.codex` (the host's `HomeDir`; none
in tests) and proposes notes, never writing one. Claude Code's saved
notes are proposed as they are; Codex's memories and both tools' chats
go to the person's model, through `write_notes_from` with the tools
`memory_search` and `memory_propose`, 32 kB at a time and three files at
a time. Each proposal passes the checks every note does, and is merged
into one that says the same. `proposals.json` and `checkpoint.json` in
`import/` under memory's plugin directory keep them and what was read,
so an import cut off by a restart starts again and a second one reads
only what is new. Keeping a proposal writes it with
`Source::imported`.

## How it fits

This is the host half (decision 0030). Its partner, `tau-memory`, is the
interface half: the note format, the records and the pages. This crate
re-exports `NAME`, `note` and `record` from it.

It builds on `tau-agent` (the `Plugin` and tool traits), `tau-ai`
(messages and request settings for the compaction and consolidation
requests) and `tau-ui-plugin` (`HostHalf`).

Used by `tau-ui`, which registers `MemoryHost`; by `tau-demo`, with the
`demo` feature; and by `crates/evals/tau-memory-e2e`, the end-to-end
evaluation. In the app a repository's notes are in `memory/` in tau's
directory for it, and the user's in `memory/` in tau's data directory.

## Usage

Memory on an agent, outside tau's interface:

```rust
use std::sync::Arc;
use tau_memory_host::{Memory, MemoryPlugin, Scopes, now};

let repo = Memory::open("notes/repo")?;
let user = Memory::open("notes/user")?;
let memory = MemoryPlugin::new(Scopes::new(repo, Some(user), Arc::new(now)));

let agent = agent.plugin(memory);
```

## Features

| Feature | What it adds                                               |
| ------- | ---------------------------------------------------------- |
| `demo`  | `demo::seed` and `demo::imports`, what tau-ui's demo shows |

It is off by default.

## Testing

```sh
cargo nextest run --release -p tau-memory-host
```

Most tests are property tests (`hegeltest`) over notes, the store, recall
(BM25 against its formula written out plainly) and redaction. The
import's tests read synthetic copies of Claude Code's and Codex's files
in a temporary home, never the developer's, with the person's model
scripted (`tau-testing`).

## Further reading

- [docs/reference/plugins.md](../../../docs/reference/plugins.md), section
  "`tau-memory`: a zettelkasten"
- [docs/research/memory.md](../../../docs/research/memory.md), the research
  behind the design and the evaluation
- [ADR 0030: host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
