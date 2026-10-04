# 0015: Delegates fork their caller, and run side by side

- Status: accepted. Amends [0009](0009-child-runs-land-on-their-parent.md):
  a sub-agent started by `delegate` starts with its caller's transcript,
  not with its task alone, and several of them run at once.
  Amended by [0016](0016-runs-nest-one-level.md): only a top-level run
  delegates. Amended by [0022](0022-the-prompt-cache-follows-the-connection.md):
  the inherited prefix hits the cache only on the caller's connection,
  which the sub-agent's first request now takes. Amended by
  [0026](0026-sub-agents-run-detached.md): `delegate` gives way to
  `spawn` and `wait`, and sub-agents outlive their call.
- Date: 2026-09-30
- Background: a comparison with Codex's multi-agent tools
  (`spawn_agent`, `wait_agent`, `send_input`, `close_agent`), which
  fork optionally, run children concurrently in one shared checkout,
  and let roles pick the model.

## Context

`delegate` hands a task to a sub-agent that works in a jj workspace of
its own and lands its commits on its caller (0009, 0014). Two things
about it get in the way:

- **It starts blank.** The sub-agent sees only `task`. The caller has
  to restate everything it learned (the files it read, what the user
  asked, what it tried) and it never restates all of it, so the
  sub-agent reads the same files again and misses the constraints the
  user gave. The prompt cache cannot help: nothing is shared.
- **It runs alone.** The tool is `Sequential`, so three delegate calls
  in one batch run one after another. Each sub-agent already has a
  workspace and a websocket lane of its own; nothing but the landing
  needs them to wait.

Codex has both (a `fork_context` flag, concurrent children), but its
children share one checkout, and edits race. tau's workspaces already
keep children apart, so tau can have both without that problem.

## Decision

### A delegate is a fork of its caller

`delegate` always forks. There is no blank mode: a model that wants a
clean context asks for one in words, and a blank child would need the
long restated task that forking exists to remove.

The child's transcript is:

1. **The caller's transcript**, by reference, up to its last stored
   entry: the child's run records the caller as its parent and a
   `fork_seq`, as a user's fork does, and inherits from the latest
   compaction on.
2. **The caller's pending turn.** The loop stores a turn only after
   its tools finish, so the assistant message that made the call is
   not in the store yet. The child stores its own copy as its first
   entry.
3. **An output for every call in that message**, so the child sees
   the whole batch and knows its own part in it:
   - its own call: that it is the sub-agent running this call, that
     its task follows, and that it does only that task;
   - a sibling `delegate` call: that another sub-agent runs it;
   - any other call: that its result is not available here.
4. **Its task**, as a user message.

The run kind stays `subagent`: forking says what it inherits, not what
it is. `subagent` runs gain a `fork_seq`, and inheritance follows any
run that has one. `Agent::as_tool` keeps its blank sub-agents, for
library users that call one agent from another.

### The call can pick a model and an effort

```json
{ "task": "…", "model": "gpt-5.5-mini", "effort": "low" }
```

- Both are optional and default to the caller's. The schema lists the
  catalog's models and the effort levels, so a typo fails validation.
- Another model cannot reuse the caller's prompt cache. The call pays
  full price for the inherited context, and the tool description says
  so.
- Reasoning items are replayed only to the model that wrote them. The
  history loader drops the reasoning from another model's messages,
  so a resumed run on a new model gets the same fix.
- **A context too big for the child's model is compacted first.**
  Before its first request, a run that inherited a transcript asks its
  compaction plugins, with a new `Trigger::Start`, whether to rewrite
  it for the model's window. A user's fork onto a smaller model gets
  the same check.

### Several delegates run at once

- The tool is `Grouped`, a new execution mode: the delegate calls in
  a batch run side by side, and the batch's other tools run before or
  after them, so no tool edits the caller's files while a landing
  moves its working copy. A semaphore lets **four** sub-agents per
  caller run at a time; the rest wait for a slot.
- Every sub-agent in a batch starts on the caller's head at the call.
  The caller waits for the batch, so its head does not move until
  they land.
- **Landing is serialized, in the order they finish** (0009). The
  first restacks onto an unmoved caller and cannot conflict. Each
  later one restacks onto the one before.
- **A conflict lands as a jj conflict, and the caller resolves it.**
  The tool's result names the conflicted files, and the caller's
  `vcs_status` shows them. This is the flow a user's landing already
  uses (0014), with no confirmation: the caller asked for the work,
  and dropping it would waste it.
- **No nesting.** A sub-agent does not get `delegate`. Every level
  would copy the whole context again and add a landing step.

### What comes back

The sub-agent's final message, then a note on what landed (changes,
and conflicts if any). The caller reads the details with `vcs_log`
and `vcs_diff`.

### Cancelling

Cancelling the caller cancels its sub-agents (their cancel tokens are
children of the call's). A cancelled or failed sub-agent's commits are
abandoned and its workspace is forgotten, as a failed one's are today.
Nothing lands from a cancelled batch.

## Alternatives considered

- **A `fork` flag, off by default.** Codex's choice. The model would
  have to judge when a blank child is enough, and blank children are
  what made delegation weak.
- **Only messages, no tool calls or outputs**, as Codex forks. The
  context is smaller, but the child loses what the caller read, and
  the prompt cache misses. Compaction already handles a context that
  is too big.
- **Cut before the calling message.** Simpler, but the child cannot
  see its siblings and may redo their work.
- **Spawn, wait and message tools**, as Codex has. Children could
  outlive the call, which clashes with landing only between turns
  and makes the model's job harder. A blocking parallel batch covers
  the cases seen so far.
- **The sub-agent resolves its own conflict** by restacking itself
  onto the updated caller first. The caller stays clean, but the
  sub-agent resolves without knowing what its sibling meant.

## Consequences

- A delegate call is cheap to write and, on the caller's model, cheap
  to run: the task can be one line, and the inherited prefix hits the
  cache.
- Every sub-agent carries the caller's whole context, so four at once
  cost up to four times the caller's context. The cap bounds it.
- A caller whose batch conflicts spends a turn resolving it.
- `ToolCtx` carries the call's id and the pending assistant message,
  which a forking tool needs and other tools ignore.
- The tool list gains nothing: the model still has one tool to
  delegate with.
