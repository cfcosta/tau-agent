---
name: interrogate
description: "Load for \"interrogate\", \"adversarial review\", \"challenge this\", \"stress test this\", \"find blind spots\" or \"tear this apart\". Makes you review a commit stack through separate lenses (correctness, structure, verification and security) and sort findings into Act on, Consider, Noted, Dismissed."
---

# Interrogate

Review a set of changes adversarially, through two or three lenses, then judge the findings as a pragmatic lead. The deliverable is a sorted verdict. Do not apply any change yourself, and do not let a reviewer apply one.

## Step 1: Scope

The scope is the commit stack under review.

- If the person names commits, files or a change, use that.
- Otherwise, use `vcs_log` to find the commits of the current work: the rows above the first `(immutable)` one, or the commits this conversation made. Read each with `vcs_show`, and use `vcs_diff {change, paths}` to look at one file in one commit.
- If the work under review is not committed yet, commit it first. Reviewers read commits, and `spawn` refuses while `@` holds changes.

Write down the change ids, oldest first, and the files they touch.

## Step 2: Intent

State the intent in one clear paragraph, from the person's message, the commit descriptions and the code. If two readings of the intent are both plausible and would lead to different reviews, put them to the person with `ask`, your best reading first, labelled "(Recommended)". Otherwise, state your reading and go on. On a sub-agent, which cannot ask, state the reading you chose and why.

## Step 3: Review through the lenses

Read `references/reviewer-prompt.md`, `references/rubric.md` and `references/code-quality-review.md` from this skill's folder. Fill the reviewer template once per lens, with the intent, the change ids and files, the full rubric and the full code quality lens. Use two or three of these lenses:

- **A. Correctness and root cause.** The rubric's Correctness and Root Causes vs. Symptoms sections. Trace execution paths. Ask whether the change fixes the real problem or a symptom.
- **B. Structure, the "code judo" lens.** The code quality lens and the rubric's Structural Integrity and Complexity Budget sections. Look for the restructuring that makes branches, helpers or layers disappear.
- **C. Verification and security.** The rubric's Verification and Security sections. Ask how anyone can tell this works, and trace every untrusted input to its sink.

Use all three for anything beyond a small change. For a small, local fix, A and C are enough.

**On the main chat**, spawn one reviewer per lens with `spawn`, all in one turn, then end the turn. The template opens with "Do not edit files. Commit nothing." Keep it: there is no read-only flag. You may give lens A a higher `effort`. Each review arrives in a later turn. When one arrives and others are still running, note it and end the turn again.

Be honest about what the reviewers are. Each one forks your conversation, so none is blind to the author's view. Diversity comes from the lenses, not from independence. The template tells each reviewer to judge the commits and not the conversation's account of them.

**On a chat or a sub-agent**, `spawn` is refused. Run the lenses yourself, one after another. Finish each lens's findings in the template's output format before you start the next, and do not soften an earlier lens's findings when a later one disagrees. Say in the output that one run reviewed through all lenses.

## Step 4: Synthesize

Parse every review. Merge findings that describe the same issue in different words, and note which lenses raised each one. A finding raised under two or more lenses is the highest signal. Read a finding from one lens on its merits, and weight it as one voice. Note disagreements: when one lens flags something and another says the opposite, that is context for the verdict.

Check every finding you will put under Act on yourself. Read the code it points at and trace the call chain. Run the test or script that would show it, when that is cheap.

## Step 5: Lead judgment

You are the lead reviewer, a pragmatic senior engineer, not a neutral aggregator. Read `references/lead-judgment.md`. Put every finding in one bucket, with the lenses that raised it and a one-line reason.

- **Act on.** Real issues that affect correctness, security or maintainability given the actual goals. They would block a real review.
- **Consider.** Legitimate, but you are not sure they outweigh the cost of fixing them now. Worth the person's attention.
- **Noted.** Technically valid but not actionable: context-dependent, premature optimization, or low impact at this stage.
- **Dismissed.** Wrong, nitpicky, or missing context, with a brief reason.

## Output format

Put a long verdict on the canvas with `canvas_write` and give the Act on list in the reply. A short one goes in the reply.

### Intent
> The intent paragraph from step 2.

### Scope
The change ids reviewed and the files they touch.

### Reviewers
One line per lens: the lens, whether a spawned reviewer or you ran it, and its number of findings.

### Act on
Each: the description, the lenses that raised it, why it matters, and what you checked yourself.

### Consider
Each: the description, the lenses that raised it, and the tradeoff.

### Noted
A brief list.

### Dismissed
Each with a brief reason.

### Agreement map
Where the lenses agreed and where they diverged, and what that pattern says about the change. Name any area no lens covered.

After the verdict, stop. Fixing the Act on items is a separate task the person starts, or the next step of the playbook that called this skill.
