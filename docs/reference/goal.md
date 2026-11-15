# heiko-goal

Keeps a conversation going until a goal holds. Each time the model
would stop, Jev (TypeSafe's System One model) is asked whether the
goal is met; if not, the model is sent back with the goal. It is a
Luau plugin heiko comes with (ADR 0050): `plugins/heiko-goal`, its
code in `src/plugin.luau` and `src/goal.luau`. Design:
[plugins.md](plugins.md).

## Setting a goal

A run's input sets it:

```text
/goal [--continuations N] [--budget USD] <condition>
/goal clear
```

- The condition is plain words, checkable from what the model runs:
  "cargo nextest run -p heiko-ai passes and clippy is clean". Its words
  are joined by single spaces, so line breaks and runs of spaces go.
- A condition written as one quoted string, `"..."`, is read exactly:
  `\"` is a quote, `\\` a backslash, `\n` a line feed and `\r` a
  carriage return. So `/goal "clear"` sets the goal `clear`, and
  `/goal "--budget 5 is spent"` a goal that starts with `--budget`.
  Quotes that do not wrap the whole condition are plain characters.
- `--continuations` (default 10) is how many times the goal may send
  the model back. `--budget` (default $2.00) caps what the goal costs:
  the run's turns and the checks, from when it was set. It is in US
  dollars, though the interface shows no amounts: what it should be set
  in is open (ADR 0057).
- The model does not see the command. Its input is `/goal <condition>`
  and a paragraph on how goals work (`goal.INSTRUCTIONS`), which
  `start` gives it in place of the message (`heiko.start{ input }`). The
  condition is written as is when it reads back as itself, and quoted
  when it does not (it is `clear`, starts with a limit, spans lines,
  has runs of white space, or is itself one quoted string), so every
  condition reads back exactly.
- A goal belongs to the conversation. Resuming the run keeps it, and a
  new `/goal` replaces it. `/goal clear` removes it.

## The check

When the model answers without tool calls, and the goal is active:

1. Jev gets one `noul` question, "is the goal met?", on a state of the
   goal, the model's last answer, and its last 6 tool results (the last
   1,500 characters of each output). The answer's own claims count only
   where a result backs them.
2. At 0.7 or more the goal is **met**, and the run stops.
3. Below it, the model is sent back, as a user message that starts with
   `heiko-goal: ` and names the goal and the continuation. Out of
   continuations, or past the budget, the goal **stops** instead, and
   so does the run.
4. When Jev gives no answer, the run stops unchecked, and the error is
   kept. The goal stays active for the next stop.

The plugin declares `continues = 20`, the most a run may be kept going
by it; the goal's own continuations are what limit it.

## State

The goal is in the run's model (`model.goal`), which a resumed run and
a fork inherit: `goal` (`condition`, `max_continuations`, `budget`,
`status`, `continuations`, `spent`, `checks`, `error`), the notes in the
transcript, and how the messages it knows draw. The banner's actions
are its `act` action (`pause`, `edit`, `clear`, and `keep_going` for
both resume and raise the limit), and the
`/goal` command its command: each changes the model. A check that waits
on Jev reads the model again once Jev answers: a Pause, an edit or a
Clear pressed meanwhile stands, and the answer, which was for a goal no
longer active, is dropped.

## In heiko-ui

- heiko-goal checks a run only with a TypeSafe key, and never a
  sub-agent; the state says whether the run did (`checks`).
- **The banner**, above the transcript while the conversation has a
  goal, under the plugin's mark (的): the condition and where it stands
  (`not met`, `paused`, `met`, `stopped · not met`), each check as a
  square (a cross for one not met, a tick for the one that met it, then
  the next check's number, or a filled square where it stopped), and
  the continuations as ticks with `N of M`. Its actions are words:
  pause, edit and clear on an active goal; resume on a paused one; new
  goal once met; raise the limit on one that stopped. What the goal
  spent is not shown.
- **The inspector's section:** the condition, the continuations as
  ticks, and every check, newest first: its square, what followed
  (sent back, the run stopped, stopped here), its turn and Jev's
  probability.
- **Notes:** a goal met or stopped is a note under the mark with its
  checks as squares; a stopped one offers raise the limit and clear
  while the goal still stands stopped. A `/goal` message draws as the
  command and its condition, with a stamp of its continuations.
- Without a key, `/goal`, resume and raise the limit say a goal needs
  Jev and do nothing, and the banner of a goal still to be met says it
  is not checked. On a run going on that started without a key, `/goal` keeps
  the goal for the next message, says so, and does not tell the model
  it is checked now.
- `/goal` in a chat going on sets the goal and tells the model; in a
  finished chat, or outside a chat, it sends `/goal` with its limits as
  the next message, which starts a chat outside one. Its popover's
  continuations and budget are its limits, unless the command types its
  own.
- A chat forks the main chat, state included, so it inherits the goal
  set there before its fork point, and shows it from the start.
- History shows what a live run showed: each check that met the goal,
  failed or stopped it, as a note; a check that sent the model back is
  its continuation message, which draws as nothing.
