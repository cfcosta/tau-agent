# tau-memory-host

Long-term memory for tau agents: a Zettelkasten of typed, linked Markdown
notes that runs search and write. This is tau-memory's host half. It keeps
the notes on disk, indexes and searches them, and builds the plugin each
run gets: four tools, the index note and a few search hits at the start of
a run, a memory-only request when compaction drops the transcript, and
stale marks when a file a note is about changes.

## What it provides

| Item                   | What it is                                                                       |
| ---------------------- | -------------------------------------------------------------------------------- |
| `MemoryPlugin`         | The `Plugin`; share one across an agent's runs                                   |
| `Scopes`               | The repository's scope, the user's (optional) and the clock                      |
| `Memory`               | One scope: its notes and their index, kept in step                               |
| `memory::Draft`        | A note to write: new, a new version (`id`), or a replacement (`supersedes`)      |
| `store::Notes`         | The note files of one directory, at `<type>/<id>.md`, with history and backlinks |
| `index::Index`, `Bm25` | The search index trait, and keyword search                                       |
| `colbert::Colbert`     | ColBERT MaxSim search over an `Encoder`, with embeddings cached on disk          |
| `docbert::Docbert`     | docbert's model as an `Encoder` (`docbert` feature)                              |
| `recall`               | Ranking: superseded notes rank lower, and one hop along links                    |
| `safety`               | `redact` removes secrets before a write; `refusal` catches prompt injection      |
| `eval`                 | The retrieval evaluation over `eval/harbor.toml`                                 |
| `MemoryHost`, `Host`   | The `HostHalf` tau's interface registers; opens each scope once per session      |
| `Memories`, `Search`   | The open scopes, and whether they search by meaning or by keywords               |
| `stale_on_turn`        | A turn hook that marks notes about a commit's paths as possibly stale            |
| `notebook`, `now`      | A scope's notes as the page lists them; the clock in milliseconds                |

The model gets four tools: `memory_write`, `memory_search`, `memory_read`
and `memory_link`. Ids from the user's scope read `user:<id>`.
`MemoryPlugin::consolidate(true)` adds a pass at the end of each run that
asks the model what the run taught; it is off by default. The default
model for `Docbert` is `lightonai/GTE-ModernColBERT-v1` (`docbert::MODEL`).

## How it fits

This is the host half (decision 0030). Its partner, `tau-memory`, is the
interface half: the note format, the records and the pages. This crate
re-exports `NAME`, `note` and `record` from it.

It builds on `tau-agent` (the `Plugin` and tool traits), `tau-ai`
(messages and request settings for the compaction and consolidation
requests) and `tau-ui-plugin` (`HostHalf`). The `docbert` feature pulls in
`docbert-pylate` and candle.

Used by `tau-ui`, which registers `MemoryHost` with the `docbert` and
`demo` features, and by `crates/evals/tau-memory-e2e`, the end-to-end
evaluation. In the app a repository's notes are in `memory/` in tau's
directory for it, and the user's in `memory/` in tau's data directory.

## Usage

Memory on an agent, outside tau's interface, searching by keywords:

```rust
use std::sync::Arc;
use tau_memory_host::{Memory, MemoryPlugin, Scopes, index::Bm25, now};

let repo = Memory::open("notes/repo", Box::new(Bm25::new()))?;
let user = Memory::open("notes/user", Box::new(Bm25::new()))?;
let memory = MemoryPlugin::new(Scopes::new(repo, Some(user), Arc::new(now)));

let agent = agent.plugin(memory);
```

With the `docbert` feature, search by meaning instead:

```rust
use tau_memory_host::{colbert::Colbert, docbert::Docbert};

let index = Colbert::new(Docbert::new()).cached("notes/repo-embeddings");
let repo = Memory::open("notes/repo", Box::new(index))?;
```

The model is loaded on first use and downloaded from the Hugging Face hub
when it is not cached.

## Features

| Feature   | What it adds                                                         |
| --------- | -------------------------------------------------------------------- |
| `docbert` | The `docbert` module and the `tau-memory-eval` binary (needs candle) |
| `demo`    | `demo::seed`, the notes tau-ui's demo shows                          |

Neither is on by default.

## Testing

```sh
cargo nextest run --release -p tau-memory-host
```

Most tests are property tests (`hegeltest`) over notes, the store, recall,
redaction and the evaluation corpus. They search with BM25 or a fake
encoder, so no model is loaded. One test loads docbert's real model and
may download it, so it only runs when asked:

```sh
cargo nextest run --release -p tau-memory-host --features docbert --run-ignored only
```

The retrieval evaluation compares BM25, ColBERT and the two fused as
near-duplicates pile up:

```sh
cargo run --release -p tau-memory-host --features docbert --bin tau-memory-eval
```

`--keywords` runs BM25 alone without loading the model, and `--json PATH`
writes every row. Embeddings are cached in `$XDG_CACHE_HOME/tau/memory-eval`.

## Further reading

- [docs/reference/plugins.md](../../../docs/reference/plugins.md), section
  "`tau-memory`: a zettelkasten on docbert"
- [docs/research/memory.md](../../../docs/research/memory.md), the research
  behind the design and the evaluation
- [ADR 0030: host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
