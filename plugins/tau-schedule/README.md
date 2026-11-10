# tau-schedule

A chat wakes itself later, once, with a note to itself (ADR 0039). The
model calls `schedule` with a name, a note and when: `in` a while
(`90m`, `2h`, `3d`) or `on` a local date and time. When it comes, the
chat takes a turn of tau's whose message is the note, after any turn
going ends. `scheduled` lists what waits, `unschedule` cancels one, and
the person sees each in the chat's inspector and may cancel it there.
It is a Luau plugin tau comes with (ADR 0050): this folder is its
package, with the tests and the fixtures the demo shows of it.

## What it provides

| File                | What it is                                                                |
| ------------------- | ------------------------------------------------------------------------- |
| `src/plugin.luau`   | The tools, `start`, the cancel action, the `wake` job and the view        |
| `src/wake.luau`     | The limits, names, the message a chat wakes on and that message read back |
| `tests/plugin.luau` | The tools, the job, `start` and the view, as cases                        |
| `tests/wake.luau`   | Names and the message, as properties                                      |
| `tests/wakes.luau`  | Waking against a model, with tau closing as it wakes chats                |
| `fixtures/`         | What the demo shows of it, and what it declares                           |

## How it fits

What a chat has waiting is in the plugin's store in the chat's
repository, `wake/<chat>/<name>` each, and in the chat's state, which
its view draws; each turn's `start` reads the store again. Its job,
`wake`, ticks each second (`uses.chats`): it reads every repository's
store, and wakes each chat with something due through
`ctx.chats.wake`, once its turn may start. A wake-up leaves the store
only once its turn started, so it wakes its chat at least once.

An agent's runs leave it out: an agent's own `schedule` also repeats
(ADR 0037). A sub-agent's `schedule` refuses, as nothing would wake it.

## Testing

```sh
target/release/tau-plugin test plugins/tau-schedule   # its Luau tests, no compiling
target/release/tau-plugin test plugins/tau-schedule --bless   # writes its fixtures anew
cargo nextest run --release -p tau-built-in-tests -E 'test(/^schedule::/)'
```

The first runs its Luau tests as tau loads it, and checks the fixtures
`tests/fixtures.luau` makes are those in `fixtures/`. The third wakes a
chat through the real host: a chat's tool keeps the wake-up, the job
wakes the chat on it in the host's local time, and the turn it starts
may open the same store.
