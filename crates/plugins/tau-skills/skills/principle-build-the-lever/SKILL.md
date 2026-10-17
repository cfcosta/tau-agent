---
name: principle-build-the-lever
description: "Load for any non-trivial work, not only bulk work: edits, migrations, analyses, checks. Build the tool that does it or proves it (codemod, script, generator, or a recipe your sub-agents follow) instead of working by hand. The tool is what a reviewer reruns."
---

# Build the Lever

When the work is not trivial, build the tool that does it instead of doing it by hand.

**Why:** A codemod, generator or script does the work the same way every time and reruns for free. The tool is one artifact a reviewer can read and rerun to check the work. Hand-done changes can only be re-verified by redoing them. A deterministic script turns "trust me" into "run this".

Default to building the lever. Skip it only when the task is trivial: a couple of obvious edits you can see at a glance.

**Pattern:**
- Do the first unit by hand to learn the recipe, then build the tool. Prove it on that unit: commit the hand-done version, `vcs_restore` those paths `from` the change before it, run the tool, and check that `vcs_diff` comes out empty. Make the lever safe to rerun.
- Pick the kind that fits: a codemod or script for edits, a generator for repetitive files, a query over a dump (sqlite, jq, or a `codemode` script) for analysis, a rerunnable check for verification.
- A deterministic lever beats fan-out. If the tool can process every unit in one pass, run it yourself. Do not `spawn` sub-agents to hand-apply what a script can do.
- When you do fan out, write the lever for the delegates: the recipe, the verification command each must pass, and the do-not-touch fences, in one place. Put it in the `spawn` task text, or in a reference file every delegate reads. Say in words that delegates must not edit that file; tau has no read-only flag, so the fence lives in the brief.
- If the lever outlives the session (a check others will rerun, a generator for future files), commit it in the repository's scripts directory. Look for where the repository keeps scripts before creating one. If it is single-use, keep it in a scratch directory outside the repository and give its path in your reply.

Applying this principle produces a file. If you cited it and there is no codemod, script, generator or delegate recipe in the diff (or, for a scratch lever, named by path in your reply), you did not apply it.

The bar is triviality, not repetition. A one-off still earns a lever when the lever is what makes the work checkable. Build the smallest script that does or proves the job, never a framework (load the `principle-laziness-protocol` skill).

Distinct from `principle-encode-lessons-in-structure`, which turns a recurring instruction into a durable guardrail. This principle is about throughput and reviewability on the work in front of you. For scripting the verification itself, load the `principle-prove-it-works` skill.
