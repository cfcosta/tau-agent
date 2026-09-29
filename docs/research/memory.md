# Research: agent memory, for tau-memory

- Status: decided 2026-09-29 (see Decisions); the plan is in
  [plugins.md](../reference/plugins.md#tau-memory-a-zettelkasten-on-docbert)
- Date: 2026-09-29
- Checked against: Codex `codex-rs` at `6c49240`; akitaonrails/ai-memory
  at `0e560e3` (v2.4.1); letta-ai/letta-code at `2b95889` and letta
  `archive` at `56ba9c2`; mem0 at `94c3fe9`; graphiti at `ea4ac0f`
  (v0.30.2); langmem at `9d033b4`; cognee at `c4cd8ce`;
  basic-memory at `20b1523` (v0.23.2); docbert at `3a8c760`; the papers
  and articles in `~/Notes/resources/ai-ml`, and arXiv as cited.

This note surveys how agents keep long-term memory today, to shape
`tau-memory`: a Zettelkasten of atomic notes, retrieved with docbert
(BM25 and ColBERT late interaction through `docbert-pylate`, with a
`docbert-plaid` index). It covers coding agents, memory frameworks,
linked-note systems, and the research evidence, then proposes a design
and lists what is left to decide.

Most published numbers are the vendor's own, run on conversational QA
(LoCoMo, LongMemEval), and several are disputed. They are quoted as
reported and marked as such. None measures a coding agent working
across sessions.

## Summary

- **Keep the Zettelkasten.** The evidence favours it on the points that
  matter: explicit links help when evidence is scattered or far apart
  (a linked-notes benchmark and CompassMem), full-text notes beat
  extracted facts and summaries, and the closest shipped systems for
  coding agents (Letta Code's MemFS, ai-memory, Basic Memory, Claude
  Code) all converged on Markdown files with frontmatter and `[[links]]`.
- **Two tiers.** A small index note is always in context, frozen for the
  run, within a hard budget; every note beyond it is found through
  docbert and read on demand. Every coding agent that ships memory does
  this (Codex 2.5k tokens, Claude Code 200 lines, Hermes about 1.3k
  tokens).
- **Links are written, typed and few.** The agent or the consolidator
  writes them, from a small fixed set of types. Links derived from
  embedding similarity forget like the embeddings they come from (The
  Price of Meaning), so docbert finds entry points and links carry the
  rest.
- **Retrieval: docbert for seeds, then one hop along links,** inside a
  fixed budget. BM25 stays a first-class path for identifiers, paths and
  error strings. Notes stay plain files, so grep and file reads work
  too; a coding agent's own search tools are a strong baseline.
- **Writes: in the loop, and at two moments the harness owns.** The
  agent writes notes through tools while it works. The plugin also gives
  the model a memory-only turn before compaction, and a background pass
  after a run that proposes notes, searching before it writes and allowed
  to write nothing.
- **Never overwrite history.** A changed fact becomes a new note that
  `supersedes` the old one, which keeps its text and gets a `valid_to`.
  Retrieval prefers current notes but still finds old ones. Mem0 moved
  away from LLM-chosen DELETE because it lost information; its add-only
  replacement now finds knowledge updates hardest. Supersession sits
  between the two.
- **Provenance on every note:** the run, turn, commit and files it came
  from, and whether the user said it, the agent did it, or it was
  inferred. That makes staleness checkable against the repository.
- **Memory is data, not instructions.** Notes are fenced as untrusted
  when injected, secrets are redacted before anything is written, and
  rules that must always hold belong in `AGENTS.md` or the constitution,
  not in memory.
- **Build the evaluation first.** No design wins everywhere, plain
  retrieval over raw logs is a hard baseline, and vendor benchmarks
  disagree by double digits. tau-memory needs its own eval on coding
  tasks before any optional machinery (decay, merging, reflection) ships
  on.

## Where tau-memory stands

The plan is in [plugins.md](../reference/plugins.md#tau-memory-a-zettelkasten-on-docbert):
four tools (`memory_write`, `memory_search`, `memory_read`,
`memory_link`), top notes put into `plan.context` at `start`, optional
distillation at `finish`, one Markdown file per note indexed by
`docbert-core`. Nothing is built; tau-ui's Memory screen runs on demo
data.

What docbert offers today (`~/Code/cfcosta/docbert`):

- **`docbert-core`** runs BM25 (Tantivy, English stemming, optional
  fuzzy) and ColBERT in parallel and fuses them with reciprocal rank
  fusion; a `bm25_only` flag skips the semantic leg. State lives in
  `config.db`, `embeddings.db`, `tantivy/` and an optional `plaid.idx`.
- **`docbert-plaid`** can update an index in place:
  `plaid::update_index_from_embedding_db` and `update_index_with_chunks`
  take changed and deleted ids and re-encode only those, without
  retraining centroids or the residual codec. A new note can be
  searchable at once.
- **Still missing: one call to upsert or delete a document.** Tantivy
  (`SearchIndex::add_document` / `delete_document`), the embeddings, the
  chunk manifests and the PLAID update are separate steps a caller has
  to sequence. The plan's upstream request stands.
- **PLAID needs a trained codec,** which is wasted on a few dozen notes.
  docbert's exhaustive MaxSim over every stored embedding serves until
  the collection is large; the switch can be a size threshold.

## Coding agents

| Agent                 | Unit                                                                           | Who writes, when                                                             | How it reaches the model                                |
| --------------------- | ------------------------------------------------------------------------------ | ---------------------------------------------------------------------------- | ------------------------------------------------------- |
| Codex CLI             | `MEMORY.md` handbook, `memory_summary.md`, rollout summaries, skills; git repo | Background: phase 1 extracts per idle session, phase 2 consolidates globally | Summary injected (2.5k tokens); agent greps `MEMORY.md` |
| Claude Code           | One file per memory, typed frontmatter, `MEMORY.md` index                      | The agent, with file tools, during the session                               | Index loaded (200 lines / 25 KB); files read on demand  |
| Gemini CLI            | `GEMINI.md` tiers plus a private `MEMORY.md` index and notes                   | The agent edits Markdown directly                                            | Index loaded; notes read on demand                      |
| Windsurf              | Memories per workspace                                                         | Automatic, or on request                                                     | Automatic                                               |
| Cline                 | Six Memory Bank files                                                          | The agent, when told "update memory bank"                                    | All files read every task                               |
| Cursor                | Rules (`.mdc`); Memories removed in 2.1                                        | User                                                                         | Rules by glob or always                                 |
| Aider                 | `CONVENTIONS.md`                                                               | User                                                                         | Loaded read-only                                        |
| pi                    | None built in; jayzeng/pi-memory adds tools and qmd search                     | Extension tools; handoff note at compaction                                  | Up to 16k characters per turn                           |
| Anthropic memory tool | Files under `/memories`, client-side                                           | The model, through six file commands                                         | Model told to view the directory first                  |

### Codex CLI

The most complete design for a coding agent, and close to what tau
needs. Off by default behind the `memories` feature
(`features/src/lib.rs`).

- **Phase 1, per session.** Once a session has been idle 6 hours
  (`min_rollout_idle_hours`) and is at most 10 days old, a low-effort
  model reads the rollout (up to 70% of its context window), with
  secrets redacted first, and returns `{rollout_summary, rollout_slug,
raw_memory}` (`memories/write/templates/memories/stage_one_system.md`).
  - The prompt has a **minimum signal gate**: return empty fields when
    nothing durable was learned.
  - Each task is marked success, partial, uncertain or fail, and memories
    are sorted into preference signals, reusable knowledge, failures and
    how to avoid them, and references, each tied to one `cwd`.
- **Phase 2, global.** Under a lock, the top stage-1 outputs by
  `usage_count` then recency (at most 256; unused for 30 days drops out)
  are written to a git-tracked folder, a diff is computed, and a
  consolidation sub-agent without network edits `MEMORY.md`,
  `memory_summary.md` and `skills/*/SKILL.md` (`consolidation.md`, 880
  lines). Claims supported only by deleted inputs are removed.
- **Read path.** `memory_summary.md` goes into the developer
  instructions (2.5k tokens). The agent is told to do a quick pass: grep
  `MEMORY.md`, open one or two summaries, in 4 to 6 steps, say when a
  fact comes from memory and may be stale, and cite what it used in an
  `<oai-mem-citation>` block. Citations bump `usage_count`, which ranks
  what phase 2 keeps (`ext/memories/templates/memories/read_path.md`).
- **Corrections.** The agent may only add ad-hoc notes, and only when
  asked; consolidation treats them as data. Users reset with
  `codex debug clear-memories` or `/memories`.
- **Known limits.** Grep misses paraphrases; items past the caps are
  dropped silently (Mem0's write-up of it, `How Memory Works in Codex
CLI`).

### Claude Code

Scoped `CLAUDE.md` files (managed, user, project, local, subdirectory,
`@imports`, path-scoped rules) hold what must always apply. Auto memory
keeps one file per memory under `~/.claude/projects/<project>/memory/`
with a `MEMORY.md` index of one line each; the harness enforces the
index budget and returns an error so the model rewrites it when full.
Types are `user`, `feedback`, `project` and `reference`; memories skip
what the code or git history already records.

### Others worth one line

- **Gemini CLI** removed its `save_memory` tool: the agent edits
  Markdown directly, in exactly one of four tiers per fact, never copying
  a fact across tiers (`prompts/snippets.ts`).
- **Hermes agent** keeps `MEMORY.md` and `USER.md` at about 1,300
  tokens together, frozen at session start for prompt caching; its
  memory tool rejects duplicates and scans writes for prompt injection
  and credentials; before compression it runs one call with only the
  memory tool (`I Read Hermes Agent's Memory System`).
- **Cursor** shipped Memories and removed them; leftovers could only be
  deleted by downgrading. Memory the user cannot see or remove is a
  liability.

## Memory frameworks

### Letta (formerly MemGPT)

- **V1 server** (now on the `archive` branch): core memory blocks with a
  character limit, always in the system prompt, edited by the agent
  through tools; archival memory searched with tags and dates; sleep-time
  agents running every N turns.
- **Letta Code, current: MemFS.** A git-backed Markdown directory. Root
  files are always in context; child directories are deferred, each with
  a `MEMORY.md` index; every file has `name` and `description`
  frontmatter checked by a pre-commit hook; `[[path]]` links are
  "synapses". A background reflection subagent ("dreaming") runs on step
  count or at compaction and resolves contradictions "in favor of the
  latest evidence". This is nearly tau-memory's plan.
- Letta's own LoCoMo run of a plain filesystem agent scored 74.0% with
  gpt-4o-mini (vendor-run), above several dedicated memory systems; its
  conclusion is that memory is more about managing context than the
  retrieval mechanism.

### Mem0

- **Paper (April 2025):** extract facts per message pair; retrieve the 10
  most similar memories; an LLM picks ADD, UPDATE, DELETE or NOOP.
  Mem0g adds a Neo4j graph where conflicting relations are marked
  invalid.
- **Now:** single-pass, **ADD-only** extraction with hash dedup; graph
  memory moved to the paid platform. Their reason: consolidation at
  write time "was where context got destroyed". Retrieval fuses
  semantic, BM25 and entity-overlap scores, plus a temporal score from
  time metadata classified at write time, used to rerank and never to
  filter.
- **Admitted weakness:** knowledge updates stay the hardest category for
  an add-only store, because old similar facts still surface. The
  platform's "Dream" pass supersedes (marks and links, never deletes),
  merges duplicates and synthesizes patterns that link to their sources.
- LoCoMo numbers are vendor-run and disputed: Zep says Mem0 misconfigured
  Zep and reports 75.14 ± 0.17 for itself; Letta could not reproduce
  Mem0's MemGPT run.

### Zep and Graphiti

A temporal knowledge graph: episodes (raw input, never lost), entities,
facts on edges with **bi-temporal** validity (`valid_at`/`invalid_at` in
the world, `created_at`/`expired_at` in the system), and communities.
One LLM call per new fact finds duplicates and contradictions; a
contradicted fact is expired, not deleted. Retrieval mixes cosine, BM25
and graph traversal with several rerankers and returns about 1.6k
tokens. Costs: heavy LLM ingestion, rate limits, results that improve
hours after ingestion (Mem0's observation), and 116 s or more per query
in an independent benchmark.

### LangMem and Cognee

- **LangMem** separates semantic (collections or a single profile),
  episodic and procedural memory (a system prompt optimized from
  feedback). Writes happen in the hot path through a tool or in a
  debounced background manager, with deletes off by default. No links.
- **Cognee** builds graph, vector and relational stores; nodes carry
  `valid_to` for supersession; an optional contradiction detector only
  records `contradicts` edges. Its own BEAM report says no single
  retrieval setting fits all question types.

## Linked notes and research systems

### A-MEM (Zettelkasten-inspired, NeurIPS 2025)

Each note has content, time, keywords, tags, a one-sentence context, an
embedding and a list of linked note ids. On every write an LLM fills the
keywords and context, the 10 to 50 nearest notes are retrieved, an LLM
picks links among them, and "memory evolution" rewrites neighbours'
context and tags. Retrieval returns the top k plus linked neighbours
within the same k.

- **Ablation (self-reported, LoCoMo multi-hop F1):** 9.65 with neither
  linking nor evolution, 21.35 with linking, 27.02 with both.
- **Independent runs disagree.** Mem0's paper puts A-MEM last among the
  systems it tested; the agent-native memory benchmark
  ([arXiv 2606.24775](https://arxiv.org/abs/2606.24775), `Are We Ready
For An Agent-Native Memory System`) finds weak answer F1 but among the
  best evidence recall (R@5/R@10 of 69.5/85.9) and stable recall as
  evidence grows far apart, where flat embedding retrieval "drops
  sharply".
- **Risks:** about 13 LLM calls per response (MemoryOS's count); rewriting
  neighbours repeatedly causes semantic drift; links are untyped, so
  "replaces" and "see also" look the same.

### akitaonrails/ai-memory

A Rust server (MCP and HTTP, about 269k lines) used by more than 20
harnesses through hooks.

- **Storage:** a git-backed wiki of Markdown pages is the truth; SQLite
  (FTS5, links, entities, embeddings, evidence, audit) is derived and
  rebuilt from it, with a watcher for hand edits.
- **Types by path:** `sessions/`, `concepts/`, `decisions/`, `gotchas/`,
  `procedures/`, `_rules/`, pinned `_slots/`, `_global/`. Frontmatter
  follows Open Knowledge Format v0.2.
- **Links:** `[[path]]`, across projects too; links to pages that do not
  exist yet are kept and resolve later; typed relations come from a
  closed set (`causes`, `fixes`, `contradicts`), because "a free-text
  relation column becomes an unqueryable folksonomy".
- **Writes:** hooks capture events without an LLM; session end writes a
  rule-based session page; a durable job consolidates it into 1 to 5
  pages with a strict faithfulness prompt that allows "no durable
  insight"; an hourly reviewer patches rules and procedures. Pages are
  superseded, never deleted.
- **Retrieval:** FTS5, entity matches, link neighbours and optional
  MiniLM vectors fused by RRF, with a bounded authority boost by kind.
  LongMemEval-S hit@5 went from 0.617 (FTS) to 0.823 with local
  embeddings. Recall is pull-only beyond a small brief at session start.
- **Weak spots seen in the code:** consolidation does not read related
  pages before writing, so a page at an existing path supersedes it
  unseen; pages are session-sized rather than atomic; contradiction
  handling is advisory.

### Basic Memory

Markdown notes with observations (`- [category] text #tag`) and typed
relations (`- relation_type [[Target]]`; a bare `[[x]]` is `links_to`).
The index stores relations and computes backlinks; links to deleted
notes keep `to_id = NULL`. The agent writes through MCP tools and humans
edit the same files. `build_context` follows relations both ways to a
depth of 1 to 3, capped at 10 related notes by default. Its capture
model depends on the agent remembering to write, which ai-memory's
author names as its main weakness.

### Other systems

- **Generative Agents:** a memory stream scored by recency (decaying from
  the last access), importance (rated once, at write) and relevance;
  reflections are notes that cite their evidence. Removing reflection
  cut believability from 29.89 to 26.88.
- **HippoRAG 2:** an LLM-built graph searched with Personalized PageRank
  seeded from matches against whole triples and passages. Matching
  against content rather than entity names mattered most (multi-hop R@5
  87.1 against 59.6). Indexing is expensive (9.2M tokens for 11.6k
  passages).
- **MemoryOS / MemOS:** OS-style tiers with promotion by "heat", and
  memory units with provenance, versions and lifecycle states. Headline
  numbers are vendor-run and not reproduced.
- **CompassMem** (`Memory Matters More`): events with typed edges
  (causal, temporal, part-of), merged with their nearest equivalent or
  linked when related, and searched by expanding from embedding seeds
  along the edges. Removing edges, or using fixed chunks instead of
  events, hurt multi-hop and temporal questions most.

## What the evidence says

From the papers in `~/Notes/resources/ai-ml`, read in full:

- **Structure pays off for distant, scattered evidence, not for top-1
  lookups** (`Are We Ready For An Agent-Native Memory System`, RQ2 and
  RQ4; CompassMem's ablation).
- **Full text beats compression and summaries.** Raw text was best on
  all four metrics; compression cut LongMemEval substring EM from 26.0
  to 10.7. Fine-grained LLM fact extraction hurt reasoning badly (MemOS
  "Fine" 2.5 against "Fast" 25.5 EM on LoCoMo). Atomic should mean one
  coherent idea in full prose, not one extracted fact.
- **Similarity-based memory forgets, by construction.** `The Price of
Meaning` proves that retrieval by a similarity threshold over a
  semantic space eventually suffers interference and false recall; its
  graph with cosine-derived edges forgot like the vector store (b =
  0.478 against 0.440). BM25 did not forget but agreed with semantic
  retrieval on only 15.5% of queries. Whether ColBERT's summed
  token-level MaxSim falls under the theorem is untested. Explicit links
  and verbatim text are the kind of exact record the theorem exempts;
  near-duplicate density is the lever to keep low.
- **Update is the operation that matters, and searching first cuts
  waste.** Removing Update cost AtomMem 5 to 7 points, removing Delete
  about 1. AutoMem's keyed upserts and a consult-before-write habit cut
  redundant writes by 68 to 83%.
- **Keep an always-loaded note.** Removing AtomMem's scratchpad cost 6
  to 11 points.
- **Experience deserves its own note type.** Case memory of task,
  approach and outcome helps when tasks recur (Memento, Evo-Memory);
  unfiltered failures are noise; about 4 retrieved cases is best.
  MemoryBench finds existing systems no better than plain retrieval over
  feedback logs.
- **Coding agents search files well.** `Coding Agents are Effective
Long-Context Processors`: agents using `rg` and `sed` over a folder of
  files beat published retrieval systems; adding a retriever lowered
  scores and crowded out grep. Notes should be real files at meaningful
  paths, and docbert should be offered next to file tools, not instead
  of them.
- **Benchmarks are soft.** The LoCoMo answer key has known errors
  (an audit found about 6% wrong), its LLM judge accepts vague answers,
  and judge or prompt choices move results by double digits.

## A design for tau-memory

A proposal to argue with, not a decision.

### Notes

- **One idea per note,** a file at `<scope>/<type>/<slug>.md`, title
  stating the claim, body in full prose with exact versions, flags,
  paths and error strings.
- **Frontmatter:** `id` (stable), `title`, `description` (one line, for
  the index), `type`, `tags`, `created`, `updated`, `valid_from`,
  `valid_to`, `source` (run, turn, commit, files; said by the user, done
  by the agent, or inferred), and `links`.
- **Types,** a closed set: `fact` (how the repository or tools are),
  `convention`, `decision` (what was chosen and why), `gotcha` (symptom,
  cause, fix), `procedure`, `case` (task, approach, outcome, including
  failures), `preference`, and `index` for hub notes. Task progress and
  TODOs are not memory.
- **Link types,** a closed set: `relates` (the default for a bare
  `[[x]]`), `refines`, `supersedes`, `contradicts`, `derived_from` (a
  summary or hub pointing at its sources), `about` (a file, crate or
  symbol in the repository). Backlinks live in the index, not in files;
  links to notes that do not exist yet are kept and resolve later.
- **Scopes:** the repository (per project), the user (preferences across
  projects), and later a team scope. A fact lives in exactly one scope.

### Storage

- Notes are the truth; docbert's index and a link table are derived and
  rebuilt from them. The index records the model and dimension it was
  built with, so a model change reindexes.
- The notes directory is versioned (a jj repository, like the
  project's), which gives history, diffs of what a pass changed, and
  restore.
- Small collections use exhaustive MaxSim; PLAID starts past a size
  threshold and updates in place per note.

### Reading

- **At `start`:** the scope's index note, frozen for the run, within a
  hard budget (the plan today sends search hits instead; the index is
  what every shipped system relies on). Past the budget the plugin says
  so rather than truncating silently. Optionally, the top few search
  hits for the task, fenced as untrusted data.
- **In the loop:** `memory_search` returns ids, titles, types,
  descriptions and snippets from docbert, then adds one hop along links
  within the same result budget. `memory_read` returns a note with its
  links and backlinks. Notes are also plain files the agent can grep.
- **Current over superseded:** superseded notes rank lower and are
  labelled, never hidden, since history is often the answer.
- **Citations:** the agent says which notes it used, as Codex does; use
  counts and last use feed ranking and later pruning.

### Writing

- **In the loop:** `memory_write` searches first and shows the nearest
  notes, so the agent can update one, supersede it, or link to it
  instead of duplicating it. A decision or a correction from the user is
  the moment to write.
- **Before compaction:** one turn with only the memory tools, since
  compaction is where details are lost and the harness controls it.
- **After a run, in the background:** a consolidation pass over the
  transcript proposes notes with the Codex and ai-memory rules: a
  minimum signal gate, faithfulness to the transcript, no invented
  paths, and search before write. Its changes land as one versioned
  commit the user can review or revert. Off by default until the
  evaluation supports it.
- **Safety:** redact secrets before anything reaches a note; scan writes
  for prompt injection; never let memory stand in for rules.

### Measuring

- Retrieval recall at 5 and 10 against gold note ids, split by how far
  apart the evidence is.
- Coding tasks that depend on something learned in an earlier run, with
  and without memory, and after the fact changed (is the stale note
  still used?).
- LLM calls and tokens per write, query latency, and the index note's
  size over time.
- Baselines: no memory, a single `MEMORY.md`, and plain docbert over raw
  transcripts.

## Changes to the current plan

| Plan today                                      | Proposed                                                                                           |
| ----------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `start` puts search hits into the context       | `start` puts the index note in; search is a tool, with an optional few hits                        |
| Links untyped (`memory_link { from, to, why }`) | A closed set of link types; `why` stays as the link's text                                         |
| Notes updated in place                          | Changed facts supersede, with `valid_from`/`valid_to`                                              |
| `finish` distillation with the agent's model    | A background consolidation pass with a signal gate and search-before-write, as a reviewable commit |
| No write at compaction                          | A memory-only turn before compaction                                                               |
| No types                                        | A closed set of note types, including cases and gotchas                                            |
| One notes directory per `Memory`                | Scopes: repository and user, one scope per fact                                                    |

## Decisions

Asked and answered on 2026-09-29; the plan in plugins.md follows them.

| Question                    | Decision                                                                                               |
| --------------------------- | ------------------------------------------------------------------------------------------------------ |
| Where repository notes live | tau's data directory, private and versioned; nothing in the project's history                          |
| Background consolidation    | Built, off until the evaluation shows it helps                                                         |
| Context at `start`          | The index note plus the top few search hits, fenced as untrusted                                       |
| The index note              | Written by the agent, about 2k tokens; a write past the budget is refused with a request to rewrite it |
| Note and link types         | The full proposed sets                                                                                 |
| Interference                | Measured in the evaluation: recall as near-duplicates pile up, for BM25, ColBERT and the hybrid        |
| Staleness against code      | Notes `about` a file are flagged when a later commit touches it                                        |

## Sources

- Codex: `codex-rs/memories/`, `codex-rs/ext/memories/`,
  `codex-rs/state/memory_migrations/0001_memories.sql`,
  `codex-rs/config/src/types.rs`.
- Claude Code memory: <https://code.claude.com/docs/en/memory>.
  Anthropic memory tool:
  <https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool>.
- ai-memory: <https://github.com/akitaonrails/ai-memory>, with
  `docs/ARCHITECTURE.md`, `crates/ai-memory-consolidate/prompts/`, and
  the author's posts of 2026-05-23, 2026-06-14 and 2026-09-02 on
  <https://akitaonrails.com>.
- Letta: <https://github.com/letta-ai/letta-code>
  (`src/agent/prompts/letta_local_memfs.md`,
  `src/agent/subagents/builtin/reflection-v2.md`),
  <https://www.letta.com/blog/benchmarking-ai-agent-memory/>.
- Mem0: <https://arxiv.org/abs/2504.19413>, `docs/migration/oss-v2-to-v3.mdx`,
  `docs/platform/features/dream.mdx`; Zep's reply:
  <https://blog.getzep.com/lies-damn-lies-statistics-is-mem0-really-sota-in-agent-memory/>.
- Zep: <https://arxiv.org/abs/2501.13956>; Graphiti:
  <https://github.com/getzep/graphiti>.
- LangMem: <https://github.com/langchain-ai/langmem>. Cognee:
  <https://github.com/topoteretes/cognee>.
- A-MEM: <https://arxiv.org/abs/2502.12110>. Basic Memory:
  <https://github.com/basicmachines-co/basic-memory>. Generative Agents:
  <https://arxiv.org/abs/2304.03442>. HippoRAG 2:
  <https://arxiv.org/abs/2502.14802>. MemoryOS:
  <https://arxiv.org/abs/2506.06326>. MemOS:
  <https://arxiv.org/abs/2507.03724>. LoCoMo audit:
  <https://github.com/dial481/locomo-audit>.
- In `~/Notes/resources/ai-ml`: `Are We Ready For An Agent-Native Memory
System`, `The Price of Meaning - Why Every Semantic Memory System
Forgets`, `AtomMem`, `AutoMem`, `MemEvolve`, `Evo-Memory`,
  `MemoryBench`, `Memory Matters More`, `Memento`, `Coding Agents are
Effective Long-Context Processors`, `How Memory Works in Codex CLI`,
  `Why long-term memory for LLMs remains unsolved`, `I Read Hermes
Agent's Memory System`, and the other articles under
  `retrieval-agents/`.
