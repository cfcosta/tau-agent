# Fast compaction

`tau-fast-compaction` (`crates/plugins/fast-compaction`) prunes stale
tool history from a run's context. It asks Jev, TypeSafe's System One
model, which tool calls and results still matter, and drops or cuts the
rest. It never summarizes: text stays verbatim. When pruning cannot free
enough, summarizing compaction takes over.

It is a port of
[`joelhooks/pi-fast-jev-compaction`](https://github.com/joelhooks/pi-fast-jev-compaction),
itself built on
[`tamaratran/fast-jev-compaction`](https://github.com/tamaratran/fast-jev-compaction),
onto tau's context seam ([plugins.md](plugins.md)). Both are MIT; the
notices are in the crate's `THIRD_PARTY_NOTICES.md`.

## Using it

```rust
use tau_compaction::Compaction;
use tau_fast_compaction::FastCompaction;
use tau_jev::TypeSafe;

let agent = Agent::new(OpenAi::from_env()?)
    .plugin(FastCompaction::new(TypeSafe::from_env()?))
    .plugin(Compaction::default()); // after: it takes what pruning declines
```

`TypeSafe::from_env` reads `TYPESAFE_API_KEY`. Context plugins are
offered the context in the order they were added, so add fast
compaction before summarizing compaction.

## When a pass runs

- **Between turns,** when both hold:
  - the loop's token estimate has passed `compact_at_percent` (60%) of
    the context window; the window is `Settings::context_window`, or the
    model registry's;
  - the context has grown by `cooldown_tokens` (8,000) since the last
    pass that asked Jev.
- **On a context overflow,** always.
- A pass with no unpinned tool call to ask about ends without asking,
  and does not start the cooldown.

## What a pass does

1. **Pins** the first message and the last `preserve_recent` (6; at
   least 1). A call is pinned when its call or its result is. Pinned
   calls are always kept.
2. **Builds the state** Jev sees: the goal (`Settings::goal`, or the
   user's last three prompts), and the history, with each tool call's
   name, input and a note of its result's size and status. **Never the
   result itself.**
3. **Fits the state** to `max_state_tokens` (25,000) by shrinking, in
   order until it fits: tool inputs cut to 200, then 60 characters; long
   texts abridged to their head and tail; old texts collapsed; old calls
   compacted to one line; old messages without calls left out; runs of
   old calls merged. Pinned messages shrink last and are never
   collapsed or left out. A state that still does not fit fails the
   pass.
4. **Asks** two yes/no questions (Nouls) per unpinned call, in batches
   that fit `max_request_tokens` (30,000) with the state, sent together:
   - does knowing the call was made, with its input, still matter;
   - does its full output still need to stay verbatim.
5. **Decides** each call, at `keep_threshold` (0.5):
   - result still needed: **keep**;
   - only the call still matters: **drop the result**, keeping its first
     `head_chars` (300) characters and a note saying how much was cut
     and to re-run the tool;
   - neither: **drop the call** and its result.
6. **Merges** the decisions into the run's ledger. A call's decision
   only escalates: keep, then drop the result, then drop the call.
7. **Applies** the ledger to the transcript. An assistant message whose
   calls were all dropped, leaving no text, goes too. Everything else
   stays as it was, in order.
8. **Rewrites** the context only if the pruned transcript is at least
   `min_reduction_ratio` (25%) smaller, measured as JSON. Otherwise the
   pass declines, the ledger stays as it was, and the next context
   plugin gets the chance.

## Cost

- Every Jev request is charged to the run (`PluginCtx::charge`), at
  $0.042 per million input tokens, so it counts toward the run's limits
  and cost. A batch that was answered is charged even when another
  batch failed.
- A rewrite is one full resend of the transcript on the WebSocket; the
  requests after it are deltas again
  ([openai-websocket.md](openai-websocket.md)). The cooldown and the
  reduction threshold keep that rare.

## Failure

Any failure ends the pass without a rewrite and is reported as a
`PluginError` event: a Jev error (after its client's retries), an answer
that is missing, of the wrong kind or out of range, a state that cannot
fit, or a cancel. The run goes on with its transcript as it was; on an
overflow, the next context plugin is offered it.

## Storage and forks

A rewrite is stored as a `context` entry by `fast-compaction`, followed
by the pruned transcript ([storage.md](storage.md)). The entry's body
is the ledger and the pass's stats (calls, pinned, kept, results and
calls dropped, requests, the state's size and fitting stage, the
characters before and after, and the reduction). A fork whose latest
inherited rewrite is fast compaction's resumes its ledger from it.

## Where it differs from pi

- **Once, not every turn.** pi re-applies its ledger to the context of
  every request. tau stores the pruned transcript once, because each
  rewrite costs a full resend.
- **Only a saving worth a resend.** pi applies any saving and leaves
  its reduction threshold to the choice between itself and summary
  compaction. tau declines below the threshold.
- **The cooldown starts only with a pass that asked Jev.** pi starts it
  with any pass, so a pass with everything pinned held the next useful
  one back.
- **A message holding only thinking stays** unless pruning emptied it;
  pi drops every such message.
- **No settings file, command or status line.** Settings are a Rust
  value; events report each pass.
