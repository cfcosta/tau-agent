# 0031: The main chat orchestrates; `wait` goes

- Status: accepted. Amends
  [0026](0026-sub-agents-run-detached.md): `wait` goes, and nothing
  blocks on a sub-agent. Up to 8 sub-agents run at once, not 4.
- Date: 2026-10-08

## Context

0026 gave main `spawn` and `wait`, and said to call `wait` only when it
could not go on without a sub-agent. Models called it anyway. A turn
that spawned work had nothing left to do, so the model waited instead
of ending the turn. Main then sat blocked while its sub-agents worked,
which is what 0026 set out to stop.

Nothing told main that delegating was its job. It had the same
instructions as every other run. The report turn after a drain told it
to start more work "only when what the person asked for still clearly
needs it", so each round of results tended to end the work.

The pieces for main to work as an orchestrator were already there.
Sub-agents land through main's queue as they end (0024, 0026), a
landing that conflicts starts tau's turn on main to resolve it, and each
drain reports to main.

## Decision

- **No `wait`.** Main spawns and ends its turn. Every sub-agent lands
  through main's queue, and tau's turn after the drain reports it. The
  tool, its refusing twin below main, its card and its landing path in
  `tau-vcs-host` go.
- **Main leads.** Each run of a main chat starts with context, ahead of
  the person's message, that says it orchestrates. It splits the
  request into tasks that can go on side by side, spawns a sub-agent for
  each, and ends its turn while they work. As each one lands, it checks
  the work, resolves any conflicts and commits, then spawns what comes
  next until the request is done. It does only the glue between tasks,
  and work too small to hand off, itself. The guidance goes in the first
  message, not the instructions, which every run of the repository
  shares (0022).
- **Reports keep the work going.** The report turn tells main to check
  what landed, resolve conflicts, spawn the next tasks, and tell the
  person in a line or two what landed, what failed and what it started.
  It ends the work only when nothing is left.
- **Sub-agents are main's crew, not the person's chats.** Forks are
  kept; sub-agents are disposable. The sidebar lists main's sub-agents
  only while they work, with the sub-agent mark, above its forks and
  outside the limit on forks shown; one that ends leaves the list, and
  folded, main's row counts those at work. Main's chat shows them in a
  tray above its composer: each one working, with its turn, latest
  call and cost, and those spawned since the person's last message,
  faded, with how they ended. History keeps them all.
- **8 at once.** `MAX_RUNNING` goes from 4 to 8. A ninth `spawn` is
  refused and told to end the turn and spawn again when tau reports
  one.

## Alternatives considered

- **Keep `wait`, word it more strongly.** The 0026 wording already said
  "only when you cannot go on". A tool that is there gets called.
- **Main never edits files.** Every change, however small, would cost a
  sub-agent's fork of the whole conversation. Glue between tasks and
  conflict resolution stay with main.
- **Put the guidance in the instructions.** It would reach chats and
  sub-agents too, and 0022 keeps what differs by kind out of the
  instructions.

## Consequences

- Main's turns are short: plan, spawn, end. Most of its work happens in
  report turns.
- A task that needs another's result waits for the report turn that
  brings it. Main spawns it from there.
- More sub-agents run at once, so more jj workspaces exist and more
  builds run at the same time.
- Chats start from main's conversation, so they read main's
  orchestrator context too. Their `spawn` refuses and says why.
