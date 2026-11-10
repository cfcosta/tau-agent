---
name: tdd
description: "Load when the person asks for TDD, a failing test, or a regression test, or when a bug has an obvious cheap local test. Makes you commit a focused test that fails for the bug's reason before the fix, then make the smallest fix that turns it green."
---

# TDD bug fix

When a bug has a clear, cheap test path, make the broken behaviour executable before you change production code. The goal is one focused regression test that fails before the fix and passes after it, with the failing test committed on its own first so history shows both states.

Do not force a test when it would be impractical. If the test would need broad harness setup, brittle mocks, slow end-to-end infrastructure, production-only state, vague reproduction steps, or large unrelated fixture churn, skip the new test and use the closest useful check instead.

## Steps

Copy these into your working notes as a numbered checklist and tick each off. A step you do not do stays as `skip: <reason>`.

1. Understand the bug. Write down the intended behaviour, the current behaviour, the code path, and the smallest observable reproduction.
2. Find the test command. Take it from the repository's instructions. If they do not name one, look at what the repository already uses: its build and task files, its CI configuration, its package manifest. Run the existing suite for that area once so you know its baseline state.
3. Choose the narrowest executable check. Prefer the unit, component, integration, or regression test layer the repository already uses for that code path. If no practical test path is obvious, do not build one from scratch to satisfy this skill; go to "If a failing test is impractical".
4. Write the failing test. Add the smallest test that would have caught the bug. It encodes the intended behaviour, not the current implementation (load `principle-test-behavior-not-implementation`).
5. Run it before the fix. Confirm it fails, and that it fails for the bug's reason. If it passes, or fails for an unrelated reason (a typo, a missing fixture, a compile error elsewhere), fix the test or the reproduction before you touch the implementation. Save the failing output.
6. Commit the test on its own. Call `vcs_commit {message, paths}` with only the test's files, before any change to production code. Use a Conventional Commits message that names the behaviour, such as `test(parser): reject a header with no name`. On the main chat this commit goes straight to trunk, so make the fix commit right after it, in the same turn.
7. Fix the bug. Make the smallest production change that satisfies the intended behaviour and keeps nearby contracts. Load `principle-fix-root-causes` if you have not yet found where the system first went wrong.
8. Rerun the test and the area's suite. The new test passes. Nothing that passed in step 2 now fails.
9. Commit the fix on its own, as `fix(scope): subject`.

## If a failing test is impractical

Use the closest executable regression check instead: a targeted script, a reproduction command, a headless UI run, a snapshot comparison, a log assertion, or a focused integration check. Run it before and after the fix, the same way.

No new test beats a bad test. A bad test mostly tests mocks, encodes implementation details, depends on timing or unrelated global state, needs expensive infrastructure for a small fix, or would be deleted right after it proved the fix.

## Guardrails

- Do not change a test to match a wrong implementation.
- Do not weaken an existing assertion unless the expected behaviour has changed and the reason is clear. Say the reason in the commit message.
- Keep the test on the bug. No broad fixture churn, no unrelated coverage.
- If the bug is flaky, make the test deterministic where you can (fixed seeds, injected clocks, no sleeps) and write down which signal it locks.
- If the bug exposes a class of failures, land the focused regression first, then consider sibling coverage in later commits.

## Reply

Report the evidence, not only the outcome.

- The failing-before test or check, the failure it produced (verbatim), and the commit that holds it.
- The passing-after run, the nearby suite you reran, and the fix commit.
- If you could not show a failure before the fix, say why, and name the closest check you used instead.
