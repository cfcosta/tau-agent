---
name: playbook-trace-forensics
description: "Load when someone hands you a captured profiling artifact to explain (a CPU profile, a trace JSON, a heap profile, a core dump, a stack dump). Makes you load it into a queryable form such as sqlite, find the cause, and return a cited diagnosis."
---

# Trace forensics

You own the diagnosis from the artifact. The capture already exists. Treat it as a fixed dataset: read it, do not re-run it. When you need to instrument a live process instead, load the `playbook-runtime-forensics` skill.

## How to run this playbook

Copy the numbered steps into your working notes as a checklist before you start. Tick each one off. A step you skip stays in the list as `skip: <reason>`. Load each skill a step names when you reach it.

Reversible choices (which tool, which schema): decide and say why. Use `ask` only for something only the person has, such as the matching build with symbols or a second capture. Never end a turn with a question in prose.

Keep extracted files and databases outside the repository (for example under `/tmp/<slug>/`). Every file in the working copy is recorded as a change.

## Steps

1. **Identify the format and load it.** Find which tool wrote the artifact: its file name and extension, its first bytes, and the repository's scripts or CI that produce captures. Then use that tool's own export or report command to read it as text or JSON.
   - A binary CPU or heap profile: the profiler's text report or raw sample export.
   - A trace or profile in JSON: plain JSON, often gzipped.
   - A core dump: open it in a debugger with the matching binary and ask for every thread's backtrace.
   - A stack or spin dump in text: read it directly.
   Large artifacts would flood your context. Load the `principle-guard-the-context-window` skill. Never `read` a large artifact whole.
2. **Make it queryable.** Load the `principle-build-the-lever` skill. Write a small script that dumps the artifact into sqlite, one row per sample, frame, allocation, or node, with the columns you will join on (sample id, stack depth, function, file, line, self and total weight, thread). Use the `sqlite3` CLI with `json_each`, or a short script in a language the machine already has. Reach this shape before you start reading for the cause. Run the bulk work through `exec_command` or `codemode` so only small results come back. On the main chat you may `spawn` a sub-agent with `land: false` to build and query the database, told "do not edit files" and to keep the database and its script outside the repository. Then end the turn and read its findings when it reports.
3. **Narrow to the cause.** Query for the frames with the most self and total time, then walk the call tree down the hot path. For a leak, group live allocations by site and follow the retainer chain to a root. For a hang or a core, find the thread on CPU or blocked, and its wait reason. Check that the capture covers the symptom's time window and the right process.
4. **Attribute to source.** Map the hot frame to file, symbol, and line using the artifact's own symbols. A frame with no source mapping is not yet a diagnosis. Resolve the symbols (the matching binary, debug info, a symbol server), or say plainly that the artifact does not carry them. Use `grep` and `read` to land on the line.
5. **Confirm with a pair if you have one.** If there is a before and after capture, load both into the same schema and diff them with a query. Otherwise mark the finding as the strongest hypothesis the artifact supports, not a confirmed cause. Load the `principle-fix-root-causes` skill: name the first place the system went wrong, not the frame where the cost showed up.
6. **Hand back a cited diagnosis.** No fix unless asked. Keep the script if the repository will see this format again; committing it is a reversible choice, so make it and say so.

## Reply

Give the artifact and its format, the tool and schema you used, the reduced finding with its numbers, the source location, the artifact and database paths, and whether a paired capture confirmed it. Once the cause is known, the fix belongs to the `playbook-bug-fix` skill, or to the `playbook-perf-issue` skill for slowness.

Write short declarative sentences. Label each claim measured, inferred, or guess in the same sentence. Never hand the person a check you could run yourself. Name each principle that changed a decision.
