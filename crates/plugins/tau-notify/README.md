# tau-notify

Shows tau's notices on the desktop while its window is not in front: a
chat asks the person something, is ready to land, failed, left
conflicts on main, or something of a plugin's waits on them. Clicking a
notice brings tau up on its chat. The plugin fills the `tau.notify`
slot (ADR 0045); the kernel decides when a notice is due, and this
plugin decides where it goes.

## What it provides

| Item         | What it is                                                                   |
| ------------ | ---------------------------------------------------------------------------- |
| `NAME`       | `"tau-notify"`                                                               |
| `NotifyUi`   | The plugin's `UiPlugin`: its row on the Plugins screen and its settings pane |
| `Settings`   | Whether notices are on (they are, until switched off)                        |
| `NotifyHost` | The `HostHalf`: fills `tau.notify` with the desktop's notifications (`host`) |

## How it fits

One crate holds both halves. The settings and the pane always build;
the desktop's notification server sits behind the default `host`
feature, over D-Bus on Linux and FreeBSD (nothing elsewhere).
