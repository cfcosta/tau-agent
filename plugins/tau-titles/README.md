# tau-titles

Writes a run's title from what it is about, as Codex names its threads:
one request to the plan's newest Luna at low effort (the run's own model
when the plan has none), its reply held to a JSON object of one short
`title`. It fills the `tau.title` slot (ADR 0045, 0050). Until the title is
written, and when it cannot be, the run shows the first line of its
prompt, which is the kernel's. What the requests cost is booked to the
plugin, so the Plugins screen shows it.

It is a Luau plugin tau comes with: `src/plugin.luau` asks through
`tools.infer` with `uses.model = "luna"`, a family the host resolves to
the plan's newest (else the run's model), and `src/titles.luau`
builds the request and cleans the title. tau-luau-plugins loads it as
always active and fills `tau.title` for it; what it asks is booked to
`tau-titles`.

## What it provides

| Item    | What it is                                   |
| ------- | -------------------------------------------- |
| `NAME`  | `"tau-titles"`                               |
| `FILES` | The Luau files, by their paths in its folder |

`tau-plugin test plugins/tau-titles` loads the plugin as tau does and
runs its Luau tests: cases for the request and the switch, and
properties for the budget and the cleaned title.
