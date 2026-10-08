# 0026: Sub-agents run detached: `spawn` and `wait`

- Status: accepted, amended by
  [0031](0031-the-main-chat-orchestrates.md): `wait` goes, and main
  orchestrates. Amends [0015](0015-delegates-fork-their-caller.md):
  `delegate` gives way to `spawn` and `wait`, and a sub-agent outlives
  the call that started it. Amends
  [0024](0024-landings-queue-while-the-parent-works.md): sub-agents
  join the main chat's landing queue, and tau's turn after a drain
  reports what they did.
- Date: 2026-10-04

## Context

`delegate` runs its sub-agent inside the tool call. The main chat's
turn waits for it, so the person cannot talk to main while a sub-agent
works: what they write queues as a steer that lands after the batch.
Delegating is meant to mean "do this on the side, and bring the result
back", not "stop everything until it is done".

0015 turned down spawn and wait tools because a sub-agent that
outlives its call would land while its caller works, and landings then
happened only between turns. 0024 changed that: landings onto a busy
main chat wait in its queue, and tau starts main's turn when a landing
leaves conflicts. A detached sub-agent can use that queue.

## Decision

### Two tools: `spawn` and `wait`

- **`spawn { task, model?, effort? }`** starts a sub-agent and answers
  at once with its run id. The sub-agent is the same as before: it
  forks main (0015), works in a workspace of its own on main's newest
  commit, and commits its work. Main must have committed its own work
  first. At most four run at once per main chat. A fifth `spawn` is
  refused and told to `wait` for one.
- **`wait { runs? }`** blocks until the named sub-agents finish, or
  every running one if no runs are named. As each finishes, its work
  lands on main inside the call, as `delegate`'s did, and the result
  lists each answer with its landing note. A sub-agent that already
  landed, failed or was stopped says so. `wait` is `Grouped`, so none
  of the batch's other tools edit main's files while a landing moves
  them.
- The model calls `wait` when it cannot go on without the result.
  Otherwise it ends its turn, and the result comes back on its own.

### What nobody waited for comes back by itself

- A sub-agent that finishes, or stops at a limit, with nobody waiting
  joins main's landing queue (0024), with any conflicts already
  confirmed. Main asked for the work, so tau lands it whatever it
  conflicts in, as `delegate` did.
- When main is idle, the queue drains: consecutive sub-agents land one
  after another, and the drain stops after a landing that leaves
  conflicts. Then **tau starts main's turn with a report**: each
  sub-agent's title, its answer, and what landed, with a final line
  asking main to resolve conflicts when there are any. That turn is a
  resolving turn (0024) when there are conflicts.
- A sub-agent that fails has its changes dropped, as before. It also
  joins the queue, so main's next report says it failed and why.
- A sub-agent the person stops is dropped and not reported. A `wait`
  on it says it was stopped.
- Whoever takes a finished sub-agent first, `wait` or a drain, lands
  it. The other finds it gone. A drain runs only while main is idle,
  and `wait` only inside main's turn.

### Main stays free

- Stopping main's turn does not stop its sub-agents. Each one has its
  own chat with its own Stop.
- When the person stops main's turn, nothing lands on main right then,
  and tau starts no turn on it, so Stop means stop. What waits lands,
  and is reported, after main's next turn or when another sub-agent
  ends.
- A sub-agent's chat sits under main as before and closes when it
  lands or is dropped.
- A sub-agent never goes on with a message, as main does. What the
  person wrote to one that it ended before reading goes to main, whose
  work it is now (decided 2026-10-07): quoted in main's report when
  nobody waited, steered into main's turn when main waited for it, or,
  with main idle, as the text of tau's turn on main. A message for a
  sub-agent that ended but has not landed goes to main the same way,
  and the person is told it was sent there. A sub-agent the person
  stopped takes what it never read with it: Stop means stop.

### Restarts

- A sub-agent cut off by tau closing is dropped at the next start, as
  before.
- One that finished and waits in the queue is stored there. It lands,
  and is reported, when the next start drains main.

### Below main

Runs nest one level (0016). Every run declares `spawn` and `wait` so
the tools match main's and the prompt cache holds (0022). Below main,
both tools refuse.

## Alternatives considered

- **A `wait` flag on one `delegate` tool.** One tool, but a call that
  is sometimes blocking and sometimes not is harder to reason about
  than two tools with one job each.
- **Steer the report into main's running turn.** The answer would
  arrive sooner, but the landing would still have to wait for main to
  be idle. The model would then read about work that is not on its
  stack yet.
- **Report only on the person's next message.** No turn starts on its
  own, but a finished sub-agent would sit unseen, and its work
  unlanded, until the person spoke.
- **Keep `delegate` beside the new tools.** Two ways to block on a
  sub-agent, and more tools for the model to choose between.

## Consequences

- The person can keep talking to main while sub-agents work.
- Main's stack can move while a sub-agent works, so its landing can
  conflict where `delegate`'s could not. Main resolves the conflicts in
  the report turn.
- tau starts main's turns more often: one after each drain that landed
  or dropped a sub-agent.
- A sub-agent's usage no longer counts toward main's turn. It is its
  own run's.
- `tau-agent` gains a detached start for a forking sub-agent, with its
  own events and cancel token. The host follows it like any of its
  runs, and `SubAgentStops` goes.
- Queued sub-agents carry how they ended (a limit, or a failure) in
  the queue's records, so a report after a restart still says it.
