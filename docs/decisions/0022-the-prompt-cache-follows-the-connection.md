# 0022: The prompt cache follows the connection

- Status: accepted. Amends [0015](0015-delegates-fork-their-caller.md):
  its claim that "the inherited prefix hits the cache" held only by
  chance. Amends [0016](0016-runs-nest-one-level.md): every run declares
  `delegate`, and below the main chat it refuses.
- Date: 2026-10-03

## Context

On tau's route (`wss://api.openai.com/v1/responses` with a ChatGPT
token), measured on 2026-10-03 with gpt-5.5
(`crates/tau-ai/examples/cache_probe.rs`; the table is in
[openai-websocket.md](../reference/openai-websocket.md), "Prompt
cache"):

- A connection that already served a prefix reads it again from cache
  at 85–86%, whatever `prompt_cache_key` says, as a delta or a full
  resend.
- A new connection reads it about 40% of the time, at random, whatever
  `prompt_cache_key` says.
- One tool fewer, or another reasoning effort, reads nothing, even on
  the same connection. Text added at the end of the instructions still
  reads the prefix before it.

tau left all of this to chance. Its pool gave a new run any idle
connection and closed idle ones after five minutes, so a message to a
finished chat, which starts a new run, mostly resent the whole
conversation uncached; a 26-turn run lost a third of its saving to one
such resend. tau never set `prompt_cache_key`. Main had `delegate`,
chats had `vcs_land`, sub-agents had neither and no `ask`, so a chat's
or a sub-agent's first request could never read main's cache, and
tau-reasoning could change the effort of a long conversation at any
message.

## Decision

A conversation is a **path** of work: the main chat, each chat, each
sub-agent. Each path has its own `prompt_cache_key` and, as far as the
pool can give it, its own connection. A fork's first request reads its
parent's prefix on the parent's connection; after that the connection
is the fork's.

- **Per-path keys.** Each run's settings carry a `Lineage`: its path
  (the id of the run whose conversation it is) sent as
  `prompt_cache_key`, and, for a fork or a forking sub-agent that is
  not resumed and whose parent last ran on the same model, the parent's
  path. The key is fixed for the run, so the delta rule holds.
- **Connection affinity.** The pool places a lane on an idle
  connection of its own path; else, for a fork's first request, on its
  parent's, taking it from the parent's lane if it holds it (a
  handoff: the parent's next request takes another connection); else
  it waits up to 5 seconds for a busy one of those; else it takes a
  free connection that served its path before, one that serves none,
  or the least recently used; else it opens one. A connection keeps the
  continuation its last lane left, so a resumed run continues by delta.
  Free connections that serve a path stay open until the 55-minute
  rotation, at most 8 of them. One response is in flight per
  connection, as before.
- **Tool parity.** Main, chats and sub-agents of a repository declare
  the same tools in the same order, and the same instructions. Where a
  tool does not apply it refuses, saying why: `delegate` below main
  ("Only the main chat delegates; …", so 0016 holds), `vcs_land` on
  main and on sub-agents, `ask` on sub-agents. What differs by kind goes
  in the first message's context, never in the instructions.
- **Sticky effort.** tau-reasoning keeps the effort a conversation last
  ran at when a run goes on with or inherits a prefix of 20k tokens or
  more, even when Jev is sure of another, unless the person chose one.
  Its choice records the tokens that kept.

## Consequences

- A resumed chat within the hour reads its conversation from cache
  instead of resending it; a chat's or a delegate's first request reads
  main's prefix.
- The parent loses its connection to the fork it hands off to. Its next
  request goes elsewhere, in full; once the fork's run ends, its next
  run takes back the connection it served before.
- tau may keep up to 8 idle sockets open for up to 55 minutes.
- A model sees `delegate`, `vcs_land` and `ask` where they only refuse,
  at the cost of a wasted call when it tries one.
- An effort Jev would have changed on a long conversation stays; the
  person can still choose one.
- Parallel forks get one cached first request between them; the others
  open new connections.
