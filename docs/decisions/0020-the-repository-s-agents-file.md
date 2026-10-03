# 0020: A run reads the repository's AGENTS.md

- Status: accepted. Amends [0001](0001-library-not-product.md): the
  library still discovers no context files, but tau, the app, reads
  `AGENTS.md`.
- Date: 2026-10-03

## Context

0001 left settings files and `AGENTS.md` discovery out of scope, since
tau-agent is a library for workflows written in code. tau, the desktop
app, is a product on top of it: it works in people's repositories, and
those repositories keep their rules for agents in `AGENTS.md`. Without
it, the model learns them only from the person, again in every chat.

## Decision

- tau-ui's host adds `AGENTS.md`, at the root of the workspace a run
  works in, to the run's instructions as the run starts. A small host
  plugin does it in `Plugin::start`, after the run's workspace exists.
- It is read at each start of a run, not each turn: instructions are
  fixed for a run's session, because the WebSocket sends only new input
  while the rest of the request stays the same
  ([architecture](../architecture.md#why-the-websocket-shapes-the-design)).
- Each run reads its own checkout, so a chat that edits the file sees
  its version the next time it starts, and the main chat reads trunk's.
  Sub-agents read their own workspace's copy.
- The text goes under a heading that says these are the repository's
  instructions. Over 32 KiB it is cut, on a character boundary, and the
  instructions say it was cut. A missing or unreadable file adds
  nothing.
- tau-agent itself still discovers no files.

## Consequences

- Repositories steer tau as they steer other agents, with no setting.
- A change to `AGENTS.md` reaches a run going on only at its next start.
- The file costs prompt tokens on every request of a run; the cap
  bounds it.
