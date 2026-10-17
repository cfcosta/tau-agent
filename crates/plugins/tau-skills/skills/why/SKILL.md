---
name: why
description: "Load for \"why is X this way\", \"why did we pick Y\", design rationale, regressions, postmortems, or where a number came from. Makes you search the history and each MCP source the run has, then give a cited answer that keeps known, inferred and unknown apart. For behaviour, load `how`."
---

# Why

Find the forces that gave the code its shape. The `how` skill answers what the code does. This skill answers why it is that way.

Work as a careful, cautious and precise investigator. Keep what you know apart from what you infer. Read `references/epistemics.md` from this skill's folder before you write the answer. Its tiers and phrasing rules bind the answer.

Change no files while you do this.

## 1. Name the target and the question

The target is a piece of code, a pattern, a feature, or a named decision. The question is a rationale, a tradeoff, a motivating edge case, an outside constraint, dead code, or a broad history sweep.

If the target is vague, take your best reading from the conversation (recent edits, open files, what was just discussed). State that reading in one line and go on.

## 2. Build the code anchor

Before anyone investigates, collect the seed context yourself:

- The target's files and line ranges, and its key symbols. Find them with `grep` and `read`.
- The commits that touched it. `vcs_log` lists `@` and its ancestors with the first line of each description. `vcs_show {change}` gives one commit's full description and diff. To find the commits that touched a file or added a string, use `codemode`: walk `vcs_log` with a large `limit`, call `vcs_show` on each commit in parallel, and return only the ids and descriptions whose diff touches the path or contains the string.
- Rename history. There is no rename follow. If the file or symbol was renamed, `grep` the commit diffs for the old name too.
- PR numbers (such as `(#1234)`) and ticket ids in commit descriptions.
- PR descriptions and review threads, only if an MCP server in this run exposes them. If none does, record that as a gap.

Line-level blame is not available. Say so when the answer would need it, and use the commits that touched the line's text instead.

The anchor goes to every investigator.

## 3. Investigate every source

Default to the full investigation. The goal is a complete map of what was searched. An empty search is a finding.

**Discover the sources.** Source control is always one. Then list the MCP servers this run has: their tools appear as `mcp__<server>__<tool>`. Classify each server by its name, instructions and tool names. Keep each server that holds tickets or issues, long-form docs, team chat, infrastructure observability, error tracking, or product analytics. Record any server you could not classify.

**One investigator per source.** Each gets `references/investigator-prompt.md` filled in with the anchor and the question, plus one playbook:

- source control: `references/code-archaeology.md`
- each MCP server: `references/mcp-source.md`
- also `references/incident-postmortem.md` when the target code looks defensive (null checks, retries, timeouts, rate limits, feature flags, guards, out-of-memory handling).

**On the main chat**, spawn all investigators in one turn with `spawn {task, fresh: true, land: false}`, then end the turn. Fresh, so an investigator gathers evidence without the person's guessed answer or your leaning; the filled template holds everything it needs. `land: false`, so nothing it touches lands. Every brief keeps the template's "Do not edit files. Commit nothing." line. `spawn` refuses while `@` holds changes: commit a finished unit first, or investigate yourself as below. Reports arrive in later turns. Synthesize only when every investigator has reported or dropped out.

**On a chat or a sub-agent**, investigate each source yourself, one after another. Write each source's report in the template's output sections before you start the next. Use `codemode` for bulk queries.

**Skipping a source** needs a written reason that goes into Sources Consulted. Two reasons are valid. No MCP server covers that kind of source in this run, which is a gap and not a choice. Or the source is provably irrelevant, for example error tracking for a build-time script that never runs in production. "Probably irrelevant" is not enough.

If the target is a single commit whose description already answers the question completely, you may answer from it directly. Say so, and say which searches you judged redundant.

## 4. Synthesize

Write the answer yourself, following `references/synthesizer-prompt.md` and `references/epistemics.md`. The rules that matter most:

- Never cite code as evidence of its own intent. Code is mechanics, not motivation.
- Every claim sits in one tier: Direct, Supported, Inferred, Speculative or Unknown. Its phrasing matches the tier.
- When the record fits more than one story, list the competing hypotheses with the evidence for and against each. Do not force a winner.
- A hypothesis inside the person's question is a candidate. Check it.
- Spot-check every citation you rely on by reading it again.

## 5. Present

Present the answer in the synthesizer's structure. Keep the confidence language as written. A long answer goes on the canvas with `canvas_write`, and the reply gives the confidence summary and points to it.

If the question comes before a change to this code, end with a constraint set for planning that change:

- **Preserve:** behaviour or properties the history says were deliberate.
- **Change:** what the evidence says no longer holds.
- **Avoid:** approaches the history shows were tried and failed, or rejected.
- **Risk:** what could break that the evidence cannot rule out.

Load the `humanizer` skill and apply it to the prose before you send it. Leave the tier labels and hedges as they are.

## Failure modes

- Recency bias. The latest commit is not the whole story. The current shape is often many decisions stacked up. Trace back.
- Rounding up. A partial finding stays partial.
- Filling a gap with a plausible story. Name the gap instead.
