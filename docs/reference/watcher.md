# tau-watcher

Notes what the person likely missed. While the main agent works, every
6th step a side request reads the whole transcript and answers
`learn: none`, or offers one short note. It is a Luau plugin tau comes
with (ADR 0050): `plugins/tau-watcher/`, `src/plugin.luau` and
`src/watch.luau`. Decision:
[0032](../decisions/0032-the-watcher-notes-what-you-missed.md).

## Switching it on

Off by default. The Plugins screen has it under Context; its settings
pane has:

- **Note what I missed**: the switch. Each check is a request.
- **Model**: `chat`, the chat's own (default), or a family from
  `ctx.models`, the plan's: its newest model reads the run. A family the
  plan lacks falls back on the chat's model.

It runs in a repository's main chat and in chats, never in sub-agents.
Settings take the scopes of ADR 0029.

## The check

`before_request` of each request, retries excepted. The step is
`replies`, the count of the agent's messages in the transcript, plus
one, so it goes on across the messages of a chat.

A check is made when all hold:

1. The step is a multiple of 6.
2. The run has made no note yet.
3. The last note is not waiting: new, and written past fewer than 2
   times.
4. The back-off allows it.

The back-off counts messages written past an unanswered note since the
person last answered one (`n`). For `n` of 0 to 2 no due check is
skipped; from 3, `2^(n-3)` are, at most 16. A run that starts while the
last note is unanswered records the message as written past it.

The request is `ctx.tools.infer`, with the family the person picked as
its `model`: outside the run's conversation, charged to the run. Its
task carries the instructions (`watch.INSTRUCTIONS`), then the
conversation, read with `ctx.transcript()` (tool results cut to 2,000
characters), the last 50 note lines, and the lines marked "knew this".
Any failure of the request is counted as `dropped`.

## The reply

```text
learn: none
```

or

```text
learn: <one line, at most 240 characters, ending with a period>
tag: You should know | Heads up
explain:
**<title, 3 to 7 words>**
- <3 to 5 bullets>
```

`explain:` may be left out. `watch.parse` is strict: a quoted or fenced
reply, a missing or unknown tag, a long line, a line with no period or
an explanation of any other shape is refused, with why, which the
plugin's log keeps. The plugin counts it `dropped` and shows nothing.

## State

The run's state (`ctx.state`) keeps it, as `watch.state` reads it:

| Field        | What it is                                                                        |
| ------------ | --------------------------------------------------------------------------------- |
| `notes`      | each `{ key, step, tag, line, explain, status, typed_past }`, `key` `n0`, `n1`, … |
| `ignored`    | messages written past an unanswered note since the person last answered one       |
| `dropped`    | replies that could not be used                                                    |
| `last_check` | the step of the last check that kept a note or a drop                             |
| `checked`    | `{ run, step }`: this run's own last check, whatever it found                     |
| `noted_in`   | the run that made the last note: a run notes once                                 |

`status` is `new`, `learned`, `knew`, `chatted` or `dismissed`; the
note's buttons set it (`actions.answer`), in the state stored with the
run, so a stored run draws what the live one did. `learn: none` keeps
nothing but `checked`.

## What the person sees

- **Annotation** under the step that asked, a keyed note of the view
  (`notes.n0`), drawn again in place as it changes: the tag in amber
  and the line, and the buttons
  **Learn more** (the stored explanation inline, no second request),
  **Knew this already**, **Chat about it** and **Dismiss**.
- **Band** (`points::COMPOSER_BAND`) between the transcript and the
  composer, same line and actions, while the note is new. Any answer, or
  the second message written past it, takes it away.
- **Chat about it** puts `Here is a note offered by a side agent:` and
  the quoted line in the composer. Control characters go, and `@` and
  words starting `ultra` get a zero-width space so the composer does not
  read them as mentions or keywords (`watch.chat_text`); the action
  answers `tau.interface("composer", { text })`.

## Items

| Item                              | What it is                                                |
| --------------------------------- | --------------------------------------------------------- |
| `src/plugin.luau`                 | The plugin: `start`, `before_request`, the action, `view` |
| `watch.parse`, `watch.write`      | Reads the model's reply; writes one as it reads           |
| `watch.due`, `watch.skips`        | When to check; the back-off                               |
| `watch.recent`                    | The last 50 lines the prompt carries                      |
| `watch.conversation`              | The conversation as the side request reads it             |
| `watch.noted`, `answered`, `band` | The notes in the state, and the one above the composer    |
| `tau_watcher::FILES`, `DEMO_*`    | The files, and the views the demo shows                   |

`tests/luau.rs` loads it as tau does and runs its Luau tests, cases,
properties and a state machine, and checks the demo's views are what
the plugin draws (`BLESS_FIXTURES=1` writes them anew).
