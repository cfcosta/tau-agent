# tau-compaction-chain

The chain of compactions, as the plugin that fills `tau.compaction`
(ADR 0045): fast pruning first, then the tree when it is switched on,
then the summary, each declining when it would save too little, the
first rewrite winning. It is the slot's default; a person who wants only
the summary picks tau-compaction for the slot on the Plugins screen, and
the kernel turns off the others' rewrites.

## What it provides

| Item        | What it is                                                    |
| ----------- | ------------------------------------------------------------- |
| `NAME`      | `"tau-compaction-chain"`                                      |
| `ChainUi`   | The plugin's `UiPlugin`: its row on the Plugins screen        |
| `ChainHost` | The `HostHalf`: fills `tau.compaction` with the three in turn |
