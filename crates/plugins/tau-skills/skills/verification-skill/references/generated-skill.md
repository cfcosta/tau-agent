# The generated skill's sections

A `verify-<app>` skill has these six sections, in this order, after its frontmatter. Each one holds the real commands, paths, and handles of this repository. No placeholders.

## Launch

The exact command that starts the app for verification, and how to tell it is ready: a log line, a prompt, a status endpoint that answers with the right body.

- Launch must reach a usable state from an empty data dir, including migrations and seed data.
- Ready means usable, not just that a process answers. A database health check can pass while its init scripts still run.
- Give each run its own data dir, port, and profile when the app allows it (`NOTES_DATA_DIR=$SCRATCH/notes-$RUN_ID`), so concurrent runs and the person's own instance do not share state.
- Start long-lived processes with `exec_command` so they keep running in the background, and record each process id the run started.
- Include teardown, and point at Cleanup.
- For a short-lived CLI or TUI there is no server to keep alive. Launch means build the binary or install dependencies once, then start each drive in its own isolated session.
- For a desktop GUI on Linux, Launch starts the headless compositor first and the app inside it. Follow the recipe and the guard in `harnesses.md`.

## Doctor

One read-only check that answers "is this instance worth driving?": the process is up, it is the right version or build, the port belongs to this run, auth is valid, and (for a GUI) the app's window is on the headless output. An agent runs it before the first drive and whenever anything looks off. Doctor changes nothing.

## Drive

The harness recipe, with real selectors and commands from this repository, not examples.

- Prefer stable handles: accessibility roles and names, data attributes, prompt strings, route paths, command names. Avoid coordinates and tab order.
- Wait for a condition (an element, a log line, a prompt), never a fixed sleep, unless nothing observable exists. Then say why.
- Show the exact invocation for every action, the way the feature-map example does.

## Evidence

What to capture for a proof, and where it goes. State the proof standards in the skill:

- Exercise the real user path, not internal setters or test-only endpoints.
- Capture the action and the resulting state, not only the final screen.
- Verify side effects (files written, rows inserted, messages sent) alongside what is visible.
- Use mocks only where a production boundary already isolates the external system.
- When the safe path is a dry-run or a test mode, check what it really skips by observing files, network, and VCS state. Do not trust its name. Some dry-runs still touch the network or open a browser.
- UI proof includes a screenshot (and an accessibility snapshot where the harness has one). CLI proof includes the command, stdout, stderr, and exit code. Mutation proof includes a second, read-only view of the stored value.
- Name the evidence folder, outside anything Cleanup deletes. Tell the agent to show screenshots to the person with `canvas_show`.

## Cleanup

How to tear down what the run created.

- Kill only what this run started, by the process ids Launch recorded. Never kill by process name: no `pkill -f`, no `killall`. In a shell trap, never `kill 0` or `kill ${pid:-0}`, which kills the script's own process group.
- Remove instances and scratch state, never the evidence. Proof survives teardown, in the location Evidence names.
- Remove any verification scaffolding Launch created.
- Cleanup runs after failed attempts too.

## Helpers

Any script the skill ships lives in the skill's folder, is executable, and has its invocation shown in the skill body. A helper the reader has to reverse-engineer is not a helper. Good helpers: `launch`, `doctor`, `drive <action>`, `shoot <path>`, `cleanup`. Each prints what it did and exits non-zero on failure.

## The feature map

`features/README.md` indexes the features and holds the baseline preconditions, the driving conventions, and the proof and skip rules. Each feature file starts with an H1 title and one paragraph on the user-visible behavior, then exactly four H2 sections:

1. `Sub-features`: short ids, one line per behavior.
2. `How to get to it (user POV)`: every entry point a person uses.
3. `Driving it with <harness>`: starts with `Preconditions:`, then labeled bullets that pair each user action with an exact command and an observable result.
4. `Gotchas`: traps that can waste or invalidate a run.

Keep implementation details out of the map. Name only user paths, stable handles, required state, commands, and observable proof.
