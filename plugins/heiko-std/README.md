# heiko-std

The standard part of `heiko`: what `require("heiko")` gives every plugin
beyond declaring itself and drawing (ADR 0058). It is not a plugin heiko
runs. Its files are read from heiko's packages folder, as the plugins'
are, and installed into the `heiko` module of every plugin's VM.

| File              | Gives                                                              |
| ----------------- | ------------------------------------------------------------------ |
| `src/text.luau`   | `heiko.text`: `is_space`, `trim`, `one_line`, `prefix`, `cut`, `codes` |
| `src/count.luau`  | `heiko.count`: `plural`, `tokens`                                  |
| `src/values.luau` | `heiko.values`: `count`, `copy`, `same`, `is_array`, `contains`    |
| `src/store.luau`  | `heiko.store`: `get`, `list`, `put`, `delete`, `bump`, `of(repo)`  |
| `src/jev.luau`    | `heiko.jev`: `ask`, `scores`                                       |
| `src/infer.luau`  | `heiko.infer`                                                      |
| `src/clock.luau`  | `heiko.now`                                                        |

Each file returns a function that takes `heiko` and adds its part.
`src/plugin.luau` is an empty plugin, there so the tests have a store,
Jev and the model to reach.

A function belongs here when several plugins wrote it by hand and it
has one right answer. It keeps its name and its meaning once here.

## Tests

`heiko-plugin test plugins/heiko-std` runs `tests/`: cases and properties
for every function, the ones that await called as a hook calls them
(`run:call`). The host checks `heiko.text.is_space` against Rust's own on
every code point (`crates/plugins/heiko-luau-plugins-host/tests/std.rs`).

heiko reads the files once, as it starts.
