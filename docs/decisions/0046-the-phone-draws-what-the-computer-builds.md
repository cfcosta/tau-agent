# 0046: The phone draws what the computer builds

- Status: proposed. Builds on [0045](0045-everything-is-a-plugin.md):
  plugins' views are trees of plain data, so they can cross to the
  phone. Amends [0013](0013-phones-connect-to-a-running-tau.md): the phone runs no
  plugin's code, only tau's kernel.
- Date: 2026-11-04

## Context

Since 0045, no plugin draws gpui: each view is a `tree::Node` and each
click an action for `UiPlugin::on_action`. The phone still runs the
same `Workspace` as the computer, with every plugin's UI half linked
in:

- **It runs plugins' code.** It folds their records into its runs,
  builds their trees, lists their commands and carries out their
  actions with its own copy of each plugin, which has to be the same
  version as the computer's.
- **Its app grows with every plugin.** The APK links each plugin's UI
  crate, and each new plugin is a new phone build.
- **Luau plugins already live on the computer.** Their trees are built
  there and sent as records. Rust plugins should work the same way.

The phone already has what a view reads: the computer sends it every
run, the catalog and the UI state (0045's "The interface's state is
data"). So the computer can build any view the phone would build, given
where it is drawn and the phone's layout.

## Decision

The phone asks the computer for each plugin view it draws, and draws
the tree that comes back. A click is sent to the computer, which runs
the plugin's action and sends back what is for the phone's window.

### Views by name

Each place a plugin's output appears is named by a `ViewKey`:

- **The plugin.**
- **What is drawn:** a point and where (its context's wire form, below);
  a page and its parameters; the settings pane, with its scope and
  entry; a command's popover and what follows the command; the
  commands listed where the composer is.
- **The layout:** compact or not, and the width, in steps, since a view
  may lay itself out by them.

A point's context crosses as `AtKey`, which names what the computer
rebuilds from its own state: the app; a repository; a run; a message
or an anchor by its run and its place; a rewrite; a card by its run and
call; setup. A card's data, a run's info and the like never cross
whole.

What comes back is JSON: trees, and the typed outputs (`NavEntry`,
`PluginStatus`, `PlanField`, `RowNote`, `CardView`, `PaneView`,
`SetupView`, the context trigger, commands), made `serde`.

### Asking and answering

- **The phone asks.** A phone's `Workspace` answers plugin points,
  pages, panes and commands from its view cache. A miss sends
  `PhoneUp::Views` with the keys it lacks. The place draws as empty
  until the answer comes, or with what it had before.
- **The computer answers.** It builds each view with its registry over
  its own `Workspace`, as it would draw it (same run, parameters and UI
  state; the phone's layout), and sends `HostUpdate::Views` to that
  phone alone. Building a view is pure (0045), so the same key always
  gives the same answer for the same state.
- **Staying fresh.** The computer keeps the keys each connected phone
  asked for. After an update that can change a plugin's view (a fold, a
  plugin's data or settings, UI state, a run's change), it builds them
  again and sends those whose answer changed. A phone drops a key it no
  longer draws after a while, and the computer forgets a phone's keys
  when it goes.

### Actions

- **A click goes up.** On the phone, a plugin's own action in a tree
  sends `PhoneUp::Action`: the plugin, where the tree was drawn
  (origin), the action, its arguments and the layout.
- **The computer acts.** It runs `on_action` with a view made from the
  origin. What the action asks splits in two:
  - **Shared requests** (`act`, settings, record, steer, send, start,
    UI state, cancel, fetch): the computer carries them out, and every
    interface sees the result.
  - **Window requests** (navigate, composer, alert, note, run details,
    open pane, submit, open run, open URL, copy, reveal, open, setup
    done): these go back to the phone that clicked, which carries them
    out on its own window.
- **Commands and keys go the same way.** The phone sends the command's
  name and arguments, and the computer runs it.
- **A reply goes to the window that asked.** An action's answer from
  the host half (`UiPlugin::reply`) runs on the computer. Its window
  requests go to the window whose action asked.

### What the phone keeps

The phone keeps what the kernel draws itself: runs, the catalog and the
UI state. It folds no plugin's records, and links no plugin crate: the
kernel names none, and tau-plugins lists them for the computer. Landing
shows a landing's changes through a point, agents' sidebar section is a
repository kind, and what waits on the person is a point the phone's
notifications watch.

## Order of work

Each step lands on main by itself and leaves the phone working.

1. **Wire forms.** `AtKey`, `ViewKey`, and `serde` on the typed outputs
   and on `Request`, with property tests that each reads back what it
   wrote.
2. **The computer builds views.** A pure `Workspace::build_view(key)`,
   tested against what the desktop draws for the same place.
3. **The phone asks.** A remote `Workspace`'s view cache, the asks and
   the answers. The computer keeps each phone's keys, and pushes what
   changed. Tested with a computer and a phone in one process, as the
   sync tests do.
4. **Actions from the phone.** `PhoneUp::Action`, commands and keys,
   with window requests sent back and replies routed to the window that
   asked.
5. **No plugin code on the phone.** The phone's registry holds no
   plugins. It stops folding, then stops linking them once the kernel
   names none.

## Alternatives considered

- **Keep linking the UI halves on the phone.** This is how it works
  today. It needs two copies of each plugin at the same version, and
  a phone build for every new plugin. Luau plugins already do without.
- **Send every view eagerly.** Every card times every point, for every
  run, would be unbounded and mostly unseen. The phone asks for what it
  draws.
- **Send whole contexts.** A card's data or a run's info would cross
  with every ask. The computer already has them, so only their names
  cross.

## Consequences

- A view the phone draws for the first time waits one round trip.
  After that, changes arrive pushed, as runs do now.
- The computer does the phone's plugin work: it builds the trees of each
  connected phone. Views are pure and cheap, and Luau's 200 ms limit
  still bounds a Luau view.
- Plugin versions cannot differ between the computer and the phone,
  since only the computer has plugins.
- A plugin's view that reads the computer's own window state, rather
  than the UI state, shows the computer's on the phone. 0045 already
  made such state data, so this is a bug in that plugin.
