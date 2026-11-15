# What needs the person

With several chats going, the sidebar and Home say which need the
person, and heiko notifies the desktop while its window is not focused. Code:
`heiko_ui_remote::attention`, `heiko-ui`'s `host/forecast.rs` and
`notify.rs`.

## A chat's state

`Attention::of(&Facts)` is the one place a chat's state is decided.
`Workspace::attention` gathers the facts: the run's status and turn
(`RunStatus::Interrupted` for a run heiko's closing cut off), what a
plugin holds it for (`points::ASKS`), the landing forecast
(`RunView::forecast`), how it ended for good (`RunView::ending`:
landed or dropped), its place in its main chat's landing queue
(`Workspace::queued`, from the parent's `RunView::landing_queue`), and,
for a main chat, the files a turn left in conflict on it
(`RunView::main_conflicts`, while marked; `dismissed` only hides the
notice in the chat). Each state has a mark (`Attention::marks`, one of
the kit's seven), a word for lists (`Attention::word`) and a line
(`Attention::line`):

| State             | Mark                 | Word        | Line                                                             | When                                      |
| ----------------- | -------------------- | ----------- | ---------------------------------------------------------------- | ----------------------------------------- |
| `Landed`          | (none)               | landed      | (none)                                                           | it landed on its parent and closed        |
| `Dropped`         | (none)               | dropped     | (none)                                                           | it was dropped and closed                 |
| `Asks`            | asks                 | asks you    | Asks you a question                                              | live, and a plugin waits on an answer     |
| `Working`         | working              | running     | Working · turn N                                                 | live                                      |
| `ConflictsOnMain` | failing, by N files  | conflicts   | Conflicts on main · N files                                      | a main chat a turn left conflicts on      |
| `Interrupted`     | (none)               | interrupted | Interrupted · heiko closed                                       | it was going when heiko closed            |
| `Failed`          | failing              | failed      | (none)                                                           | it stopped with an error                  |
| `Queued`          | asks, to confirm     | queued      | Queued · lands after main's turn, or Queued · needs confirmation | it waits in its main chat's landing queue |
| `WouldConflict`   | failing, by N files  | conflicts   | Would conflict in N files                                        | a stopped fork whose landing conflicts    |
| `ReadyToLand`     | met                  | to land     | Ready to land · N changes                                        | a stopped fork with changes to land       |
| `Idle`            | (none)               |             | (none)                                                           | anything else                             |

The first that holds, top to bottom, wins. A queued chat needs
confirmation when it would conflict in a file the person did not
confirm (`Waiting::needs_confirmation`); only then has it a mark.

- **The sidebar** is a rail of tiles (Home, each repository, each
  agent) and, beside it, the tree of the one chosen. A chat's row in
  the tree is its title and, at its right edge, its marks
  (`Marks::shown`), in one order: met, working, limit, failing, asks,
  each with a count above one. A row with replies not read yet is in
  bold. A row over others (a chat with its forks and sub-agents folded
  under it, an agent) shows their marks added to its own.
- **Order:** a state never moves a row. Main stays first; the rest
  keep newest first.
- **Ended chats stay listed:** a chat that landed or was dropped is
  closed, but its row stays, dim, with no marks and no close button. A
  chat closed by hand leaves the sidebar.
- **What needs the person** (`Attention::needs_you`): open chats that
  ask, would conflict, are ready to land, or wait in the queue for a
  confirmation, and a main chat with conflicts on it. A repository's
  or an agent's tile on the rail has a pip while it holds one, and
  Home's tile counts them all, with what plugins hold for the person.
- **Home** lists what happened across runs and agents
  (`Workspace::since`, at most 14): the runs at work, then those that
  ask, are ready to land or would conflict, failed, met a goal or
  stopped at a limit, or have replies not read; what agents left for
  the person; and each repository's changes the forge lacks. Each is
  when, its mark, whose it is, one line and one action (answer, read,
  push). The action of one that waits on the person stands out.
- **Phones** fold the same facts from the snapshot and the updates
  they get. A phone's row leads with the mark and has the line under
  its name; a working chat keeps a plugin's line (a goal's) when it
  has one.

## Landing forecasts

The host works out, in the background, what landing each finished
fork would do (`Host::forecast_landing`: `Vcs::land` with `confirm`
off, in the parent's workspace; it catches nothing up and makes no
workspace). It follows the workspace: each time a repository's runs
change shape (a run starts or stops, ends a turn, closes), its finished
open forks get new forecasts, so a forecast follows main's head.

- Asks that come within 250 ms make one pass; a pass a later ask
  overtook shows nothing.
- A fork or a parent still going keeps the forecast it had.
- `HostUpdate::Forecast { run, forecast }` sets `RunView::forecast`;
  `None` is nothing to land. A run that starts again drops it.

## Notifications

`heiko_ui::notify` notifies while heiko's window is not focused:

| Kind              | Title                       | Body                                            |
| ----------------- | --------------------------- | ----------------------------------------------- |
| `Asks`            | `<chat> asks you`           | the question                                    |
| `ReadyToLand`     | `<chat> is ready to land`   | `N changes on <parent> · <repo>`                |
| `ConflictsOnMain` | Conflicts are still on main | `heiko's turn left conflicts in N files · <repo>` |
| `Failed`          | `<chat> failed`             | `The run stopped with an error · <repo>`        |

- `Notifier` sends one notice each time a chat's state turns into one
  of these; a chat seen for the first time gets none, and a turn while
  the window is focused is kept quiet for good.
- `notify::conflicts_on_main(main, repo, files, cx)` says conflicts
  left on main. heiko-ui's `main` passes it to
  `Host::on_conflicts_on_main` before `attach`, so the host calls it
  each time a turn of main ends with conflicts still on it (ADR 0024).
- Notices go to the plugin that fills the `heiko.notify` slot (ADR 0045),
  looked up as each goes (`notify::Slotted`, given `Host::slots`), so
  the person's pick on the Plugins screen counts at once. heiko-notify,
  a Luau plugin, fills it: its `notify` shows each with
  `ctx.desktop.notice`, which heiko-luau-plugins-host sends to the desktop
  through `notify-rust` over zbus (no libdbus), and its setting
  switches them off. Opening one brings
  heiko's window up and opens its chat. With nothing in the slot, no
  notice shows.
