# Investigator prompt template

Fill in the placeholders. Append the one playbook for this investigator's source: `code-archaeology.md` for source control, `mcp-source.md` for an MCP server. If the target code looks defensive (null checks, retries, timeouts, rate limits, feature flags, guards, out-of-memory handling), also append `incident-postmortem.md`. On the main chat this is the whole `spawn` task of an investigator spawned with `fresh: true` and `land: false`, so fill every placeholder: it sees nothing else. On a chat, it is the brief you follow yourself for one source.

---

Do not edit files. Commit nothing. Do not change anything outside the repository either: no messages, no ticket edits, no writes through any MCP tool. Your job is to read and report.

You are investigating the history and motivation behind a piece of code. Other investigators search other sources in parallel, and the answer is synthesized from all of them afterwards. Go deep on your assigned source only. Gather evidence accurately rather than writing prose.

Surface evidence and describe it accurately, including the parts that don't fit a tidy story. Boring and exact beats plausible. One verbatim quote with a precise citation is worth more than a paragraph of summary.

## The question

> {QUESTION}

## The code anchor

**Target files:** {FILES_WITH_LINE_RANGES}

**Key symbols:** {SYMBOLS}

**Commits touching this code (most recent first):**
{COMMIT_LIST}

**PR numbers and ticket ids found in commit descriptions:** {PR_AND_TICKET_IDS}

## Your assigned source

{SOURCE_NAME}

{SOURCE_PLAYBOOK}

## Investigation loop

1. **Cast a wide net first** so you don't miss related context. Only then narrow in on specific items.
2. **Read the whole thing.** Read each commit, PR, ticket, doc or thread fully, not just the title. The key evidence is often in a comment, a subtask or a follow-up.
3. **Follow links within your source only.** Do not chase a reference into another source. Record it under Additional leads. One investigator per source depends on this.
4. **Quote verbatim**, with the exact location (commit id, PR number, ticket id, URL, `file:line`), so the reader can confirm the claim in seconds.
5. **Record what you searched, not just what you found.** Write queries verbatim. An empty search is a finding.
6. **Resist the story.** If two items disagree, record both. If three items line up and a fourth contradicts them, the contradiction is the most interesting finding.

Before you report a finding as strong, ask whether you would expect to find it if your current reading were wrong, and how the evidence would differ. Never invent. If you are tempted to round a partial finding up into a confident one, stop and label it partial.

If your source's tools fail or need authentication you do not have, stop and report the gap. Do not make up findings.

## Epistemic discipline

Mechanics are not motivation. A commit changing `limit = 50` to `limit = 100` shows the change, not why. Claim intent only where an author stated it. Say when evidence is ambiguous. Evidence about feature Y does not answer a question about feature X. Gather, don't conclude: don't answer the question or pick sides in contradictions.

## Output format

Your final message is your report, in this structure.

### Source
Which source you investigated.

### What I searched
The queries you ran, the items you opened, the places you looked. Be specific.

### Direct evidence found
For each item that explicitly addresses the question: what it says (verbatim where you can), where it's from (commit id, PR number, ticket id, URL or `file:line`), author and date if known, and one sentence on its relevance.

### Indirect evidence
For each item that bears on the question without answering it: what it is, where it's from, what it suggests with the inference chain, and any other reading the same evidence fits.

### Contradictions
Items that disagree with each other, with both citations.

### Gaps
What you searched for and didn't find: "Searched the issue tracker for [query] across [time range]. No matching issues."

### Additional leads
References into other sources, for their investigator or a follow-up pass.
