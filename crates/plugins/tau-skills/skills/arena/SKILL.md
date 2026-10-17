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

On the main chat, a candidate is a sub-agent you start with `spawn`. Two arguments decide how it runs:

- `land: false` keeps its commits beside yours under its own bookmark instead of landing them on your change. Race candidates always use it. Since their commits never land, candidates cannot clash, and each may edit the real files in place. tau's report turn names each candidate's changes by change id; the last change holds all its files. You read them with `vcs_show {change}` and take files with `vcs_restore {paths, from: <change>}`. Losers never reach your stack or trunk. tau drops kept work at its next start, so take what you need in this session.
- `fresh: true` starts the sub-agent from its task alone, without your conversation. Candidates usually fork (the default), because they need the context you built, and they get their differences from distinct lenses. Spawn a candidate fresh when you want it free of your leaning; its task must then hold the whole brief. A fresh prompt costs more, since nothing of your cache is reused.

Partition workers do real work that should land: spawn them with the defaults, on disjoint file sets, per `principle-separate-before-serializing-shared-state`.

On a chat or a sub-agent, `spawn` is refused. Write the candidates yourself, one after another, each under its own scratch path (for example `prototypes/<task>/a/`, mirroring the repository layout when the task changes existing files) and from its own brief. Write every brief before you write any candidate, and commit each candidate before you start the next. Say in the output that the candidates were written in sequence by one run, so each saw the ones before it. Delete the losers and the scratch folder in your own commit once you have grafted.

## Phase A: Frame

The brief is the contract. Write these down before anything runs.

1. **The artifact** each candidate or worker produces, and the done predicate: what must be true for the arena to be finished.
2. **The mode.** For a race, declare the selection rule now, before spawning: `first pass` (the first candidate that meets the done predicate wins), `rank all` (score and order every candidate), or `best-of` (pick a base and graft). A design bakeoff is `best-of`.
3. **The rubric.** Three to six concrete, gradeable criteria for success on this task. Write it in your notes before you spawn. It goes into the judge's task in Phase D. You may also commit it, but you do not have to.
4. **The briefs.** One per candidate, each with a different lens, so the candidates differ in shape and not only in wording. Lenses that work: the smallest change that solves it; redesign from the data model; the caller's code first; the conventional approach done well; the approach you would not try first. You may also vary `spawn`'s `effort`. For a partition, each brief names its exact slice and file set.
5. **Exact ids and method.** When workers verify or measure commits, each brief names the exact commit ids. A measurement brief also names the method: how many samples, what one sample is, and the order of runs. The worker records both in its report.

## Phase B: Fan out

`spawn` refuses while `@` holds changes, so commit first. Spawn every candidate in one turn, up to 8, race candidates with `land: false`, then end the turn. There is no wait tool. Each report arrives in a later turn. When one arrives and others are still running, note it and end the turn again. Do not edit the files the candidates are working on while they run: `vcs_restore` replaces a file whole, so your edits there would be lost when you graft.

Every brief stands alone. It holds the goal, the scope, its lens or slice, how to verify, and the report contract below. Tell each candidate to commit its work before it ends; an uncommitted candidate leaves nothing to read.

**Report contract.** Each candidate ends with one of:

- `PASS`, with the evidence: what it ran and what it saw.
- `ISSUES`, listing every issue it can prove, not only the first, each with its evidence.
- `BLOCKED`, with what stopped it and what it would need.

A race candidate also gives a short rationale: the alternatives it considered and what it rejected.

## Phase C: Collect

Read each report as it arrives, and note each race candidate's change ids from tau's report turn.

- A report that lacks the commit ids and method its brief named does not count. Spawn that worker again once. After a second miss, record a gap. A gap is not a pass.
- If a candidate produced nothing, go on with N-1 and note the dropout.
- For a partition, the workers' commits land on yours. Run `vcs_status` and resolve any conflict with `vcs_resolve` before you build on them.
- For a partition, every slice needs a result. For a race, apply the selection rule you declared. Do not change it now.

## Phase D: Judge (race only)

On the main chat, spawn one judge once every candidate has reported, with `fresh: true` and `land: false`. Fresh, so it sees no reasoning, leaning or report of yours or the candidates'. Its task holds everything: the task the candidates attempted, the rubric, each candidate's last change id under a neutral label (A, B, C), the instruction to read each with `vcs_show`, to score each criterion with a reason, and to recommend a base. It also says: "Do not edit files. Commit nothing."

While the judge runs, read every candidate end to end yourself with `vcs_show` and score each criterion. Do not score on overall feel.

On a chat, you are the judge. Score each candidate against the rubric, criterion by criterion, and write the scores down before you pick.

Compare your scores with the judge's. Agreement on the base confirms the pick. Disagreement means you are biased or the rubric was ambiguous: read both rationales before you decide.

Pick the base a future maintainer can extend most easily without breaking invariants. When two feel tied, prefer the cleaner boundary or the smaller API, per `principle-laziness-protocol`.

## Phase E: Graft

Take the base's files with `vcs_restore {paths, from: <base's last change>}`. Then walk each losing candidate once more with `vcs_show` for what is worth porting. That is usually one or two things per candidate, not most of it. Fold each graft in by hand so the result stays coherent under one mental model, per `principle-redesign-from-first-principles`. Do not paste mechanically. Commit the result.

If the candidates converged on one shape, that is strong agreement. Note it and keep the shared shape. No graft is needed. If they diverged wildly, Phase A was under-specified. Reframe and run again rather than averaging.

Nothing to clean up: the losers stay aside and tau drops them at its next start. If the person wants a loser kept, restore its files under a path of their choosing and commit them.

## Phase F: Verify

Verify the result like any other output, per `principle-prove-it-works`. If verification finds a problem the arena missed, either Phase A was wrong (reframe and run again) or a candidate caught it and you missed the graft (go back to Phase E, while the candidates are still kept).

## Output

For a race: the synthesized artifact, and a short synthesis note with the base and why, the judge's verdict and where it differed from yours, each graft with its source candidate, what was rejected and why, any convergence, dropouts or gaps, and the verification result.

For a partition: one consolidated report. A table with one row per slice (slice, result, evidence, commit ids and method where the brief named them), one line per issue, then the gaps and dropouts. Do not paste raw worker reports.

State the selection rule in both. Say whether the candidates forked your conversation or started fresh, or were written in sequence by you.
