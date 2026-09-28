# 0001: A library for agent workflows, not a coding-agent product

- Status: accepted; the optional UI crate is in
  [0007](0007-gpui-interface.md)
- Date: 2026-09-26

## Context

pi is a terminal coding agent. About two thirds of its relevant code
serves a person at a keyboard:

- RPC and print modes, and a CLI.
- Settings files and `AGENTS.md` discovery.
- Skills and prompt templates.
- A branching session tree with labels and context edits.
- Follow-up queues, cache warming, and the TUI.

Porting all of that would take about 17.5k lines of Rust over roughly
three months.

Our goal is different: arranging agent workflows from code.

## Decision

tau-agent is a library, built around three concepts:

- **`Agent`** is a value: a model, instructions, tools, hooks and limits.
- **`Run`** is one execution of an agent. It has an event stream,
  `steer`, `cancel`, and a final result that can be typed.
- **Workflows** are ordinary async Rust. The library ships no graph
  language and no scheduler.

The library adds a few workflow primitives that pi lacks:

- typed results;
- agents usable as tools;
- per-run limits that include the usage of child runs;
- forks from a checkpoint;
- a scripted model for tests.

## Consequences

- The core is about 7.5k lines of Rust, estimated at about 5 weeks.
- Out of scope:
  - a CLI, a TUI, and an RPC server in the library (an optional GUI
    crate sits beside it, see [0007](0007-gpui-interface.md));
  - settings files and context-file discovery;
  - skills and prompt templates;
  - pi's session tree and `context_edit`;
  - follow-up queues. A caller that wants a follow-up simply starts
    another run.
- The coding tools become an optional crate (see
  [0004](0004-coding-tools-optional.md)).
