# 0039: Chats wake themselves later: `tau-schedule`

- Status: accepted. Builds on [0030](0030-host-halves-are-crates.md)'s host
  halves and the conversations service of
  [0037](0037-agents.md), which it extends from an agent's main chat
  to any chat. Agents keep their own `schedule`, which also repeats
  (0037, "It schedules itself").
- Date: 2026-10-12

## Context

A chat's work sometimes waits on the world: a pull request under
review, a release that ships tomorrow, a build that takes an hour. The
model can only say "check back later", and later the person has to
remember to ask. An agent (0037) can wake itself, but making an agent
for one follow-up is a repository, a conversation and a cap too many.

What a chat needs is smaller: wake me once, at a time, with a note to
myself. Doing that again and again is what agents are for; a chat that
repeated would be an agent without the cap, the page or the pause that
make one safe to leave running.

## Decision

- **A built-in plugin, `tau-schedule`**, with a host half (ADR 0030)
  and a UI half, listed under Plugins like the others.
- **Its tools, in every run** but an agent's, whose own `schedule`
  takes their place. A sub-agent has them too, so main, its chats and
  its sub-agents keep one prompt prefix, but its `schedule` refuses: it
  ends and lands, so nothing would wake it, and it says in its answer
  what the chat that started it should schedule.
  - `schedule { name, note, in | on }` wakes the chat once, `in` a
    duration (`"2h"`) or `on` a local date and time
    (`"2026-10-13 09:00"`). A minute ahead is the soonest, a year the
    furthest, and a chat may have 100 waiting. `every` and `at` are
    refused with a word on why: repeating is for agents. Scheduling a
    name again replaces it.
  - `scheduled {}` lists what the chat has waiting, with the host's
    local time now; `unschedule { name }` cancels one.
- **Waking.** When one comes due, the chat takes a turn of tau's
  (`Conversations::wake`, new here), on the model it last ran on, whose
  message is the note:

  ```text
  [scheduled · recheck · 2026-10-13 09:30]
  Did #412 merge? If not, look at what its review asks.
  ```

  The interface draws it as a card. A chat with a turn going is not
  interrupted: the wake waits for that turn to end. A chat that landed
  or was dropped takes no more messages, and what it had scheduled
  goes. A turn that fails to start is tried again half a minute later.
- **Kept on the host**, in the plugin's directory, one file for all
  chats, written whole on each change. tau closed when one came due
  wakes the chat once it starts again; there is no daemon (0037).
- **Shown in the chat's inspector**, under Scheduled: each with its
  name, when, and note, and a way for the person to cancel it.

## Alternatives considered

- **Repeating schedules in any chat.** The person asked for repeats to
  stay with agents: a chat has no daily cap, no pause and no page that
  says what it is doing unattended.
- **Steering a chat whose turn is going.** `exec_command`'s `notify`
  does this (0038), and it suits a command the turn started. A wake-up
  from hours ago belongs to no turn going; it waits for its own.
- **Inside tau-agents.** Less code, but every chat would depend on the
  agents plugin being on.

## Consequences

- `Conversations` gains `wake(run, text)`, answering `Started`, `Busy`
  or `Gone`; hosts that do not wake chats refuse.
- The clock, durations and local dates tau-agents kept move to
  `tau-schedule`, which both use.
- A chat can still keep itself going by scheduling again each time it
  wakes. Each wake is a turn the person sees in the chat, and cancels
  from its inspector.
