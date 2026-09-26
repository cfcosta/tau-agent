# 0004: Coding tools live in an optional crate

- Status: accepted
- Date: 2026-09-26

## Context

pi ships seven coding tools: `read`, `bash`, `edit`, `write`, `grep`,
`find` and `ls`. They carry many behaviours that are worth keeping, such
as truncation limits, fuzzy edit matching, process-group kills, and
output spilling.

Many workflows never touch a filesystem. They call the user's own tools
instead.

## Decision

The coding tools live in `tau-tools`. The core crates do not depend on
it.

## Consequences

- The core stays at about 7.5k lines.
- `tau-tools` adds about 2k lines and roughly 1.5 weeks of work, after
  the core is done.
- The tools' behaviour spec lives in [`../reference/tools.md`](../reference/tools.md).
