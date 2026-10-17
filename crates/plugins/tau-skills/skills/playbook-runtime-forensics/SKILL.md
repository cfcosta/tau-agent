---
name: playbook-runtime-forensics
description: "Load when a running program misbehaves and the cause is unknown (memory that grows, CPU spinning while idle, a hang, a stall, a glitch that only shows at runtime). Makes you instrument the live process and return a cited diagnosis, not a fix."
---

# Runtime forensics

You own the diagnosis. Instrument the live process. Do not theorize from source. The deliverable is a cited diagnosis. You write no fix unless the person asks.

When the capture already exists and you only have the file (a CPU profile, a trace file, a heap dump, a core dump), load the `playbook-trace-forensics` skill instead.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`. Load each skill a step names when you reach it.

Reversible choices (which profiler, which run to attach to): decide and say why in the reply. Use `ask` only when the next step needs the person, such as attaching to a process they own in production, or a capture that needs root they have not granted. Never end a turn with a question in prose.

Use the profilers, debuggers and tracers the repository or its language already uses. Look in its instructions, build files, scripts and CI first. Keep every artifact outside the repository (for example under `/tmp/<slug>/`). Every file in the working copy is recorded as a change.

## Steps

1. **Reproduce and pick the signal.** Start the program the way its repository's instructions say, in the state that shows the symptom. For a desktop UI, use the repository's `verify-*` skill if it has one, and take screenshots only from a headless compositor, never the person's display. Then capture a real artifact that matches the symptom:
   - CPU spin or slowness: a sampling CPU profile of the live process, with call stacks, and per-thread CPU from the operating system to find the hot thread.
   - Memory growth: a heap profile over time from a heap profiler or the program's allocator, plus the resident memory the operating system reports for the process, sampled at intervals to show the slope.
   - Hang or stall: every thread's stack, from a debugger's thread dump or the runtime's own stack dump. Add the operating system's per-thread state for what each thread waits on, and a system-call tracer attached to the process for the call it sits in.
   - Busy without input: a system-call tracer's counts for the call mix, the program's own logs and spans at a higher verbosity, the runtime's task or event-loop inspector if it has one, and timers or wakeups per second.
   If a tool needs privileges the environment lacks (kernel limits on profiling, or on attaching to another process), say so and use the next tool that works.
2. **Reduce it to the smoking gun.** Find the function on the hot path, the allocation site whose bytes keep growing, the thread and lock it waits on, or the loop that fires with no input. Large artifacts would flood your context. Load the `principle-guard-the-context-window` skill. Process bulk output with a script or `codemode` that returns only the top entries. On the main chat you may `spawn` a sub-agent told "do not edit files" to reduce a large artifact, then end the turn and read its summary when it lands.
3. **Prove the mechanism.** A hypothesis is not a diagnosis until you confirm it cheaply on the running program. Add one log line or tracing span at the suspected site and rerun, count hits with a debugger breakpoint or a dynamic tracing probe on the function, or toggle the suspected input and watch the signal change. Predict the result before you look. Load the `principle-fix-root-causes` skill: follow the symptom back to the first place the system went wrong, not the place it became visible.
4. **Map it to source.** Name the file, the symbol, and the line that allocates, schedules, spins, or blocks. Use `grep` and `read` to land on the exact line. Use `vcs_log` and `vcs_show` if you need to know when it came in. Line-level blame is not available.
5. **Clean up.** Remove the instrumentation you added with `vcs_restore` on those paths. Leave no throwaway edits in tracked files. Stop processes you started.

## Reply

Give the signal you captured and how, the reduced finding, how you proved the mechanism (with the prediction and what you saw), the source location, and the artifact paths. Say what stays unconfirmed. No fix unless asked. Once the cause is known, the fix belongs to the `playbook-bug-fix` skill, or to the `playbook-perf-issue` skill for slowness.

Write short declarative sentences. Label each claim measured, inferred, or guess in the same sentence. Never hand the person a check you could run yourself. Name each principle that changed a decision.
