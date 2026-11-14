---
name: playbook-perf-issue
description: "Load when something is measurably slow or heavy and you must fix it once (a slow command, request, build, startup, a memory or CPU regression). Makes you measure a vetted baseline, profile for the limiter, fix the cheapest cause, and prove the delta."
---

# Perf issue

You own the measurement story. Tie every fix to a measurement. Do not read source in place of measuring, and do not claim a ceiling you have not run.

For sustained work on one metric against a target, with many keep-or-revert attempts, load the `playbook-hillclimb` skill instead. When the cause is unknown and the process must be watched live, load the `playbook-runtime-forensics` skill first and come back with the diagnosis.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`. Load each skill a step names when you reach it.

Reversible choices (the workload, the tool, the fix): decide, proceed, and say in the reply what you assumed and why. Use `ask` only for a call the person must make, such as a tradeoff between speed and a behaviour they care about. Never end a turn with a question in prose.

The tools are the repository's. Its instructions name the build and test commands. Look in them, its build files, scripts and CI for the profiler and benchmark runner it already uses, and use those. You need an optimized build with production flags, a benchmark runner (or a small loop that interleaves runs), a CPU profiler, a heap profiler for memory, a system-call tracer, and the operating system's report of a process's CPU time and peak memory. Keep profiles and logs outside the repository (for example under `/tmp/<slug>/`). Every file in the working copy is recorded as a change.

## Steps

1. **Write the claim.** Name the slow path, the case that reproduces the complaint, and the sentence you expect to ship ("`index` is 30% faster at p50 on the 60k-file repo"). Name the workload dimensions that move the number: data size, history, cache state, concurrency. If no case reproduces the complaint, fix the repro first. Load the `principle-explain-the-number` skill.
2. **Measure the baseline.** Follow the measurement procedure below. Save the baseline binary or build outside the repository so you can run old and new side by side later.
3. **Find the limiter.** Profile in a separate run you do not report, because profilers slow the work. Use CPU per process and I/O wait from the operating system, a sampling CPU profiler, and system-call counts from a system-call tracer. Map the hot spot to file and symbol. Check the load generator too: if it saturates first, you measured it. Load the `how` skill to ground hypotheses in how the code really works.
4. **Pick the cheapest fix.** Try the performance mantras in order and stop at the first one that meets the target:
   1. Don't do it. Stop work whose result nothing uses.
   2. Do it, but don't do it again.
   3. Do it less.
   4. Do it later.
   5. Do it when they're not looking.
   6. Do it concurrently.
   7. Do it cheaper.
5. **Plan and implement.** If the fix crosses a function boundary, load the `architect` skill first. Load the `principle-sequence-verifiable-units` skill: one attempt, one measurement, then the next. On the main chat you may delegate: commit first, `spawn` with file pointers and the whole scope (the claim, the limiter, the chosen mantra, every later instruction from the person), then end the turn. The sub-agent forks your conversation and its commits land on top of yours. Review its diff with `vcs_show` and write your own summary. On a chat or a sub-agent, implement it yourself. Keep a code comment only for a non-obvious why.
6. **Measure after.** Run the same procedure with old and new interleaved. Profile again and confirm the limiter moved. Compare the artifacts, not your memory of them: the profiler's own diff, two flame graphs side by side, or both profiles dumped to sqlite and queried. A result that is inconclusive, or measured on the wrong path, is not a pass. Say so.
7. **Prove behaviour held.** Run the repository's tests. Load the `principle-prove-it-works` skill.
8. **Commit.** `vcs_commit` with a Conventional Commits message that carries the primary number, for example `perf(index): skip rehashing unchanged files (p50 41 ms to 33 ms)`. If the measurement script is worth rerunning, commit it too (load the `principle-build-the-lever` skill). On a chat, `vcs_land` last, with everything committed.

## Measurement procedure

Use it for the baseline and for every number after it.

- Read the measurement script. Note what it times, counts, and ignores.
- Check the machine's load average and core count. If the machine is busy and you cannot stop the load, interleave the sides so both see the same noise, and say so.
- Run each side the way production runs it: release build, production flags, the same data, caches as warm or cold as users see them. An untuned side means you compared configurations, not code.
- Run each side at least 5 times, alternating A, B, A, B. Report the median and the range. Many benchmark runners run each command in one block, so write a small loop when interleaving matters.
- Count errors and check outputs are correct. Fast failures and skipped work make fake wins. Confirm the work ran inside the timed region: rows written, bytes read, result used.
- Do the arithmetic. Removing a piece that takes 10% of the run makes the run at most about 11% faster. A result past that, or past disk, network, or core limits, measured something else.
- Measure the end-to-end path a user waits on next to any micro result, and give the micro result as a share of the whole.
- A gap smaller than the run-to-run range is no measurable difference.

## Reply

Lead with the verdict: faster, slower, no measurable difference, or inconclusive. Then the baseline and post-fix numbers with units, the delta, the run count and range, the limiter, the commit, and the artifact paths. Example: "p50 41 ms to 33 ms, median of 7 runs per side, range 32 to 35 ms after, bound by JSON parsing on one core." Call it inconclusive when you cannot name the limiter, a side ran untuned, or you could not check errors and that the work happened. Name the gap.

Write short declarative sentences. Label each claim measured, inferred, or guess in the same sentence. Never hand the person a check you could run yourself. Name each principle that changed a decision and the choice it changed.
