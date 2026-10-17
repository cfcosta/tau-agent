---
name: arena
description: "Load when one attempt at a non-trivial artifact could lock in the wrong shape, for \"arena this\", or when work splits into parallel slices or a race. Makes you fan out candidates with distinct briefs, judge them on a rubric written first, keep one base, graft the best parts, and verify."
---

# Arena

Run several attempts at one task, or split it into slices, then bring the results back into one artifact and one report. Two modes, which you can mix:

- **Race.** N candidates attempt the same task. You pick a base and graft the strongest parts of the others into it. This is the default.
- **Partition.** The work splits into independent slices, one worker per slice, and you consolidate their results.

Write the phases below into your notes as a numbered checklist: Frame, Fan out, Collect, Judge, Graft, Verify. Tick each off as you finish it.

## What a candidate is in tau

On the main chat, a candidate is a sub-agent you start with `spawn`. A spawned sub-agent forks your conversation: it sees everything you saw, including your leanings and the rubric. Candidates are not independent, and neither is a spawned judge. Reduce the bias two ways: give each candidate a distinct brief and lens, and have the judge score against a rubric written before any candidate ran, so the pick cannot drift toward whichever result looks best.

Each candidate writes only under its own path, for example `prototypes/<task>/a/`, `prototypes/<task>/b/`, `prototypes/<task>/c/`. That keeps candidates from clashing. When the task is a change to existing files, each candidate writes its version of those files under its own path, mirroring the repository layout, and you move the winner into place.

On a chat or a sub-agent, `spawn` is refused. Write the candidates yourself, one after another, each under its own path and from its own brief. Write every brief before you write any candidate, and commit each candidate before you start the next. Say in the output that the candidates were written in sequence by one run, so each saw the ones before it.

## Phase A: Frame

The brief is the contract. Write these down before anything runs.

1. **The artifact** each candidate or worker produces, and the done predicate: what must be true for the arena to be finished.
2. **The mode.** For a race, declare the selection rule now, before spawning: `first pass` (the first candidate that meets the done predicate wins), `rank all` (score and order every candidate), or `best-of` (pick a base and graft). A design bakeoff is `best-of`.
3. **The rubric.** Three to six concrete, gradeable criteria for success on this task. Write it to `prototypes/<task>/rubric.md` and commit it before you spawn. It is the judge's tool in Phase D.
4. **The briefs.** One per candidate, each with a different lens, so the candidates differ in shape and not only in wording. Lenses that work: the smallest change that solves it; redesign from the data model; the caller's code first; the conventional approach done well; the approach you would not try first. You may also vary `spawn`'s `effort`. For a partition, each brief names its exact slice.
5. **The paths.** Each candidate's own output path, per `principle-separate-before-serializing-shared-state`.
6. **Exact ids and method.** When workers verify or measure commits, each brief names the exact commit ids. A measurement brief also names the method: how many samples, what one sample is, and the order of runs. The worker records both in its report.

## Phase B: Fan out

`spawn` refuses while `@` holds changes, so commit first. Spawn every candidate in one turn, up to 8, then end the turn. There is no wait tool. Each candidate's commits land on your change when it ends, and its report arrives in a later turn. When one arrives and others are still running, note it and end the turn again.

Every brief stands alone. It holds the goal, the scope, its lens or slice, its output path, the rule "write nothing outside your path", how to verify, and the report contract below. Tell each candidate to commit its work under its path before it ends.

**Report contract.** Each candidate ends with one of:

- `PASS`, with the evidence: what it ran and what it saw.
- `ISSUES`, listing every issue it can prove, not only the first, each with its evidence.
- `BLOCKED`, with what stopped it and what it would need.

A race candidate also gives a short rationale: the alternatives it considered and what it rejected.

## Phase C: Collect

Read each report as it lands.

- A report that lacks the commit ids and method its brief named does not count. Spawn that worker again once. After a second miss, record a gap. A gap is not a pass.
- If a candidate produced nothing, go on with N-1 and note the dropout.
- If a candidate wrote outside its path, its changes may clash with others and land as conflicts. Resolve them with `vcs_resolve`, and note it.
- For a partition, every slice needs a result. For a race, apply the selection rule you declared. Do not change it now.

## Phase D: Judge (race only)

On the main chat, spawn one judge once every candidate has landed. Its brief: "Do not edit files. Commit nothing." It reads `rubric.md` and each candidate by its path label, scores each criterion, and recommends a base with reasons. Spawn the judge before you form your own view, so its fork does not carry your scores. Then read every candidate end to end yourself and score each criterion. Do not score on overall feel.

On a chat, you are the judge. Score each candidate against the rubric, criterion by criterion, and write the scores down before you pick.

Compare your scores with the judge's. Agreement on the base confirms the pick. Disagreement means one of you is biased or the rubric was ambiguous: read both rationales before you decide.

Pick the base a future maintainer can extend most easily without breaking invariants. When two feel tied, prefer the cleaner boundary or the smaller API, per `principle-laziness-protocol`.

## Phase E: Graft and clean up

Walk each losing candidate once more for what is worth porting. That is usually one or two things per candidate, not most of it. Fold each graft in by hand so the result stays coherent under one mental model, per `principle-redesign-from-first-principles`. Do not paste mechanically.

If the candidates converged on one shape, that is strong agreement. Note it and keep the shared shape. No graft is needed. If they diverged wildly, Phase A was under-specified. Reframe and run again rather than averaging.

Move the result into its real place. Delete the losing candidates and the `prototypes/<task>/` folder in your own commit, unless the person asked to keep them. Their commits stay in history.

## Phase F: Verify

Verify the result like any other output, per `principle-prove-it-works`. If verification finds a problem the arena missed, either Phase A was wrong (reframe and run again) or a candidate caught it and you missed the graft (go back to Phase E).

## Output

For a race: the synthesized artifact, and a short synthesis note with the base and why, the judge's verdict and where it differed from yours, each graft with its source candidate, what was rejected and why, any convergence, dropouts or gaps, and the verification result.

For a partition: one consolidated report. A table with one row per slice (slice, result, evidence, commit ids and method where the brief named them), one line per issue, then the gaps and dropouts. Do not paste raw worker reports.

State the selection rule in both, and say plainly that candidates and judge were forks of your conversation, or written in sequence by you.
