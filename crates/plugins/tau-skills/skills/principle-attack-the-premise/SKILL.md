---
name: principle-attack-the-premise
description: "Load when two or more fixes that share one premise have failed the same gate (the same test, check or flaky failure). Write the premise down and test it directly, with a rerunnable check, before you write another fix that assumes it."
---

# Attack the Premise

When two or more fixes that share one premise have failed the same gate, suspect the premise, not the fixes.

**Why:** Each failed fix is evidence against the belief it assumed. A third fix on the same premise spends effort where the evidence already says the cause is not.

**Pattern:**
- **Write the premise down.** The premise is the one sentence every failed fix assumed, for example "the test fails because the cache is stale". Put it in your notes or your reply.
- **Test the premise before the next fix.** Find an observation that would come out differently if the premise were false, and make it. Look at how the failures are distributed, not only at how large they are: which tests, which inputs, which runs, which order.
- **Make the check rerunnable** (load the `principle-build-the-lever` skill). A filtered test run that targets the one failing case, or a small script in a scratch directory outside the repository that sets up the condition and prints the observation. Keep the exact command so you can rerun it after each change.
- **Flaky tests are the usual case.** "It fails because of a timeout" is a premise. Before you raise the timeout or add a retry, run the filtered test 50 or 100 times in a loop with `exec_command`, count the failures, and write down what the failing runs share (test order, parallelism, seed, machine load). If a raised timeout does not change the failure rate, the premise was wrong.
- **Follow the evidence.** If it contradicts the premise, the cause is elsewhere: look for what the failures have in common that the premise did not predict. If it confirms the premise, ask the next "why" (load the `principle-fix-root-causes` skill). The earlier fixes may have treated a consequence instead of the cause.
- **Remove the condition instead of compensating for it** (load the `principle-laziness-protocol` skill). If the evidence shows one part always lands on the losing side, change what puts it there. A mechanism that cleans up after it leaves the cause in place and adds work on every run.

**Stop:** Do not start the next fix until the premise is written down and tested.

Distinct from `principle-redesign-from-first-principles`, which rebuilds a design around a new requirement. This principle questions a fact the current design assumes.
