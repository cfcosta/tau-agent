# tau-compaction-chain

The chain of compactions, as the plugin that fills `tau.compaction`
(ADR 0045, 0050): fast pruning first, then the tree when it is switched on,
then the summary, each declining when it would save too little, the
first rewrite winning. It is the slot's default; a person who wants only
the summary picks tau-compaction for the slot on the Plugins screen, and
the kernel turns off the others' rewrites.

It is a Luau plugin tau comes with: `luau/plugin.luau` declares the
chain in its `compaction` table, and tau-luau-plugins loads it as
always active, filling the slot for it. The Plugins screen shows the
slot filled by tau-luau-plugins, whose own plugins' rewrites run first.

## What it provides

| Item    | What it is                                   |
| ------- | -------------------------------------------- |
| `NAME`  | `"tau-compaction-chain"`                     |
| `FILES` | The Luau files, by their paths in its folder |

`tests/luau.rs` loads the plugin as tau does and runs its Luau tests.
