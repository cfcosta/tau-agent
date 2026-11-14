---
name: principle-explain-the-number
description: "Load before you trust, report, or act on a number you measured, such as a speedup, a regression, a throughput, a latency, or an eval result. Name what limits it, rule out that it measured something other than the work, and lead with a verdict."
---

# Explain the Number

A measured number is a claim about what the code did. Before you report or act on it, know what bounds it and rule out that it measured something else.

**Why:** Benchmarks lie quietly. A cache, a skipped step, an error path, a debug build, or a busy machine can each produce a large, clean-looking number. A wrong number sends the next decision the wrong way.

- **Ask "why not double?"** Name the resource or code path that bounds the result, such as a core, a lock, the disk, the network, or the load generator itself. Get it from a profile or from system counters taken during a run, then map it to source. A guess from reading the code is not a limiter. If you cannot say why the number is not twice as good, you do not know what you measured.
- **List what else the number could be measuring, and rule out each one with evidence.** The usual suspects are errors, skipped or cached work, code that never ran, an untuned side (one left on default settings or a debug build), noise, and a piece too small to matter end to end.
- **Keep the evidence with the number.** Put the run count, the spread, and the limiter in your reply, the commit message, or the canvas.

## For a performance number

1. Write down the exact claim you expect to make, in the words you would ship ("export is 30% faster at p50 on the 60k-row dataset"). The steps below test that sentence.
2. Read the measurement script. Note what it times, counts, and ignores.
3. Check the machine's load average and core count before you run. If something else is busy and you cannot stop it, interleave the sides so both see the same noise, and say so.
4. Run every side the way production runs it: release builds, production flags, the same data and versions. A side on defaults compares configurations, not code.
5. Run each side at least 5 times, interleaved (A, B, A, B, and so on). Report the median and the range. A gap smaller than the run-to-run spread is no measurable difference.
6. Name the limiter from a profile taken in a separate run you do not report, because profilers slow the work down. Use the CPU profiler the repository or its language already uses (look in its instructions, build files and CI first), and a system-call tracer to count system calls.
7. Check that the work happened and did not error. Count failures and check outputs are correct. Confirm the timed region did the work, and that lazy code or a timeout did not skip it.
8. Check that it matters end to end. Measure the path a user waits on and report the micro result as a share of it. A piece that takes 10% of a run can make the run at most about 11% faster.
9. Lead the report with a verdict: faster, slower, no measurable difference, or inconclusive. Then give one number with its unit, the run count, the range, and the limiter ("p50 41 ms to 33 ms, median of 7 runs per side, range 32 to 35 ms after, bound by JSON parsing on one core"). Call it inconclusive when you cannot name the limiter, a side ran untuned, or you could not check step 7.

For a quick ballpark the person asked for, one run is enough. Still do step 7, and say it was one run.

## For an eval result

Ask the same of the trials. Did every run do the task? Does the gap hold across trials? Does the scenario matter to a real user? The `playbook-eval` skill runs this.

You skipped this when the evidence behind a number has no run count, no spread, or no named limiter, or when the time saved is larger than the time the changed piece took.

Distinct from `principle-prove-it-works`, which checks that an output is real. This principle checks that a real number means what you claim.
