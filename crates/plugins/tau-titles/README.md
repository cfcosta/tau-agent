# tau-titles

Writes a run's title from what it is about, as Codex names its threads:
one request to the plan's newest Luna at low effort (the run's own model
when the plan has none), its reply held to a JSON object of one short
`title`. It fills the `tau.title` slot (ADR 0045). Until the title is
written, and when it cannot be, the run shows the first line of its
prompt, which is the kernel's. What the requests cost is booked to the
plugin, so the Plugins screen shows it.

## What it provides

| Item                            | What it is                                                          |
| ------------------------------- | ------------------------------------------------------------------- |
| `NAME`                          | `"tau-titles"`                                                      |
| `TitlesUi`                      | The plugin's `UiPlugin`: its row on the Plugins screen and settings |
| `Settings`                      | Whether titles are written (they are, until switched off)           |
| `request`, `format`, `parse`, `model` | The request, the reply's schema, its title, and the model asked |
| `TitlesHost`                    | The `HostHalf`: fills `tau.title` (feature `host`)                  |

## How it fits

One crate holds both halves. The request, the reply's schema and the
settings always build; asking the model sits behind the default `host`
feature.
