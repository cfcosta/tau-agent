# 0036: The plugins repository is in the sidebar

- Status: accepted. Amends [0027](0027-luau-plugins.md): its chats are
  no longer only on the Plugins page. Amended by
  [0037](0037-agents.md): Agents get a section of their own, between
  Repositories and Everywhere.
- Date: 2026-10-10

## Context

0027 kept tau's plugins repository out of the person's sight: the
sidebar, search and new runs passed it by, and its chats were listed
only on the Luau plugins page, under the Plugins screen's "Yours"
group. Since 0034 those chats land by themselves and go on, so they are
where the person keeps working on a plugin, and a chat at work there
had no row anywhere the person looks. The person's own plugins were
also one group among the built-in ones, though what they need (a
version to allow, tests to fix) is the person's to do and the built-in
ones need nothing.

## Decision

- **Plugins is a tree in the sidebar**, first under "Everywhere":
  - its row opens the plugins built in, and its chevron folds the tree;
  - **Built in** opens the Plugins screen, which lists every plugin but
    the person's own, with a link to theirs;
  - **Yours, in tau-plugins** opens the person's own plugins' page;
  - the plugins repository's chats follow, drawn as a repository's are
    (its main chat, the chats under it);
  - **Write a plugin** opens a new run's composer with `/plugin ` in it.
- Plugins counts the person's plugins that need them (a version waiting
  for Allow, failing tests, one that does not load), as a repository
  counts its chats that need the person.
- **The person's plugins have a page, and each of them one.** The page
  lists each plugin as a card, with where it stands, its tests, where
  it steps in, Allow when it waits, and the chats. A plugin's page has
  the same, every test, what it may reach, and its settings with the
  scope switch the Plugins screen had for it (0029).
- The repository stays out of everything else 0027 named: the sidebar's
  Repositories, search, History's chips, settings scopes and new runs.
- A plugin's page that asks for the composer, as Write a plugin does,
  opens a new run's when the screen has none.

## Alternatives considered

- **Pin it first among the repositories.** Least new interface, but it
  reads as one of the person's projects, and the plugins' standing,
  which is what needs them, would not show.
- **A section of its own, a plugin per row.** Puts the standing in the
  sidebar, but grows with every plugin; the count and the page carry it.
- **A card docked at the bottom, or an icon rail.** Always in view, but
  either takes room from the repositories or adds a second way to
  navigate.

## Consequences

- The plugins repository's chats show wherever chats do in the sidebar:
  live, waiting to land, unread.
- The Plugins screen no longer has a Yours group; the inspector sends a
  run's Luau plugins to their page instead of opening their settings in
  place.
- The fold of Plugins is the window's, not saved: it opens with tau.
