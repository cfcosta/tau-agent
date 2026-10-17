---
name: principle-make-operations-idempotent
description: "Load when designing commands, startup or lifecycle steps, queues, retries, or processing loops that can crash, restart, or run twice. Make each one converge to the same end state no matter how many times it runs or where the last run stopped."
---

# Make Operations Idempotent

Design operations so they converge to the correct state however many times they run and wherever the previous run stopped. Every operation that changes state must answer two questions. What happens if this runs twice? What happens if the previous run crashed halfway?

**Why:** Commands, lifecycle steps, and processing loops run where crashes, restarts, and retries are normal. If leftover state changes the next run's outcome, every restart becomes a debugging session.

**The idea:** derive each step from the state that exists, not from what the previous run meant to do. Read the current state, compare it to the goal, and apply only the difference. A step written this way is safe to repeat, because a repeat finds nothing left to do.

Anything a run leaves behind, such as a lock, a temporary file, a half-written record, or a claimed job, must be recognizable as stale by the next run. Judge staleness by evidence that stays true wherever the previous run could have stopped, not only in the common case. A marker the next run cannot judge turns every crash into manual cleanup. Write to a temporary path and rename it into place. Compare artifacts by content, not by creation time or order.

**Examples:**
- A program that installs files on every start rewrites each one from its source, or compares content and writes when it differs. A "skip if the marker file exists" check says an install once began. It says nothing about whether the files are right now.
- A queue that resumes after a crash (a landing queue, a batch of migrations) checks each item against the real state: is the commit already on trunk, is the row already there. It does not trust a "next index" saved before the crash.
- A wake-up, webhook, or retry that fires twice finds the work done and does nothing. An outward effect that cannot be read back, such as a sent message or a charge, carries an idempotency key the receiver checks.

**The test:**
1. What happens if this runs twice in a row?
2. What happens if the previous run crashed at every possible point?
3. Does running it again converge to the same end state?

If any answer is "it depends on what state was left behind", the operation needs a reconciliation step.

You skipped this when recovery from a crash needs a person to delete a file, reset a flag, or skip an item by hand.

Distinct from `principle-separate-before-serializing-shared-state`, which handles several actors writing at the same time. This principle handles one operation repeated over time. When something breaks only after a restart, the `principle-fix-root-causes` skill says to suspect persisted state first. This principle is how you design so that state cannot mislead the next run.
