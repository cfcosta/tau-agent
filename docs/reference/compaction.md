# Compaction

This is ported from pi's coding-agent compaction
(`packages/coding-agent/src/core/compaction`). pi has four compaction
implementations, and its summarization prompts have drifted apart. This
one is the version pi ships.

Compaction is **off by default**. Most workflow runs are short. Turn it on
for long-running agents with `Agent::compaction(Compaction::default())`.

## When it triggers

- **Threshold.** After a turn, compaction runs if
  `estimated_tokens > context_window - reserve_tokens`.
  - `reserve_tokens` defaults to 16,384.
  - It is checked at most once per turn.
- **Overflow.** If the response fails with `context_length_exceeded`, the
  loop compacts once and retries once. A second overflow fails the run.

## Token estimate

1. Start from `usage.input + usage.output` of the last successful
   assistant message.
2. Add `chars / 4` for every message after it.
3. Images count as 4,800 characters.

## Cut point

1. Walk backwards from the newest message, adding up estimated tokens,
   until `keep_recent_tokens` (default 20,000) is reached.
2. Snap to a valid cut point: never between a tool call and its result,
   and never on a tool result.
3. If the cut falls inside a single oversized turn, summarize that
   turn's prefix separately and merge the two summaries.

## Summary request

- **Model.** The summary is one request to the run's model. It is not
  sent on the run's lane; it uses a short-lived lane of its own.
- **Input.** The messages being dropped are serialized into a single user
  message inside `<conversation>…</conversation>`.
- **Prior summary.** If one exists, the request uses the "update" variant
  of the prompt and includes the prior summary.
- **Output limit.** `max_output_tokens` is `0.8 × reserve_tokens`.
- **Structure.** The summary always has these headings:

```
## Goal
## Constraints
## Progress
### Done
### In Progress
### Blocked
## Key Decisions
## Next Steps
## Critical Context
```

- **File lists.** Lists of files read and modified are appended. They are
  taken from the `path` argument of tool calls and carried forward
  between compactions.

## After compaction

- A `messages` row is written with `kind = 'compaction'`. Its body holds
  the summary, the token count before compaction, and the file lists.
- A `Compacted` event is emitted.
- The next turn sends a full request, because the transcript changed. The
  WebSocket continuation chain restarts from there.
