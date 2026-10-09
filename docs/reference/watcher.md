# tau-watcher

Notes what the person likely missed. While the main agent works, every
6th step a side request reads the whole transcript and answers
`learn: none`, or offers one short note. Crate:
`crates/plugins/tau-watcher`. Decision:
[0032](../decisions/0032-the-watcher-notes-what-you-missed.md).

## Switching it on

Off by default. The Plugins screen has it under Context; its settings
pane has:

- **Note what I missed**: the switch. Each check is a request.
- **Model**: the chat's own (default), or the newest model of a plan
  family (`sol`, `luna`, `astra`, `terra`). A family the plan lacks falls
  back on the chat's model.

It runs in a repository's main chat and in chats, never in sub-agents.
Settings take the scopes of ADR 0029.

## The check

`before_request` of each request, retries excepted. The step is the
count of assistant messages in the transcript, plus one, so it goes on
across the messages of a chat.

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

The request goes through `PluginCtx::ask`: outside the run's
conversation, charged to the run. It carries the instructions
(`tau_watcher::prompt::INSTRUCTIONS`), then the conversation (tool
results cut to 2,000 characters), the last 50 note lines, and the lines
marked "knew this". Any failure of the request is recorded as `dropped`.

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

`explain:` may be left out. `tau_watcher::reply::parse` is strict: a
quoted or fenced reply, a missing or unknown tag, a long line, a line
with no period or an explanation of any other shape is an `Unreadable`.
The plugin records a `dropped` outcome and shows nothing.

## Records

| `kind`       | Fields                           | Effect                                    |
| ------------ | -------------------------------- | ----------------------------------------- |
| `noted`      | `step`, `tag`, `line`, `explain` | a note, anchored under the step           |
| `dropped`    | `step`, `reason`                 | an unusable reply; nothing is drawn       |
| `typed_past` |                                  | a message was written past a new note     |
| `answered`   | `key`, `answer`                  | `learned`, `knew`, `chatted`, `dismissed` |

`learn: none` makes no record. The UI stores `answered` itself
(`Request::Record`), so a stored run draws what the live one did.

## What the person sees

- **Annotation** under the step that asked: amber left edge,
  `Heads up · <line>` or `You should know · <line>`, and the buttons
  **Learn more** (the stored explanation inline, no second request),
  **Knew this already**, **Chat about it** and **Dismiss**.
- **Band** (`points::COMPOSER_BAND`) between the transcript and the
  composer, same line and actions, while the note is new. Any answer, or
  the second message written past it, takes it away.
- **Chat about it** puts `Here is a note offered by a side agent:` and
  the quoted line in the composer. Control characters go, and `@` and
  words starting `ultra` get a zero-width space so the composer does not
  read them as mentions or keywords (`tau_watcher::cadence::chat_text`).

## Items

| Item                       | What it is                                  |
| -------------------------- | ------------------------------------------- |
| `WatcherPlugin`            | The agent `Plugin`                          |
| `WatcherUi`, `WatcherHost` | The `UiPlugin` and `HostHalf` tau registers |
| `State`                    | Notes and counts folded from the records    |
| `reply::parse`             | Reads the model's reply                     |
| `cadence::due`, `skips`    | When to check; the back-off                 |
| `cadence::recent`          | The last 50 lines the prompt carries        |
| `ui::settings::Settings`   | `enabled`, `family`                         |
