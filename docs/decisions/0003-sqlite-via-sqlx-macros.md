# 0003: SQLite through stock sqlx, with checked macros

- Status: accepted (supersedes an earlier choice of Turso)
- Date: 2026-09-26

## Context

pi stores sessions as one JSONL file each. The audit found four problems
with that:

- The current leaf is lost on reload.
- There is no locking.
- Loading a file can write to it.
- Session directory names collide: `/a-b` and `/a/b` map to the same
  directory.

Turso was considered first. It needs a third-party SQLx adapter,
`sqlx-turso` 0.1.0-alpha.1, which ships a single release. That adapter:

- has no linked repository;
- pins a Turso pre-release;
- uses its own macros, and those cannot type-check bind parameters.

We want stock sqlx macros on every query.

## Decision

- Use embedded SQLite, via the `sqlite` feature of `sqlx`, which bundles
  SQLite.
- Every query goes through one of `sqlx::query!`, `query_as!` or
  `query_scalar!`.
- Migrations run through `sqlx::migrate!`.
- The offline query metadata in `.sqlx/` is committed, so crates that
  depend on `tau-store-sqlite` build without a database.
- Use WAL mode.
- All writes go through a pool with a single connection and
  `BEGIN IMMEDIATE`.
- All reads go through a separate read-only pool.

## Consequences

- Only one write transaction runs at a time. Each turn's appends are
  batched into one transaction that takes milliseconds, while a turn
  takes seconds. We will measure time spent waiting for the writer
  connection from the start.
- `WITH RECURSIVE` is available, so a fork's full transcript is a single
  query.
- Other processes can read the database while workflows run, on a local
  disk only.
- Schema and workflow: [`../reference/storage.md`](../reference/storage.md).
