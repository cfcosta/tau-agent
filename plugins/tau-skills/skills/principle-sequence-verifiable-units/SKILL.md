---
name: principle-sequence-verifiable-units
description: "Load for multi-step work (sweeps, migrations, runs of similar edits) and when deciding where to commit. Break the work into small units that each end in a check, verify each before the next, and commit them in an order that reads as an argument."
---

# Sequence Work into Verifiable Units

Order work as a sequence of small units, each ending in a state you can check. Do not start the next unit until the current one is green.

**Why:** A break caught at the unit that caused it is cheap to find. A break caught after a batch is buried, and you have already built on a broken base. Committing those same units in an order a reviewer can replay turns "trust me" into "watch it go red, then green".

**Execution.** In a sweep, a migration, or any run of similar edits, verify each change before you start the next. Each unit is a bracket: a known-good state, one change, the check, then the next unit. Start from a clean, committed state (`vcs_status` shows nothing pending) so the first check measures against the real baseline. When a script does the edits, the check per unit costs almost nothing. Run it anyway.

**Delivery.** Each unit ends in a `vcs_commit`. Commit wherever a reviewer would want a boundary, with a Conventional Commits message that says what the unit does. A chat's commits land on main as a stack when you call `vcs_land`, and the person reviews them in order, oldest first. On the main chat, the commits go straight to trunk and the person reads them in the same order. Either way the stack is the argument, so order it to prove the work:

- The failing test first, then the fix on top.
- A subtraction (dead code removed, a wrapper collapsed) before the reshape.
- A baseline measurement or harness before the change it measures.
- The scaffold before the feature that fills it.

Each commit builds and passes the checks on its own. A commit that mixes a refactor with a behavior change hides the behavior change, so split them while you work. tau has no split or squash tool, so the boundaries you choose as you go are the ones the person sees.

You skipped this when one commit holds a whole sweep, when a check first ran after the tenth edit, or when a commit in the middle of the stack does not build.

Distinct from `principle-prove-it-works`, which keeps each check real. This principle decides where the checks and commits go. The `principle-build-the-lever` skill makes the check per unit cheap.
