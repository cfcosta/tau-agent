---
name: principle-outcome-oriented-execution
description: "Load during planned rewrites and migrations with explicit phases. Converge on the target architecture, one commit per phase, instead of keeping every intermediate state working with throwaway compatibility code; finish with the repository's full test and lint run."
---

# Outcome-Oriented Execution

Optimize for the intended, verifiable end state, not for smooth intermediate states.

**Why:** Keeping every intermediate step fully stable often means temporary compatibility code, and temporary code tends to stay. Converge on the target architecture and prove correctness at explicit verification points.

**Core rule:**
- End-state integrity comes before transitional stability.
- Intermediate breakage is acceptable when it is planned, scoped and reversible.

**Guardrails:**
- Use this for planned rewrites and migrations with explicit phases. Write the phases down before you start, as a numbered checklist in your notes, or on the canvas for long work.
- Each phase is a commit, or a short run of commits. The `vcs_commit` message says what the phase did and, if it leaves something broken on purpose, what is broken and which phase fixes it. A reviewer reading `vcs_log` sees the plan.
- Declare where temporary breakage is acceptable before you cause it. "Phase 2 leaves the old config keys unread until phase 3" is a declaration. Finding the breakage afterwards is not.
- Keep high-signal checks on the areas you are touching while you migrate: the tests and compiler errors for the module you just changed. Do not wait for the end to learn that a phase went wrong.
- Keep each phase reversible. `vcs_undo` reverts the last operation, and `vcs_restore` with `from` brings paths back from an earlier change.
- Finish with full verification on the final state: the repository's whole test suite and all its linters, as its instructions define them. The rewrite is not done until that run is clean. On a chat, run it before `vcs_land`.

You skipped this when the diff adds an adapter that translates the old shape into the new one "for now", and no later phase deletes it.

Distinct from `principle-sequence-verifiable-units`, which keeps each unit green before the next. This principle lets a planned intermediate phase break, and moves the full proof to the declared boundaries and the end. Distinct from `principle-migrate-callers-then-delete-legacy-apis`, which is the specific case of replacing one API.
