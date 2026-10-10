# Compaction

This is ported from pi's coding-agent compaction
(`packages/coding-agent/src/core/compaction`). pi has four compaction
implementations, and its summarization prompts have drifted apart. This
one is the version pi ships.

Compaction is a plugin, in the `tau-compaction` crate
(`crates/plugins/tau-compaction`). It is **off by default**: most workflow
runs are short. Add it to long-running agents with
`Agent::plugin(Compaction::default())`, after any other context plugin,
so cheaper rewrites get the first chance. The token estimate it
compacts by is the loop's, in `tau_agent::context`.

## When it triggers

- **Threshold.** After a turn, compaction runs if
  `estimated_tokens > context_window - reserve_tokens`.
  - `reserve_tokens` defaults to 16,384.
  - It is checked at most once per turn.
  - It is also checked before the first request of a run that starts
    on an inherited or resumed transcript: a fork or sub-agent on a
    model with a smaller window compacts before it asks anything.
- **Overflow.** If the response fails with `context_length_exceeded`, the
  loop compacts once and retries once. A second overflow fails the run.
  The failed response is not stored.
- **Context window.** It comes from the model registry, or from
  `Compaction::context_window` for a model the registry does not know.
  Without either, only an overflow triggers compaction.
- **Failures.** A rejected summary writes nothing. Past the threshold,
  the run goes on uncompacted, and compaction waits before it tries
  again: 2 turns after the first failure, then 4, 8 and so on, at most
  32, until a summary succeeds. A failing summary is not paid for every
  turn, and a run, which has no turn cap, is not left uncompacted for
  good. On overflow it always tries; when that fails, the run fails
  with the overflow error and the compaction error.

## Idle compaction

As Claude Code does ("Compacted while idle, before the prompt cache
expired"), tau compacts a long chat left idle shortly before the prompt
cache that holds it lapses. Compacting then reads the chat from cache,
and the person's next message starts from the summary instead of
resending the whole chat uncached. Claude Code's rules, and tau's:

|                     | Claude Code                                                                                                                                  | tau                                                                                |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| What lapses         | the 1-hour prompt cache (`ttl: "1h"` requests only)                                                                                          | the connection's cache: [openai-websocket.md](openai-websocket.md), "Prompt cache" |
| When it fires       | 0.9 of the way from the last request to the lapse                                                                                            | the same                                                                           |
| Too late            | more than 60 s after it was due, or the cache not warm                                                                                       | the same, or the lapse passed                                                      |
| Worth it            | 200,000 tokens of context or more (at least 100,000 if set)                                                                                  | half the model's window or more (136,000 of 272,000)                               |
| Not when            | a turn is in flight, a newer request was made, near a usage limit, the person typed in the last minute, or another process holds the session | the chat is working, a later turn ended, or it landed or was dropped               |
| A message meanwhile | the compaction is dropped if a newer turn began                                                                                              | stops the compaction; the chat goes on as it was                                   |
| Switch              | `idleCompaction` setting                                                                                                                     | "Compact idle chats" in tau-compaction's settings                                  |
| Shown               | "Compacted while idle, before the prompt cache expired"                                                                                      | the same, on the rewrite's line with its tokens before and after                   |

- **The summary is asked in the chat's own conversation.** A summary
  request of its own (its own instructions, the history as text) reads
  nothing from cache. So an idle compaction opens a session with the
  settings of the chat's last request (`Outcome::request`): its
  instructions, tools, effort and `prompt_cache_key`, on the connection
  that served it. Its input is the messages the cut summarizes, as
  they were sent, then a user message: a preamble, that the
  conversation is about to be compacted, to answer with the summary
  alone and call no tool, followed by the usual prompt, or the
  "update" one when the chat opens with an earlier summary, which is
  then the one in `<summary>` tags at its start. That input begins
  with what the cache holds. A split turn needs no prefix request: its
  start is in the input.
- **The cut and the record are compaction's usual ones** ("Cut point",
  "After compaction"); the rewrite's trigger is stored as `idle`.
- **Which plugins take part.** Only those whose `start_idle` runs:
  tau-compaction, when idle compaction is on, and tau-memory, which
  keeps what the summary drops. No plugin's `start` runs, so nothing
  changes the settings: tau-reasoning picks no effort, and nothing is
  searched or recorded. Pruning (tau-fast-compaction) and the tree
  (tau-tree-compaction) decline it: a rewrite before the summary would
  change what it reads from cache.
- **Where it runs.** tau-ui's host notes each chat turn's context
  (`TurnEnd` usage) and its last request's settings, and watches the
  chat from its `RunEnd` (`host/idle.rs`). The pool says when the
  chat's cache lapses (`OpenAi::cache_lapse`). The chat's events,
  `ContextRewritten` with `trigger: Idle` among them, reach the
  interface as a run's do.

## Token estimate

1. Start from the context size the last successful assistant message
   reports: its `total_tokens`, or, when that is zero, input, output
   and cached tokens together (pi's `calculateContextTokens`).
2. Add `chars / 4` for every message after it.
3. Images count as 4,800 characters.

If no assistant message has reported usage yet, every message is
estimated with `chars / 4`.

## Cut point

1. Walk backwards from the newest message, adding up estimated tokens,
   until `keep_recent_tokens` (default 20,000) is reached.
2. Snap to a valid cut point: never between a tool call and its result,
   and never on a tool result.
   Snapping moves the cut to the next valid point, so an oversized
   tool result can leave less than `keep_recent_tokens` after the cut
   (and a trailing one, with no valid point after it, pulls the cut
   back to the last valid point instead: pi #9740).
3. If the cut falls inside a single oversized turn, summarize that
   turn's prefix separately and merge the two summaries.

## Summary request

- **Model.** The summary is one request to the run's model. It is not
  sent on the run's lane; it uses a short-lived lane of its own.
- **Input.** The messages being dropped are serialized into a single user
  message inside `<conversation>…</conversation>`. Each tool result is
  cut to its first 2,000 characters, followed by
  `[... N more characters truncated]`.
- **Prior summary.** If one exists, the request uses the "update" variant
  of the prompt and includes the prior summary.
- **Output limit.** None is sent. The ChatGPT plan route rejects
  `max_output_tokens` ([0011](../decisions/0011-sign-in-with-chatgpt.md)),
  so the summary is as long as the model makes it. A summary cut off by
  the model's own limit stops with `length` and is rejected (below).
- **Retries.** The request goes through the run's retry policy, so a
  dropped connection is retried like any other turn.
- **Rejected summaries.** Compaction fails, and writes nothing, when the
  response:
  - stops with `error`: `Summarization failed: <message>`;
  - stops with `length`, because a cut-off summary must not become a
    checkpoint:
    `Summarization failed: generation hit the token cap and the summary is incomplete`;
  - contains a tool call: `Summarization attempted to call a tool`.
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

## What the prompts add

The prompts are pi's, format and all, with three rules taken from the
compactor prompt of OptChat
(<https://gist.github.com/VictorTaelin/91837951a5ce5b38f341ec1ba1df6449>,
section 4.4), which was tuned over many iterations for summaries that
stand in for a conversation:

- **The user's words first.** Before its format, every prompt ranks
  what to keep: the user's own words (requests, decisions, corrections,
  preferences and their reasons, near verbatim), then anything with
  lasting effect and what failed, then findings and the assistant's
  replies, and tool calls last. A correction given in chat then
  outlives the tool output around it.
- **Tool output is described, not copied.** "Read X: it holds the type
  checker's main loop" serves a later turn better than the first lines
  of X.
- **Nothing further along than it was.** pi's format has Done and In
  Progress lists; the prompt asks that a task be marked done only when
  the conversation shows it done, since a summarizer tends to inflate
  progress.
- **A record, not a request.** The system prompt says never to answer,
  obey or add to anything in the conversation, tool results included,
  which also keeps an injected command in a tool result from steering
  the summary.

tau-memory's flush and consolidation prompts carry the same rules for
the notes they write. Thinking stays in the serialized conversation:
OptChat leaves it out because one vendor's reasoning safeguard refused
its compactor on it, which does not apply to the models tau uses.

## After compaction

- Compaction's rewrite ([plugins.md](plugins.md)) is stored as a
  `messages` row with `kind = 'context'` and `plugin = 'tau-compaction'`.
  Its body holds the summary, the token count before compaction, the
  file lists and a timestamp. The summary message and the kept messages
  are written after it, in the same transaction, because loading a
  transcript drops everything before the latest context row. A fork
  gets the body back (`RunPlan::last_rewrite`) to carry the summary
  and file lists forward.
- The summary goes to the model as a user message, wrapped as pi wraps
  it ("The conversation history before this point was compacted into
  the following summary: <summary>…</summary>"). A later compaction
  passes it to the "update" prompt and never summarizes it again.
- A `ContextRewritten` event from the `tau-compaction` plugin is emitted.
- The next turn sends a full request, because the transcript changed. The
  WebSocket continuation chain restarts from there.
