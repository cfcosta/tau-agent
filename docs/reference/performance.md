# Performance

What heiko measures of itself, how to read it, and what it measured
last. Where code may block is ADR 0028; this is how we see whether it
does.

## What is measured

`heiko-perf` keeps a table for the whole process: a group (a kind of
work), a name (whose), and for each how often, how long in all, the
longest, and how many fell under 1, 4, 16, 50, 100, 500 and 1 000 ms.
Recording is always on.

| Group | Name | What it is |
| --- | --- | --- |
| `apply` | the update's kind | The interface's thread applying one update from the host (`Workspace::apply`) |
| `render` | `workspace` | Building the window's tree for a frame; layout and paint are not in it |
| `thread` | `beat lag` | How late the interface's thread ran a timer it asked for every 8 ms: everything that kept it busy, layout and paint included |
| `thread` | `host lag` | The same for a task on the host's runtime, every 10 ms: it grows when the runtime's threads are all busy |
| `hook` | plugin and hook | One call of a Luau plugin's hook, start to end, with what it awaited |
| `luau` | plugin | One step of a plugin's VM: the time a runtime thread was the VM's alone |

A **stall** is a beat more than 32 ms late (two frames at 60 Hz). A
thread of its own watches the beats, so it sees a stall while it
lasts, and keeps what the interface's thread said it was doing (the
update it was applying). The report keeps how many, how long in all,
and the twenty longest.

## Measuring

`HEIKO_PERF=<file> heiko` starts the beat and the watching thread, and
writes the report to the file as JSON, and beside it as a `.txt` table,
every two seconds.

`scripts/perf.sh [rounds] [seconds]` measures both sides under load and
prints the tables:

- **The interface.** `heiko --demo --stress <rounds>` plays a long run
  ten times as fast as the demo's script: each round a turn that
  thinks, answers with code in it, reads, edits and runs a command, then
  one answer of some 3 600 words; every three seconds it leaves the run
  for Home or comes back. It runs in a headless sway of its own, as
  the screenshots do.
- **The host.** A test of heiko-ui, ignored unless asked for, runs sixty
  turns of three tool calls on a real host with every built-in plugin
  and a real repository, the model answering at once, so the time left
  is heiko's own.

## Measured on 2026-11-15

One machine, release build, headless sway. Read them as orders of
magnitude.

**The interface, 75 s of the stress run** (60 rounds and the long
answer, 2 381 text deltas, 24 route changes): one stall, 72 ms, as the
window opened. After it no beat was 16 ms late; the mean lag was
0.11 ms. Applying an update took under 0.03 ms whatever its kind, and
building a frame's tree 0.46 ms on average, 0.9 ms at most.

**The host, 60 turns of three tool calls**: 29 ms a turn in all. Luau
hooks were about 1 000 ms of the run's 1 770 ms, yet the VMs ran for
only some 370 ms of it: the rest is what hooks await, mostly the store.
No hook took 16 ms but heiko-skills' `start`, once, at 38 ms (it reads
the skills' folders). The costliest, each time they ran: heiko-memory's
`turn_end` (2.8 ms), heiko-fast-compaction's `rewrite_context` (1.4 ms),
heiko-memory's `after_tool_result` (1.0 ms), heiko-constitution's
`before_tool` (0.7 ms). The runtime never woke a task more than 2 ms
late.

What this does not cover: a plugin of the person's own that is slow, a
history of thousands of items opened cold, large diffs and terminal
output drawn open, a phone attached, several runs at once.
