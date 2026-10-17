# Explorer prompt template

Build each explorer's task from this template. Fill in the placeholders. On the main chat this is the `spawn` task, spawned with `land: false`. On a chat, it is the brief you follow yourself for one angle.

---

Do not edit files. Commit nothing. Your job is to read and report.

Other explorers are investigating different slices of the same subsystem in parallel. Don't try to cover everything. Focus on your assigned angle and go deep. Use `read`, `grep`, `find` and `ls`. Use `codemode` when you need many reads or greps and only a small result.

## Question

> {QUESTION}

## Your exploration angle

{EXPLORATION_ANGLE}

## Output

Your final message is your report. Use these sections.

### Components found
The key types, interfaces, services, modules and abstractions. For each: name, file path, and a one-sentence description of what it does.

### Flow
The execution flow step by step. For each step: what function or method runs, what file it's in, what it does, what it calls next. Include the data that flows between steps.

### Files read
Every file you read during exploration, so the explanation can reference them.

### Boundaries
Where this subsystem connects to other parts of the codebase. The inputs and outputs.

### Non-obvious things
Anything surprising, historically motivated, or easy to get wrong. Things that look like they should work one way but work another.

### Open questions
Anything you couldn't fully trace or understand. Be honest about gaps.
