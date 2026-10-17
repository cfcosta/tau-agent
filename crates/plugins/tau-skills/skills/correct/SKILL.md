---
name: correct
description: "Load when the person corrects the same kind of mistake again, or asks to stop agents repeating mistakes in a repository. Mines history for repeated mistake classes and makes each impossible: architecture, then types, then a lint, then a test, docs last."
---

# Correct

The person keeps correcting agents in this repository for the same mistakes. Change the repository so the next agent cannot make them.

Assume every contributor is an agent that sees only the files it opened, copies the nearest example, and takes the shortest path that compiles. Design the repository so a change that looks right from one file is right for the whole repository.

This is `principle-encode-lessons-in-structure` applied to a whole repository. Load that principle too.

## 1. Find the mistake classes

Read the evidence, then group the mistakes into classes. A class counts once it has happened twice.

- **History.** `vcs_log` for recent commits. Look for `revert` and `fix` subjects, commits that undo part of an earlier one, and the same file fixed again and again. Read the suspicious ones with `vcs_show`. Line-level blame is not available, so `grep` the code for the pattern and `vcs_show` the commits that touched it. To scan many commits without filling your context, use `codemode` to run `vcs_show` over a batch and return only the subjects and the lines that match.
- **Workaround comments.** `grep` for `HACK`, `XXX`, `FIXME`, "do not", "never", "must", "workaround", "because X breaks", and lint or type-check suppressions with or without a reason. Each one marks a place where the code relies on a reader remembering something.
- **The repository's instructions.** Every rule in the instructions file (`AGENTS.md` or its equivalent) that reads as a correction ("never X", "always use Y") is a mistake that already happened. Check whether anything enforces it.
- **Memory.** `memory_search` for corrections the person gave in earlier conversations about this repository.
- **This conversation.** The correction that made you load this skill is the first piece of evidence.
- **Review comments.** If the person pasted review comments, or an MCP server exposes the repository's reviews, read those too. tau has no tool of its own for them.

## 2. Fix each class at the highest level that works

1. **Eliminate it with architecture.** Give each piece of state one owner and each task one supported way. Hide internals so the wrong import fails: a private module, or module-private or package-private visibility, so the bad call does not build outside its home. Replace hand-synced lists with one source of truth. Delete old ways and dead code an agent would copy.
2. **Enforce it with types, so the bad state cannot be written.** A distinct type with a private field and one checked constructor, such as a `Port` type in place of a bare integer. A tagged union (sum type) handled exhaustively in place of a bag of flags. Load `principle-type-system-discipline`.
3. **If bad code still builds, add a lint or a check whose error names the fix.** Use the linter the repository already runs: a lint rule that bans the call or import, with a message naming the fix, such as "start processes with `proc.spawn`, which sets the sandbox". If the linter has no such rule, a script in the check command that searches for the pattern does the same job. The error must name the file, type, or function to use instead. If the pattern is already common, fail only when a change adds more: count the uses and fail when the count rises.
4. **Test the behavior.** Fix or delete any test that still passes when the behavior it checks breaks, or that fails when only the internals change. Load `principle-test-behavior-not-implementation`.
5. **Write docs or rules in the repository's instructions file last, only for judgment calls.** Nothing fails when an agent skips them.

## 3. Fix and prove

Fix the most frequent classes now, one `vcs_commit` each. For each new check, prove it fails on a real past mistake: put the bad code back with `edit`, copied from the `vcs_show` of the commit that fixed it, run the check, and see it fail with the message that names the fix. Then take the bad code out again, see the check pass, and confirm with `vcs_diff` that none of it is left before you commit. A check you never saw fail proves nothing. Load `principle-prove-it-works`.

Wire every check into the command the repository's instructions already tell agents to run, so the same command fails locally and in CI.

An exception goes on the offending line as the linter's own suppression marker, with a reason, an expiry date, and the person's approval, such as "until 2027-01: the vendored parser needs it". Get the approval with `ask`, one question per exception, with "allow until the date" and "fix it now" as options.

## 4. Keep the rule table

Keep a table in the repository's instructions file that pairs each rule with what enforces it:

| Rule | Enforced by |
|---|---|
| Spawn processes through `proc.spawn` | a lint rule that bans direct process creation |
| Ports are `Port`, never a bare integer | the `Port` type |
| Pick the error message tone per the copy guide | judgment, no enforcer |

When the person corrects you, fix the mistake and add the rule. If the rule was already there and nothing enforces it, that is a repeat: fix it at the highest level in the same change. Drop a rule once its mistake cannot happen.

Commit the table with the fixes. On a chat, finish with `vcs_land`.

## Reply

For each class: the evidence (commits by change id, comments by file and line, the memory note), the level you picked, why a higher level did not work, and the command that showed the check failing on the past mistake. Then the classes you found but did not fix yet, most frequent first.
