# What needs the person

With several chats going, the sidebar says which need the person, and
tau notifies the desktop while its window is not focused. Code:
`tau_ui_remote::attention`, `tau-ui`'s `host/forecast.rs` and
`notify.rs`.

## A chat's state

`Attention::of(&Facts)` is the one place a chat's state is decided.
`Workspace::attention` gathers the facts: the run's status and turn
(`RunStatus::Interrupted` for a run tau's closing cut off), what a
plugin holds it for (`points::ASKS`), the landing forecast
(`RunView::forecast`), how it ended for good (`RunView::ending`:
landed or dropped), its place in its main chat's landing queue
(`Workspace::queued`, from the parent's `RunView::landing_queue`), and,
for a main chat, the files a turn left in conflict on it
(`RunView::main_conflicts`, while marked; `dismissed` only hides the
card). Each state has its icon and a line under the title:

| State           | Line                        | Color  | When                                   |
| --------------- | --------------------------- | ------ | -------------------------------------- |
| `Landed`        | `landed` at the row's end   | dim    | it landed on its parent and closed     |
| `Dropped`       | `dropped` at the row's end  | dim    | it was dropped and closed              |
| `Asks`          | Asks you a question         | blue   | live, and a plugin waits on an answer  |
| `Working`       | Working · turn N            | muted  | live                                   |
| `ConflictsOnMain` | Conflicts on main · N files | red  | a main chat a turn left conflicts on   |
| `Interrupted`   | Interrupted · tau closed    | muted  | it was going when tau closed           |
| `Failed`        | (none; the red warning)     | red    | it stopped with an error               |
| `Queued`        | Queued · lands after main's turn, or Queued · needs confirmation | amber | it waits in its main chat's landing queue |
| `WouldConflict` | Would conflict in N files   | red    | a stopped fork whose landing conflicts |
| `ReadyToLand`   | Ready to land · N changes   | green  | a stopped fork with changes to land    |
| `Idle`          | (none)                      |        | anything else                          |

The first that holds, top to bottom, wins. A working chat keeps a
plugin's line (a goal's) when it has one. Asking and ready-to-land
rows are tinted. A queued chat needs confirmation when it would
conflict in a file the person did not confirm
(`Waiting::needs_confirmation`).

- **Order:** a state never moves a row. Main stays first; the rest
  keep newest first.
- **Ended chats stay listed:** a chat that landed or was dropped is
  closed, but its row stays, dim, with no line and no close button. A
  chat closed by hand leaves the sidebar.
- **The pill:** a repository's row says "N need you": its open chats
  that ask, would conflict, are ready to land, or wait in the queue
  for a confirmation, and a main chat with conflicts on it.
- **Phones** fold the same facts from the snapshot and the updates
  they get, so their list says the same.

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

`tau_ui::notify` notifies while tau's window is not focused:

| Kind              | Title                       | Body                                            |
| ----------------- | --------------------------- | ----------------------------------------------- |
| `Asks`            | `<chat> asks you`           | the question                                    |
| `ReadyToLand`     | `<chat> is ready to land`   | `N changes on <parent> · <repo>`                |
| `ConflictsOnMain` | Conflicts are still on main | `tau's turn left conflicts in N files · <repo>` |
| `Failed`          | `<chat> failed`             | `The run stopped with an error · <repo>`        |

- `Notifier` sends one notice each time a chat's state turns into one
  of these; a chat seen for the first time gets none, and a turn while
  the window is focused is kept quiet for good.
- `notify::conflicts_on_main(main, repo, files, cx)` says conflicts
  left on main. tau-ui's `main` passes it to
  `Host::on_conflicts_on_main` before `attach`, so the host calls it
  each time a turn of main ends with conflicts still on it (ADR 0024).
- The desktop gets them through `notify-rust` over zbus (no libdbus).
  Clicking one brings tau's window up and opens its chat.
- `"notifications": false` in `interface.json` turns them off.
