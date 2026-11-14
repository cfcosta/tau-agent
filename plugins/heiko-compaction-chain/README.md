# heiko-compaction-chain

The chain of compactions, as the plugin that fills `heiko.compaction`
(ADR 0045, 0050): fast pruning first, then the tree when it is switched on,
then the summary, each declining when it would save too little, the
first rewrite winning. heiko-compaction is Luau too, and runs after the
others in the order heiko comes with them.

It is a Luau plugin heiko comes with: `src/plugin.luau` declares the
chain in its `compaction` table, and heiko-luau-plugins loads it as
always active, filling the slot for it. The Plugins screen shows the
slot filled by heiko-luau-plugins, whose own plugins' rewrites run first.

## What it provides

| Item    | What it is                                   |
| ------- | -------------------------------------------- |
| `NAME`  | `"heiko-compaction-chain"`                     |
| `FILES` | The Luau files, by their paths in its folder |

`heiko-plugin test plugins/heiko-compaction-chain` loads the plugin as heiko
does and runs its Luau tests.
