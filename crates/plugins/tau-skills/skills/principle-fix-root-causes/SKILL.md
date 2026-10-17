---
name: principle-fix-root-causes
description: "Load when debugging or fixing a bug. Trace each symptom back to the first place the system went wrong and fix it there, after committing a failing reproduction. A guard is a fix only when the state it handles is legitimate."
---

# Fix Root Causes

When debugging, fix the fault, not the place where it showed. The symptom is where a wrong state became visible. The root cause is the first place the system did something it should not do, or failed to handle a state it should handle.

**Why:** A symptom fix hides the evidence and leaves the fault free to appear somewhere else. Each one makes the system harder to reason about, because the code now carries a workaround for a bug nobody named.

**Pattern:**
- Reproduce first. Write the reproduction as a test that fails on the current code, run it, and watch it fail for the reason you expect.
- Commit the failing test on its own with `vcs_commit` (for example `test(parser): a trailing comma drops the last field`). Then fix, run it again, watch it pass, and commit the fix separately. The reviewer sees red, then green. If the repository's checks forbid a failing test in a commit, mark the test as expected to fail in the first commit and remove the mark in the fix.
- Ask "why" at each step until the answer is a wrong decision in the code, not a consequence of an earlier one.
- When stuck, instrument. Read the actual error and the actual state. Do not guess.
- Ask whether the state that triggered the symptom is legitimate. If the system may be in that state, the code that broke on it is the root cause, and handling the state there is the fix. If the state should never exist, a check where it broke hides the bug. Find what produced the state. Guards, retries, swallowed errors, clamped values, and discarded state are the usual ways to hide a bug.
- When something fails only after a restart, suspect persisted state first: config files, caches, lock files, serialized state.
- Check for the pattern, not just the instance. `grep` for every place with the same fault and fix them all.
- If a workaround needs a paragraph-long comment to justify it, the code is wrong. Fix the code, not the comment.
- If the deciding fact took a deep dig to find, propose the cheap, always-on signal that would have shown it at first look, such as a log line, an assertion, or a better error message.

You skipped this when the fix adds a null check, a retry, a default value, or an exception handler at the place that crashed, and you cannot say what produced the bad state.

Distinct from `principle-attack-the-premise`, which applies after several fixes on one premise have failed. This principle is the first pass on any bug. The `playbook-bug-fix` skill runs it end to end.
