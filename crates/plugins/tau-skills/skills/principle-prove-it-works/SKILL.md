---
name: principle-prove-it-works
description: "Load after finishing a task or receiving a sub-agent's report, before you say it is done. Verify against the real artifact (run the repository's tests, the real app, read the actual value, inspect the diff), not a proxy, a self-report, or \"it compiles\"."
---

# Prove It Works

Verify every task output by checking the real thing directly. Do not infer from proxies, self-reports, or "it compiles".

**Why:** Unverified work has unknown correctness. Indirect checks (file modification times, output freshness, a sub-agent's summary, an old screenshot) feel cheaper than looking. Acting on a wrong inference costs far more than checking the source.

Check the real thing, not a proxy:
- Run the repository's own test command, from its instructions or its build files. Do not substitute a narrower command you assume is equivalent.
- For behavior a person sees, run the real app. Use the repository's `verify-*` skill if it has one, or whatever its instructions say to run. Take screenshots from a headless compositor, never from the person's display, and put them on the canvas with `canvas_show`.
- Check process liveness directly, not through state derived from it.
- Read the actual value, not a cached or derived copy.
- Inspect the diff you are about to commit with `vcs_diff`. Make sure it holds what you meant and nothing else.
- When verification fails, suspect the observation method before you suspect the system.

## A sub-agent's report is a claim

When a sub-agent ends, tau starts your turn with what it says it did. That is a claim, not proof. Read its commits with `vcs_show` or `vcs_diff`, then run the check yourself. You own the result and the summary you give the person.

## Script the check when you can

The strongest proof is a deterministic script that reruns the same comparison, not a one-time look. Write the script, run it, and keep its output where a reviewer can rerun it instead of trusting your word. A test in the repository is the best home for it.

The proof must show up in your tool results. When the person sets a `/goal`, tau checks each stop against your last answer and your recent tool results, and tells you to run what proves it. Run the proving command last, so its output is the evidence. When you suggest a goal to the person, phrase the condition as a check that a command can show, such as "the repository's tests pass and the new test fails on the old code".

For large or long work, such as a big port or migration, keep the evidence visible. Put the commands you ran, their results, and any screenshots on the canvas, so the person can audit the trail later.

You skipped this when your "done" rests on a build that compiled, a test you did not run, a summary you did not check, or a value you inferred instead of read.

Distinct from `principle-explain-the-number`, which asks whether a measured number means what you think. This principle asks whether the output is real at all.
