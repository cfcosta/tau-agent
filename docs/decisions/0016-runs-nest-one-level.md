# 0016: Runs nest one level

- Status: accepted. Amends [0009](0009-child-runs-land-on-their-parent.md)
  and [0015](0015-delegates-fork-their-caller.md): only a top-level run
  forks or delegates. Amended by
  [0022](0022-the-prompt-cache-follows-the-connection.md): every run
  declares `delegate`, and below the main chat it refuses.
- Date: 2026-09-30

## Context

Every repository has a main chat, and every chat the user starts forks
it. A chat could then be forked again, and could delegate to
sub-agents, each of which is a run under it. The tree grew as deep as
the user and the model took it: main, a chat, its fork, that fork's
sub-agent. Each level lands on the one above it, one review at a
time, and the sidebar indents each one further.

The user wants one shape: the main chat, and the chats under it.

## Decision

A repository's main chat is the only **top-level** run. Only it has
runs under it. A run with no parent from before main chats is not
top-level: it cannot be forked, does not delegate, and does not merge
into trunk.

- **Forking.** Only a top-level run can be forked. The host refuses to
  fork any other run, and the interface does not offer it: no "Fork
  here", no "Fork from turn", no `/fork`.
- **Delegating.** Only a top-level run gets the `delegate` tool. Its
  sub-agents sit directly under it, as its chats do.
- A chat under the main chat still lands on it, and so moves trunk
  ([0014](0014-the-model-commits-and-runs-land-as-stacked-diffs.md)).

## Consequences

- The tree is two levels deep: main, then its chats and sub-agents. A
  landing is never waiting on a landing below it.
- Work a chat would have delegated is done in the chat itself, or
  started as another chat of the main chat.
- The main chat's forks are the user's chats, so "try another way"
  from inside a chat starts from the main chat instead.
- Nothing merges into trunk any more: only a run from before main
  chats did, and a chat lands on the main chat, which commits on trunk.
  The host's merge (`Host::merge`, its preview, and the turn it
  started to resolve a merge's conflicts) and
  `Project::fast_forward_trunk` are gone. A chat cannot
  have runs under it, so landing or dropping one never waits on its
  children.
