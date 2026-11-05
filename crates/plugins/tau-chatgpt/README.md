# tau-chatgpt

Runs tau's agents on the person's ChatGPT plan: the plugin that fills
the `tau.provider` slot (ADR 0045). While the active ChatGPT sign-in
has plan usage, it gives runs the plan's client, the picker the plan's
models, and the kernel the plan's last refusal and when a
conversation's prompt cache lapses; signed out, it gives none.

## What it provides

| Item           | What it is                                                              |
| -------------- | ----------------------------------------------------------------------- |
| `NAME`         | `"tau-chatgpt"`                                                         |
| `ChatGptUi`    | The plugin's `UiPlugin`: its row on the Plugins screen                  |
| `plan_account` | The active account in tau's config directory, when it has plan usage   |
| `ChatGptHost`  | The `HostHalf`: fills `tau.provider` from the saved sign-ins (`host`)   |
| `Over`         | A provider over a client given it, always signed in: for tests (`host`) |

## How it fits

One crate holds both halves. The sign-ins are kept by `tau_ai::chatgpt`
in `chatgpt/` under tau's config directory.
