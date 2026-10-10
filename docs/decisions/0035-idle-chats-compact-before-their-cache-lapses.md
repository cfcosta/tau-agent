# 0035: Idle chats compact before their cache lapses

- Status: accepted. Builds on [0022](0022-the-prompt-cache-follows-the-connection.md).
- Date: 2026-10-10

## Context

A long chat costs most when the person comes back to it after a
while. By then its prompt cache has lapsed: on OpenAI's route the cache
lives on the connection, which tau rotates 55 minutes after it opened
(0022), and the next message resends the whole chat uncached. Claude
Code compacts such a session while it is idle, shortly before its
1-hour cache lapses ("Compacted while idle, before the prompt cache
expired"): the summary reads the session from cache, and the next
message starts small.

tau's compaction could not do that. Its summary request has its own
instructions and sends the history as text, so it reads nothing from
the chat's cache; and compacting meant a run, which needs a message.

## Decision

- **A long idle chat is compacted shortly before its cache lapses.**
  The host watches each chat and main chat from the end of its turn
  (`host/idle.rs`). When the pool says when its connection's cache
  lapses (`OpenAi::cache_lapse`: 0.9 of the way from its last request
  to the first of rotation and the measured cache lifetime), and its
  last request held half its model's window or more, the host compacts
  it then, unless it went on, ended or landed first. A message for it
  stops a compaction under way, and it goes on as it was.
- **The summary is asked in the chat's own conversation.** A
  compaction for an idle run (`Resumed::compact_idle`) opens a session
  with the settings of the run's last request (`Outcome::request`), on
  its connection. Plugins ask the model there through
  `ContextView::conversation`; tau-compaction sends the messages it
  summarizes as they are, then its prompt, so the request reads them
  from cache.
- **Plugins opt in.** Only plugins whose `Plugin::start_idle` returns a
  run take part: tau-compaction (switched by "Compact idle chats") and
  tau-memory, which keeps what the summary drops. No `start` runs, so
  nothing changes the settings the cache depends on, and nothing a
  start does for a message (an effort picked, a memory search) happens.
- **It says so.** The rewrite is stored and sent with its trigger,
  `idle`; the chat shows "Compacted while idle, before the prompt cache
  expired" with its tokens before and after.

## Alternatives considered

- **Keep the cache warm with a request before it lapses.** Rotation
  ends the connection, and its cache, at 55 minutes whatever is sent;
  a warm-up would only move the cost.
- **Compact at the next message, as now.** That pays the uncached
  resend this avoids, and then the summary on top.
- **Resume the run with every plugin, as a message would.** Plugins
  start for a message: tau-reasoning asks a model to pick the effort,
  which can change it and miss the cache; memory searches; others
  record. A compaction is not a message.
- **Claude Code's 200,000-token floor.** tau's models have 272,000-token
  windows and compact at 255,616: half the window leaves room between
  the two.

## Consequences

- A chat compacted while idle loses detail to the summary even if the
  person never comes back; it costs the summary's request, mostly read
  from cache.
- Only chats that ran in this session are watched: the pool's
  connections, and so the caches, do not outlive tau.
- A compaction's events reach the interface without `RunStart` or
  `RunEnd`; interfaces fold `ContextRewritten` as they do between
  turns.
