# 0062: memory keeps the attempts on a problem, and the model they came from

- Status: accepted.
- Date: 2026-11-19

## Context

The notes heiko-memory kept were mostly the story of runs. Of the 63
notes left from before the heiko rename, 56 were `case`s and 59 came from
the pass after a run: "the investigation was started but yielded no
findings", "the pin landed and the check passed", three notes on one
pin, four on one flake input. The consolidation prompt asked for "the
case itself (task, approach, outcome) when the task is likely to
recur", and the faithfulness rules said to report outcomes as far as
they went, so a run that found nothing became a note that said so. Only
the prompt at compaction left out what git history already records. The
few notes worth having, such as Luau's UI registration capping
contributions at eight, were filed among them.

"Self-Improvement Memory Enables Continual Recursive Self-Improvement"
(arXiv 2611.18856) studies memory for the agent that improves a harness,
across a change of benchmark or of policy model. It groups what was
tried by the problem it addresses, and keeps for each attempt the change
and why, the outcome predicted and the one observed, and the domain,
benchmark and model it ran under; failed attempts included. Against the
same evolved harness with no memory, the full record finished higher in
all ten settings, and the root causes alone were mixed: up to 1.82
points worse than nothing. A harness evolved with one model started
below the seed harness on another in every domain; the record of why
each edit was made is what let the optimizer revise them. Its evidence
is for the optimizer's memory, not a task agent's, with one run per
condition outside coding.

## Decision

- **A problem keeps its attempts.** `gotcha` and `case` notes have
  `[[attempts]]`: `at`, `what` was tried, its `outcome` (`worked`,
  `failed` or `untested`), the `evidence`, and the `run` and `model`. The
  tool `memory_attempt { id, what, outcome, evidence, settles? }` appends
  one, or settles an untested one once, by its number. Updating a note
  keeps its attempts, and so does a note that supersedes a gotcha or a
  case. Search indexes what was tried; a hit says "tried 2: 1 worked, 1
  failed"; a reading lists the attempts, oldest first.
- **A note records its model.** `source.model` is the model of the run
  the note came from, at every write, the memory requests' too. A reading
  says to check a note from another model before relying on it, a hit
  names the model, and the notes page shows it beside who wrote it.
- **The memory requests keep knowledge, not progress.** One set of rules
  (`render.WORTH`) for compaction and the end of a run: what is worth a
  note, a title that states the claim, nothing of what the run did or
  how far it got, nothing git log records, no case that is the story of
  one run; search first, and work on a problem a note has goes in
  `memory_attempt`, a failed approach included.

## Consequences

- A later run sees what was tried for a problem and what failed, under
  which model, in one note instead of one note per run.
- Notes from before carry no model and no attempts; they read as before.
  The notes from before the rename are in `tau-memory` folders, which
  heiko does not read.
- The prompts' effect on what gets kept is not measured here:
  heiko-memory-e2e measures tasks that need an earlier run, not how many
  notes are progress.

## Alternatives

- **Attempts as their own notes, linked to the problem.** Search would
  then return attempts beside the problem and the one-note-per-run shape
  would stay; the paper's record is one entry per problem.
- **Predictions written before a change and checked after, as the paper
  does.** A task agent's run has no evolve set to check them against;
  `untested`, settled later, is the part that carries over.
- **The heiko version beside the model.** Plugins are not given it, and
  the crate's version has not changed; the commit is already recorded.
