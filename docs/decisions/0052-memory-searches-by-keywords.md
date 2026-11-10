# 0052: memory searches by keywords

- Status: accepted.
- Date: 2026-11-09

## Context

tau-memory searched its notes with ColBERT: docbert's model
(`lightonai/GTE-ModernColBERT-v1`, through `docbert-pylate` and candle)
encoded each note, the embeddings were cached beside the notes, and a
query ranked them by MaxSim. It replaced BM25 on 2026-09-29, after the
retrieval evaluation (`eval/harbor.toml`) showed it finding paraphrased
queries that BM25 missed. BM25 stayed behind the `Index` seam for tests,
and the host picked one through the `tau.memory-search` service.

ADR 0050 moved every portable plugin of tau's to Luau and kept
tau-memory in Rust, for its embeddings and its files. The embeddings
are the part Luau cannot reach: a model loaded in process, run on
candle, with a cache of tensors on disk. Files are a reach Luau can be
given. While memory encodes notes with a model, it cannot move.

## Decision

Memory searches by keywords alone: the in-process BM25 index, over each
note's title, description, tags and body. The embedding goes, whole:

- `colbert` (the MaxSim index, its encoder trait and its cache) and
  `docbert` (the model as an encoder), and the `docbert` feature.
- `Search` and the `tau.memory-search` service: there is one way to
  search, so nothing chooses one.
- The `Index` trait: BM25 is the only index, and `Memory` holds it
  directly.
- The retrieval evaluation (`eval.rs`, `eval/harbor.toml`,
  `tau-memory-eval`): it compared BM25, ColBERT and the two fused, and
  with one of them left it measures nothing to choose between.
- In tau-memory-e2e, the ColBERT arm, its `docbert` feature and
  `--keywords`: memory and transcript search are BM25 there too.
- `docbert-pylate` and candle, from the workspace, `deny.toml` and the
  flake.

Recall is unchanged: superseded notes rank lower, and one hop along
links reaches what search did not.

## Consequences

- tau-memory no longer needs anything Luau lacks but its files, and
  can be ported once Luau reaches them.
- A query worded unlike the note it wants finds it less often. The
  agent writes the queries and can try other words; links carry what
  the words miss.
- The app no longer downloads or loads a model for memory, and the
  embeddings caches beside each scope's notes (`memory-embeddings`) are
  no longer written. Old ones are left on disk; nothing reads them.

## Alternatives

- **Keep ColBERT behind the seam, in Rust, for a later Luau port to
  call.** The port would then rest on a host service for the one thing
  that kept memory Rust, and the model would still be loaded.
- **Keep the evaluation, BM25 only.** It tracked BM25 as near-duplicates
  pile up, but without a second index it decides nothing.
