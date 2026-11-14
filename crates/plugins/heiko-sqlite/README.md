# heiko-sqlite

Keeps every run in SQLite: the plugin that fills the `heiko.store` slot
(ADR 0045) with `heiko-store-sqlite`. The host opens its store through the
slot as it starts, before any plugin's host state is made, so the
plugin fills it from its half alone (`HostHalf::fill_early`).

## What it provides

| Item         | What it is                                               |
| ------------ | -------------------------------------------------------- |
| `NAME`       | `"heiko-sqlite"`                                           |
| `SqliteUi`   | The plugin's `UiPlugin`: its row on the Plugins screen   |
| `SqliteHost` | The `HostHalf`: fills `heiko.store` early (feature `host`) |

## How it fits

One crate holds both halves; opening the store, with SQLite bundled,
sits behind the default `host` feature.
