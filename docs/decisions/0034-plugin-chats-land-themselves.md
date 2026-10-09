# 0034: Plugin chats land themselves

- Status: accepted. Amends [0033](0033-plugins-are-live.md): it chose
  not to land plugin chats by themselves; this reverses that.
- Date: 2026-10-09

## Context

With 0033, a plugin works in the chat that writes it as soon as its
tests pass, and everywhere else once it lands on the plugins
repository's trunk. In practice nothing landed: two chats wrote
`checkpoints` and `decision-log`, used them, and ended, and both stayed
on their own bookmarks. Other chats, in that repository and in others,
never saw them, and the model in them could only say the plugin was not
there. Nothing told the person a landing was the missing step, and a
landing closed the chat, so they would have had to stop refining the
plugin to share it.

## Decision

- **A plugins chat lands when its turn ends.** When a chat's turn ends
  as it meant to (not stopped, not failed), the host asks each plugin's
  host half whether the chat may land by itself (`HostHalf::lands_itself`,
  false by default). tau-luau-plugins says yes for its own repository
  when every plugin folder the chat's workspace changes from trunk
  loads, passes its tests and reaches no further than the person
  allowed. A folder the same as trunk's does not count, even when
  trunk's fails.
- **Only clean landings.** It lands when the chat has something to land,
  the landing would leave no conflicts, and neither the chat nor main is
  going. Otherwise it waits: the chat's next turn end tries again.
- **The chat goes on.** A landing like this keeps the chat's workspace
  and bookmark ([`Host::land_and_go_on`]): its commits are rebased onto
  main's head, keeping their change ids, so its working copy follows
  and its next turn works on top of main. A later landing brings only
  what it did since. Its links in main do not name it and its record is
  marked `kept`, so it is not taken for a chat that landed and closed.
  The landing's card shows in main and in the chat.
- **Recovery keeps it open.** The intent stored before a landing says
  the chat goes on, so a landing tau closed in the middle of is finished
  at start without closing the chat, and only once.

## Alternatives considered

- **Ask the person to land.** Keeps a review step, but the review that
  matters is already there: a version that reaches further waits for
  the person's Allow on the Plugins screen (0027), wherever it lands.
- **Run any open plugins chat's versions everywhere, unlanded.** Two
  chats changing the same plugin would fight over which version runs.
- **Land and close the chat, as a landing always did.** Changing the
  plugin again would need a new chat, without the one that wrote it.

## Consequences

- The plugins repository's trunk moves after each passing turn; every
  run picks the new versions up before its next request (0033).
- A plugins chat that edits things besides plugins (its README, say)
  lands those too, as long as its plugins pass.
- A chat whose landing waits (its tests fail, or main is going) says
  nothing about it; its next passing turn lands it.
