# tau-demo

tau's demo: a scripted session over the real interface (tau-ui-remote)
and the plugins' real host halves, with no agent and no network. The
`tau` app opens it with `--demo` (and `--open <screen>` for one of
`SCREENS`); tau-ui's tests drive the interface with it, and the README's
screenshots are taken from it.

| Module    | What it holds                                                         |
| --------- | --------------------------------------------------------------------- |
| `lib`     | The `retry-after` session: its runs, events and the plugins' data     |
| `host`    | `DemoHost`: the plugins' host halves over a store of the demo's own   |
| `screens` | `SCREENS`: each screen the demo opens, by name, and how it gets there |

It is the one place besides tau-ui's list of host halves that names
plugins by design: it shows each plugin's own screens, with their data.
