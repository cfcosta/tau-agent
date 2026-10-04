# 0024: Landings queue while the parent works

- Status: accepted. Amends
  [0009](0009-child-runs-land-on-their-parent.md): its "idle runs
  only" rule no longer refuses a landing, it delays it. Amends
  [0014](0014-the-model-commits-and-runs-land-as-stacked-diffs.md): the
  parent's resolving turn is checked, and conflicts it leaves hold the
  parent. Amended by [0026](0026-sub-agents-run-detached.md):
  sub-agents nobody waits for join the queue, and tau's turn after a
  drain reports them.
- Date: 2026-10-03

## Context

0009 lets the host rewrite only idle runs, so Land on a chat refused
while its main chat ran. The main chat is busy often: the person writes
to it, and tau starts its turn to resolve each conflicting landing
(0014). Someone with several finished chats had to watch main and click
Land in the gap between turns, one chat at a time.

0014 also trusted the resolving turn: once it ended, main counted as
resolved. A turn that gave up, or committed only part of a resolution,
left conflicts on trunk. New chats forked that conflicted code, and the
next landing restacked onto it.

## Decision

### Land queues

- Land on a finished chat puts it in its main chat's **landing queue**,
  one per main chat, first in, first out. When main is idle and nothing
  waits before it, it lands at once, as before.
- Only finished chats can be queued. A chat that is still running is
  landed once it finishes.
- The queue is stored as records on the main chat, so it survives tau
  closing. Each landing out of it is the crash-safe landing of 0009.
- When main becomes idle (its turn ended, and no resolving turn is about
  to start), the host lands the queue in order, one chat at a time. Each
  is previewed again at that moment:
  - clean: it lands;
  - conflicts the person confirmed when they clicked Land (the same
    files, or fewer): it lands, tau starts main's resolving turn, and
    the queue waits for that turn to end;
  - new conflicts: it stays first in the queue, marked as needing
    confirmation, and the queue stops. Land again confirms what it
    shows now.
- Unqueue takes a chat out. A chat that landed or was dropped meanwhile
  leaves the queue quietly; one that cannot land leaves it and says why.
- The queue's decisions are one pure state machine (`Lane`); the host
  carries out what it returns.

### The resolving turn is checked

- tau's resolving turn is held once, as tau-vcs holds a stop with
  uncommitted changes, while main's stack still has conflicts:
  "Conflicts remain in a.rs, b.rs. Resolve them and commit."
- A main turn that ends with conflicts on main's stack marks main
  **conflicted**, a stored mark that names the files. The person sees
  "Conflicts are still on main", with Resolve again (tau's turn, same
  message) and "I'll write to main" (the card goes, the mark stays), and
  a notification.
- While main is conflicted, its queue does not drain, and new chats are
  refused: "main has conflicts in a.rs; resolve them first". They would
  fork conflicted code.
- The mark goes once main's stack is clean, checked at each turn end of
  main and each time the queue drains, after catching main up.

## Consequences

- Land never fails because main is working; the person can land
  several chats in a row and leave.
- Landing, previewing and dropping run off the interface's thread.
- A preview while main works reads main as it is, without catching it
  up: what lands later is previewed again.
- A chat can wait a long time behind a conflicted main or a chat that
  needs confirmation; both say so on main's card and on the chat's own.
- The host keeps a little state per main chat in memory (busy, a
  resolving turn about to start); everything else is stored.
