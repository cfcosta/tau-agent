---
name: playbook-hillclimb
description: "Load when asked to improve one measurable thing against a target over many attempts (latency, throughput, memory, binary size, a score). Makes you freeze a committed harness, then loop one change, one measurement, keep or revert, with one commit per win and a decision table on the canvas."
---

# Hillclimb

You own the metric and the integrity of the experiment. This is a loop. A one-off fix belongs to the `playbook-perf-issue` or `playbook-bug-fix` skill.

Core discipline: one change, one measurement, keep or revert. Never stack untested changes. Never claim a win from reading the code.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`. Load each skill a step names when you reach it.

Reversible choices (the next hypothesis, a workload the person did not give): decide, proceed, and record the choice in the decision table. Use `ask` only for a call the person must make, such as a tradeoff against behaviour they care about, with the recommended option first. Never end a turn with a question in prose.

The tools are the repository's. Look in its instructions, build files, scripts and CI for its optimized build, its benchmark runner and its profiler, and use those. You need an optimized build with production flags, a benchmark runner (or a small loop) that interleaves runs, a CPU profiler, a hardware counter reader for instruction counts (they are less noisy than wall time), and a system-call tracer. Keep raw output outside the repository, since every file in the working copy is recorded as a change.

## Steps

1. **Ground the workload.** Load the `how` skill on the target. Name the workload dimensions that move the result (data size, history, state, concurrency) and pick a case that reproduces the complaint. If none does, fix the repro first. Then fix one metric, its better direction, and a stop condition that pairs a target with a floor on attempts, such as "p50 at least 40% below baseline and at least 10 attempts", so a lucky early win cannot end the run. Use the person's numbers when given. Otherwise pick sensible ones and say so.
2. **Build the harness, prove it, freeze it.** Load the `principle-build-the-lever` skill. One command emits the metric as the median of N runs, the range, an error count, and a count of the work done. Run contrasting workloads and confirm the target case shows the symptom while easier cases separate. If the harness cannot tell them apart, change the workload or the metric. Vet it with the measurement checks below. Then commit the harness on its own with `vcs_commit` (for example `test(bench): frozen harness for index p50`). After this commit, do not edit it. A harness change restarts the baseline.
3. **Record the baseline.** Run the frozen harness and the regression gate (the tests that must stay green). Create the decision table with `canvas_write`, titled after the task, with the columns `id | hypothesis | change | before | after | delta | gate | verdict | commit`. Put the baseline in its first row.
4. **Ground each hypothesis.** Name a specific mechanism from step 1 ("defer the config parse off the startup path because it blocks the first frame"), never "try memoizing something". For a performance metric, order ideas by the mantras, cheapest first: don't do it; do it but don't do it again; do it less; do it later; do it when they're not looking; do it concurrently; do it cheaper. Read the table before each attempt so you never retry a loser.
5. **Loop, one hypothesis per iteration.**
   - Make the change. Keep a code comment only for a non-obvious why.
   - Run the frozen harness against the last kept state, interleaved (keep a copy of that build outside the repository), and run the regression gate.
   - Keep the change only when the metric moves past the run-to-run range, the error count stays zero, the work count matches, and the gate stays green. Then `vcs_commit` that change alone, with the number in the message (`perf(index): reuse the path buffer (p50 52 ms to 47 ms)`).
   - Otherwise revert it in full. Uncommitted, use `vcs_restore` on the paths you touched. Already committed, use `vcs_undo`. A tweak that "might help" is not kept.
   - Append the row to the table with `canvas_edit`, kept or reverted.
   Load the `principle-sequence-verifiable-units` skill: each iteration ends in its check before the next begins.
6. **Delegate with care.** On the main chat you may hand attempts to sub-agents. Commit first, brief each one with one hypothesis, the harness command, and the keep rule, `spawn`, then end the turn. A sub-agent forks your conversation, and its commits land on top of yours when it ends. Review each diff with `vcs_show`. Parallel attempts share the machine, so they disturb each other's timings. Fan out in parallel only for metrics that load does not move (binary size, allocation counts, instruction counts). For wall time, run one attempt at a time, and re-measure every landed win yourself. Load the `principle-separate-before-serializing-shared-state` skill when attempts could touch the same files. On a chat or a sub-agent, do the attempts yourself.
7. **Push past the first plateau.** After several rejects in a row, change category, combine near misses, re-read the source, or try something more radical. Correctness and simplicity outrank the number. Revert a win that breaks behaviour. Keep a simplification that holds the number (load the `principle-laziness-protocol` skill).
8. **Stop.** Stop when the condition holds, or when the remaining ideas cost more than they could give. Never relax the condition to meet it. Never quit while cheap untried ideas remain. If you are stuck, say so with the evidence. For an unattended run, load the `playbook-autonomous-run` skill for wake-ups only; its stop rule does not replace this one.
9. **Hand over.** The accepted commits stand in the order they landed. On a chat, `vcs_land` last, with everything committed.

## Measurement checks

Apply them to the harness before you freeze it, and to any number you report.

- Write the claim you expect to make before you run.
- Check the machine's load average and core count. If you cannot stop the load, interleave sides and say so.
- Run release builds with production flags and data. An untuned side compares configurations, not code.
- At least 5 runs per side, interleaved A, B, A, B. Median and range.
- Profile in a run you do not report, and name the limiter. If a change did not move the number, the limiter explains why.
- Count errors and check outputs. Confirm the work happened inside the timed region.
- Do the arithmetic: removing 10% of the work saves at most about 11%. A result past a limit measured something else.
- Check it matters end to end, and report a micro result as a share of the whole.

## Reply

Lead with the verdict on the stop condition. Then the metric and target, baseline to final with the percent delta, attempts run (kept and reverted), each accepted commit on one line, the limiter now, the canvas title of the decision table, and the best idea you would try next.

Write short declarative sentences. Label each claim measured, inferred, or guess. Never hand the person a check you could run yourself. Name each principle that changed a decision.
