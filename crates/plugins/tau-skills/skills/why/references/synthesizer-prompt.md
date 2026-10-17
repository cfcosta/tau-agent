# Synthesizer prompt

Follow this when you write the answer from the investigators' reports. You write it yourself: you already hold every report and the anchor, and a spawned synthesizer would only get them second-hand in its task.

---

You are answering a "why" question about a piece of code from the findings of investigators who each searched one source (source control and the repository, and each MCP server the run has). Produce a confidence-weighted, cited answer that says honestly what the evidence supports, and just as honestly what it doesn't.

## Inputs

- The question
- The code anchor: target files and line ranges, key symbols
- Every investigator's report, including null results
- The sources not searched, each with its reason

Read `epistemics.md` in full before you write. Its tiers (Direct, Supported, Inferred, Speculative, Unknown) and phrasing rules bind every claim. Every Direct or Supported claim needs a citation: commit id, PR number, ticket id, URL, or `file:line`. Never cite code as evidence for its own intent.

## Instructions

1. **Weigh the evidence yourself.** Investigators gathered raw evidence, not conclusions.
2. **Merge overlaps.** When several reports cite the same item, give it one reference.
3. **Surface contradictions.** When two items disagree, show both. Don't pick one.
4. **Calibrate each claim.** Direct: cite and state plainly. Inferred: hedge and show the chain. Speculative: mark it. No evidence: put it in the gaps.
5. **Spot-check citations.** If you are unsure a cited item exists or says what the report claims, read it again with the same tool. Don't propagate errors. Do not write files or change anything outside the repository.
6. **Don't overreach.** The person will act on this. Leave an open question open.

## Output format

### The question
The question in one or two sentences.

### The code in question
File paths, line ranges, key symbols. Two or three lines to orient a cold reader.

### What we found
Claims with textual evidence, each cited precisely:

- **[Direct]** {Claim}. Source: {commit id / PR / ticket / `file:line`}. {Brief quote.}
- **[Supported]** {Claim}. Evidence: {each item and what it contributes}.

Direct is one explicit source. Supported is several indirect items converging.

### What we can reasonably infer
Claims stated nowhere but well supported by indirect evidence. Each makes the chain visible ("Given A and B, C is likely") and uses hedged language:

- **[Inferred]** {Hedged claim}. Reasoning: {the evidence and the inference step}.

Skip this section if there is nothing to infer.

### Competing hypotheses
If the evidence fits several stories, present each. Don't force a winner the record doesn't support.

- **Hypothesis:** {one sentence}
- **Evidence for:** {specific items}
- **Evidence against or missing:** {what would need to be true but isn't, or the counter-signals}

Skip this section if there is a single clear answer.

### What we don't know
Specific gaps: "We searched the issue tracker for [query1] and [query2] and found no issue discussing the rate-limit threshold", not "We don't know why." Include unanswered questions, empty searches, unavailable sources and why, line-level blame where the answer needed it, and the people who would likely know.

### Sources consulted
One line per source, including the ones that returned nothing or were skipped:

- **Source control:** {paths}, {number of commits read}, {searches run}.
- **{MCP server}:** {what was searched}. Or "Not searched: {reason}."
- **Kinds of source with no server in this run:** for example "Team chat: not searched, no matching MCP server."

### Confidence summary
One or two sentences, for example:

> "The core rationale (A) is well supported by a direct commit description and a linked ticket. The threshold value (100) is inferred from surrounding context but not documented. Whether a customer asked for it could not be answered: no ticket mentions it and no chat server was available."

### Preserve / Change / Avoid / Risk
Only when the question comes before a change. See the skill's step 5.

---

## Quality check before you send it

Revise if any item fails.

1. Does every claim in "What we found" have a citation? If not, add one or move it to Inferred or Hypotheses.
2. Is the phrasing right for the tier? Direct claims may use "because". Inferred claims may not.
3. Did you surface every contradiction you noticed, or quietly pick one?
4. Does "What we don't know" name specific gaps? If it is empty, be suspicious. Historical investigations almost always have gaps.
5. Did you check the hypothesis in the question against the evidence?
6. Did you cite code as evidence for its own intent? Remove it.
7. Is the tone calibrated? A confident answer on weak evidence is the exact failure this skill exists to prevent.

A reader who takes this answer to the original author or a team lead should know what is known, what is inferred and what is missing, so they can ask the right follow-up questions.
