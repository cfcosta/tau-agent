---
name: principle-guard-the-context-window
description: "Load when context is filling up or about to, with large command output, long files, repeated reads, bulk data, or fan-out planning. Send bulk reading to sub-agents or a codemode script, read files in slices, and keep summaries in the conversation, not raw payloads."
---

# Guard the Context Window

Keep the conversation for decisions and summaries. Move bulk reading and bulk data somewhere else, and bring back only what you need to decide.

**Why:** Context overflow degrades reasoning, forces compaction that loses detail, and halts progress. Every raw log or whole file you pull in pushes out something you needed later.

**Pattern:**
- **Send bulk reading to sub-agents.** On the main chat, `spawn` a sub-agent to read a subsystem, a log, or a set of files and answer a specific question. Ask for file paths, line numbers, names, and a short finding, not pasted content. Keep its summary. Spawn it with `land: false`, since it only reads. A forked sub-agent already knows the task, so the brief only needs the question and the shape of the answer. A fork also starts with your whole context: when yours is nearly full, spawn it with `fresh: true` and a brief that holds the question, the paths and the answer's shape, so it starts small. On a chat or a sub-agent, where `spawn` is refused, use the next items instead.
- **Process bulk data in a `codemode` script.** When the work is mechanical (count matches across many files, parse a large JSON or log, compare outputs of many commands), write a Luau script that calls the tools, does the processing, and returns a small result. The raw data never enters the conversation.
- **Search before you read.** Use `grep` and `find` to locate the lines that matter, then `read` with `offset` and `limit` around them. Read a whole file only when you need the whole file.
- **Do not re-run to see more.** When a command's output was cut to its head plus a `[full output: <path>]` note, `grep` or `read` that file in slices. Re-running a long command costs time and may not give the same output.
- **Make commands quiet.** Pipe through `tail`, `grep`, or `head`, or use the tool's own quiet flag, so a passing run prints a line, not a page.
- **Do not read the same thing twice.** Note what you learned from a file in your working notes before moving on. Re-read only when the file changed.
- **Size phases and cap scope.** Limit the files each phase touches and each sub-agent reads. Budget the turns a phase may take. Count the cost of the mechanism too: a sub-agent's report and a big diff both land in your context.
- **Keep durable facts out of the window.** A fact the next conversation needs goes to `memory_write`. A long report for the person goes on the canvas with `canvas_write`, and your reply points to it.

You skipped this when you pasted a whole log to find one line, read a large file top to bottom to answer a question `grep` could, or re-ran a slow command because its output was cut.

Distinct from `principle-minimize-reader-load`, which is the same idea for human readers of code. This principle protects your own working memory during the task.
