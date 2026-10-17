---
name: principle-laziness-protocol
description: "Load when refactoring, sizing a diff, or tempted to add an abstraction, a layer, an option, or a flag threaded through calls. Bias to deletion and the smallest change that solves the problem."
---

# Laziness Protocol

Make the smallest change that solves the problem. Prefer deleting code to adding it. Every line you add is a line someone must read, test and keep working.

**Why:** Each extra layer, option or helper costs review time now and reader time for as long as it lives, and most are built for a second case that never arrives. A small diff is easy to check and reverts cleanly.

**Pattern:**
- **Delete first.** Before you add, look for code the change makes dead or redundant, and remove it.
- **Minimize the diff.** Fewer lines beat elegant boilerplate. Do not reformat, rename or reorder code the task does not need touched.
- **Consolidate decisions.** When the same choice is made in several places, put it behind one source of truth (a function, a constant, a config field) and pass the result down as a plain value or flag. Callers do not re-derive it.
- **No speculative layers.** No interface with one implementation, no wrapper with one caller, no option nobody sets, no extension point for a case that does not exist yet. Add the layer when the second case arrives.
- **No signal threading.** Do not pass a new flag through five functions so the sixth can branch on it. Decide at the one place that knows, or change the data so the branch goes away.
- **Remove the condition instead of compensating for it.** A cleanup step that undoes a bad state on every run costs more than the change that stops the bad state.
- **The smallest script, never a framework.** A twenty-line script that does or proves the job beats a configurable tool.
- **Small commits.** One `vcs_commit` per idea, so each diff stays small enough to read in one sitting.

**The tests:**
- "If I delete this, what breaks?" If nothing, delete it.
- "How many callers does this abstraction have today?" If one, inline it.
- "Is this choice made in more than one place?" If yes, consolidate it.

You skipped this when the diff touches files the task never needed, or adds a parameter that every caller passes with the same value.

Lazy means the smallest change that *solves the problem*. A patch on the symptom does not solve it (load the `principle-fix-root-causes` skill), and skipping the test or the check is not a smaller change, it is an unfinished one.

Distinct from `principle-subtract-before-you-add`, which sets the order of the work (removal first, as its own commit). This principle sets the size of each change.
