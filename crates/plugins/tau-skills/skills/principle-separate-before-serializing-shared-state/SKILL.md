---
name: principle-separate-before-serializing-shared-state
description: "Load when concurrent actors (threads, processes, workers, or spawned sub-agents) might write the same file, key, branch, or state object. Eliminate the sharing first, split work so file sets do not overlap, and serialize structurally only when one shared writer is a real invariant."
---

# Separate Before Serializing Shared State

When concurrent actors might share mutable state, first ask whether they need the same mutable object. If not, eliminate the sharing. When the sharing is real, enforce serialization structurally with lockfiles, sequential phases, or exclusive ownership. Instructions and conventions are not concurrency control.

**Why:** Concurrent writes to shared state create races that are intermittent, hard to reproduce, and expensive to debug.

**Pattern:**
1. **Identify shared mutable state.** Look for files that several actors both read and write, branches several actors commit to, and interfaces one actor defines while another consumes them.
2. **By default, eliminate the shared write target.** Ask whether the actors need one canonical object or are publishing independent facts. Give each actor its own file, key, branch, or state directory, and merge only where the results are read or reported. Two workers writing their own `last_x` field into one `state.json` is still shared mutation. `indexer-state.json` and `metrics-state.json` is not.
3. **Serialize only when one shared write target is a real invariant.** Use a lockfile, sequential phases, a single-writer actor, or an atomic compare-and-swap. Treat "we need a lock" as a design smell to check, not as the default answer.

## Splitting `spawn` tasks

Sub-agents are concurrent actors. Each works in its own workspace, and its commits land on the caller when it ends, unless it was spawned with `land: false`. Two sub-agents that edit the same file both land, and the overlap arrives as a conflict the caller resolves by hand with `vcs_resolve`, after both writers are gone. Plan the split so that cannot happen.

- Before you spawn, write down each task's file set. Make the sets disjoint.
- If two tasks need the same file, give the file to one task. Otherwise make the shared edit yourself first and commit it before spawning, or run the second task after the first lands.
- Watch for the usual shared files: a module list, a workspace members list, a registry, an index, a changelog. Give each task its own new file where you can, and wire the files together yourself after the tasks land.
- Competing candidates do not need disjoint files. Spawn them with `land: false`: their commits stay beside yours and never land, so they cannot clash. You take the winner's files with `vcs_restore {paths, from}`. The `arena` skill does this.
- Name the file set in each brief ("edit only these files"). The brief alone does not prevent a clash. The disjoint split does.
- After the sub-agents land, run `vcs_status` and look for conflicts before you build on their work.

On a chat or a sub-agent, where `spawn` is refused, you run the tasks one after another, so this section does not apply.

You skipped this when you needed a lock, a retry loop, or a conflict resolution that a different split would have made unnecessary.

Distinct from `principle-make-operations-idempotent`, which makes one operation safe to repeat over time. This principle keeps several actors from writing the same thing at once.
