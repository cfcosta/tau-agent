---
name: principle-encode-lessons-in-structure
description: "Load when you catch yourself writing the same instruction a second time, or the person corrects the same mistake again. Encode the rule as architecture, a type, a lint, a test, or a runtime check, and prove it fails on a real past mistake. Text is the last resort."
---

# Encode Lessons in Structure

When a mistake repeats, change the code so the mistake cannot be made, or fails loudly when it is. Do not answer a repeated correction with another sentence in the instructions file.

**Why:** An instruction works only when the reader opens the file, notices the line, and agrees it applies. Agents see only the files they opened, copy the nearest example, and take the shortest path that compiles. A compiler error or a failing check reaches every contributor, every time, with no reading required.

**Pick the highest lever that works, in this order:**
1. **Architecture.** Remove the possibility. Give each piece of state one owner and each task one supported way. Hide internals so the wrong import does not resolve. Replace hand-synced lists with one source of truth. Delete the old way, so nobody copies it.
2. **Types.** Make the bad state impossible to write. Load the `principle-type-system-discipline` skill.
3. **Lint rules.** When bad code still compiles, add a check whose error message names the fix. A lint rule that bans the call, with a message naming what to call instead, is one example. Use the linter the repository already runs. A custom lint, a grep in the repository's check script, or a schema check also count. If the pattern is already common, fail only on new occurrences.
4. **Tests.** Pin the behavior the mistake broke. Load the `principle-test-behavior-not-implementation` skill.
5. **Runtime checks.** An assertion or a startup validation that fails fast with a clear message, for what only shows at runtime.
6. **The repository's instructions file, last.** Use it only for judgment calls nothing can check. When you do, make the rule prominent and add an example of the failure.

**Prove each new check works.** Recreate a real past instance of the mistake (from `vcs_log` and `vcs_show`, or the conversation where it happened), run the check, and watch it fail with the message you wrote. Then remove the instance and watch it pass. A check you never saw fail may catch nothing.

**Corollary:** If the fix is structural, use only the structural fix. Delete the instruction it replaces. The instruction was the symptom.

You skipped this when the instructions file grows a second "never do X" line about the same mistake, or a correction lands as a comment or a memory note while the code still allows the mistake.

To apply this to a whole repository, mining its history for mistake classes, load the `correct` skill. Distinct from `principle-build-the-lever`, which builds a tool to do or prove one piece of work. This principle builds a check that keeps a mistake from coming back.
