---
name: blast-radius
description: "Load for \"blast radius of X\", \"what could this break\", or a small diff you don't trust yet. Makes you find what a change breaks outside itself, beyond what grep shows, and prove the one fact it is safe because of by running real code instead of writing it up."
---

# Blast radius

Find what a change breaks somewhere else, before it ships.

The `how` skill tells you what the code does. The `why` skill tells you why it has its shape. This skill tells you what the change breaks elsewhere.

Listing the callers is not the job. `grep` finds those in a second. The job is the breakage `grep` won't show you.

## Don't trust your own writeup

A blast-radius writeup that sounds right is worthless. It reads as convincing whether or not it is true. So don't hand back the writeup alone. Find the one or two facts the whole thing depends on and prove them by running code.

### How sure are you

For each fact the change's safety depends on, get it as far down this list as is cheap, and say where it stopped.

1. You said so. Worthless on its own.
2. You pointed at the line. A real `file:line`, or the dependency's own source.
3. You showed the bad case can't happen. You walked the failure step by step and it doesn't reach.
4. You ran it. A test or script that calls the real code and fails loudly if you're wrong.
5. You reproduced it in the running program.

Step 4 is usually one small test or script that uses the same dependency, at the same version, that the program ships, and calls the exact function you're worried about.

## Steps

1. **Read the change.** Find the commits with `vcs_log` and read each with `vcs_show`. List the symbols it adds, changes and deletes, and what it now does differently, including what the diff does not spell out. For the history behind the touched code, use the `why` skill's code anchor step.
2. **Find the one fact it is safe because of.** Most changes that look risky are safe because of a single fact, such as "this call only drops cache entries that are already dead, and does nothing else". Find that fact. If it holds, most risky cases clear at once. Spend your time here, not on a long list of maybes.
3. **Look where grep stops.** Read the source of the dependency you call, at the version the lockfile pins, and check for local patches or forks. Work out when things run. Follow what a symbol search misses. Check the ones that apply to the repository:
   - **Destructor or cleanup order.** Moving a variable, a guard or a `finally` block changes when cleanup runs. A lock, a file handle, a temporary directory or a channel that now lives longer or shorter changes behaviour with no change at any call site.
   - **Cancellation at a suspension point.** An async task cancelled while it waits stops there. Code after it never runs. A change that adds a wait between two steps that must both happen, or moves one into a race between tasks, can leave state half-updated.
   - **The serialization format other components read.** Renaming a field or variant, changing how names or tags are written, changing defaults or the handling of unknown fields, or reordering fields in a positional format changes bytes that older readers and stored data still expect.
   - **Schema owned elsewhere.** A table or column in a database another component owns, migrates or reads. A rename that builds in your module can break that component's queries at runtime.
   - **Build-time feature flags.** A flag you turn on, or conditional code you move, can change what other modules build. A build with the defaults off, or of one package alone, may no longer work.
   - **Public re-exports across packages.** Removing or changing a re-export breaks every package that imported through it, and `grep` in your package will not find those imports.
   - **Shared files and formats.** Another program, script or language that reads the same file, environment variable, config key, or bytes on the wire.
   - **Ordering and time.** Event order, retries, timeouts, caches that hold the old value, and code three hops downstream that assumed the old order.
4. **Be honest about each risk.** Give it a real chance of happening and a real cost if it does. Keep the risks you confirmed. List the ones you checked and cleared separately. The `why` skill's evidence rules apply: cite a real `file:line`, a search that finds nothing is still an answer, and never make up a caller or an API.
5. **Prove the one fact.** Write a test or script that runs the real code, run it with `exec_command`, and paste what happened. Use the repository's own test command, from its instructions or from looking. If the test belongs in the codebase, commit it. If it is a throwaway probe, delete it when you are done.
6. **For a big or wide change, run it as an arena.** On the main chat, load the `arena` skill in partition mode: one worker per area of risk (for example storage, wire formats, async paths), each told "do not edit files outside your probe path" and to report `PASS` or `ISSUES` with proof. Different briefs find different real bugs. On a chat, take the areas yourself, one after another.

## What to hand back

- **What it does.** What changed, including the part that isn't obvious.
- **The one fact it is safe because of.** State it, say which step of the ladder you got it to, and show the proof. If you couldn't prove it, write "unproven".
- **Risks.** Each names how it breaks, the `file:line`, how likely and how bad, and how to check. Paste the proof for the ones that matter.
- **Cleared.** What you checked and why it's fine.
- **Before you merge.** The cheapest test or reproduction that catches the real bug, including the one you wrote.

Load the `humanizer` skill and apply it to the writeup. Cite real code. Strip anything private before it goes anywhere public.

**Reply:** the writeup above, with the one safety fact either proven or marked unproven.
