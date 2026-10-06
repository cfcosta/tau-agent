# tau-tools-host

The coding tools themselves: `read`, `bash`, `edit`, `write`, `grep`,
`find` and `ls`, ported from pi. Every tool takes a root directory at
construction, and every path it is given resolves against it. A tool
that fails returns `Err`, and the agent loop turns that into an error
result. The crate is optional: an agent without it has no file or shell
access.

## What it provides

| Item                         | What it is                                                                         |
| ---------------------------- | ---------------------------------------------------------------------------------- |
| `plugin::CodingTools`        | The tools on one root, as a `Plugin` for `Agent::plugin` (Unix only)               |
| `plugin::Tool`               | One of the seven tools; `Tool::ALL` lists them in pi's order                       |
| `coding_tools`               | The same seven tools as a `Vec`, for `Agent::tools` (Unix only)                    |
| `path::Root`                 | Where paths resolve: the root directory, and the home `~` stands for               |
| `read`, `bash`, `edit`, ...  | One module per tool: `Read`, `Bash`, `Edit`, `Write`, `Grep`, `Find`, `Ls`         |
| `artifact_read`              | `ArtifactRead`, the nested `artifact_read` tool over granted artifacts            |
| `artifact_grant`             | `publish_artifact`, plus the grant shapes re-exported from `tau-tools`             |
| `truncate`                   | `truncate_head`, `truncate_tail`, `truncate_line` and the shared limits            |
| `lock`                       | Process-wide per-path locks that `edit` and `write` hold                           |
| `image`                      | Image detection and resizing for `read`                                            |
| `errno`                      | I/O errors written as Node prints them, as models saw them from pi                 |
| `details`                    | Re-export of `tau_tools::details`, the `ls` listing shape                          |
| `ToolsHost`                  | tau-tools' `HostHalf`, for tau's plugin registry                                   |
| `SearchError`, `ABORTED`     | A search whose glob or pattern does not parse; the cancelled-call message          |

`CodingTools` has builder methods: `only(&[Tool])` and `without(Tool)`
pick a subset (pi's order is kept), `with_launcher` starts `bash`'s
commands through a `tau_agent::launch::Launcher`, and `with_artifacts`
gives `read` and `bash` private byte storage (`tau_artifacts::Bytes`)
for full outputs. `CodingTools` always adds `artifact_read` as a nested
tool; without storage, a call to it fails.

## How it fits

This is the host half (decisions 0017, 0030): the behaviour that runs
inside the agent. Its partner is [tau-tools](../tau-tools), which draws
the tools' cards and owns the shapes they return.

- Builds on `tau-agent` (`Plugin`, `AgentTool`, `launch`),
  `tau-artifacts`, `tau-ai`, `tau-tools` and `tau-ui-plugin`, and, with
  the `terminal` feature, `tau-terminal`.
- Used by `tau-ui`, `tau-vcs-host`'s tests, and the evals
  `tau-codemode-eval`, `tau-output-pruning-eval` and `tau-memory-e2e`.

`ToolsHost` adds no agent plugin itself. The host builds `CodingTools`
on each run's workspace, where the tools act. `ToolsHost` answers the
cards' `Action::Read`: a range of an artifact the run was granted.

## Usage

```rust
use tau_agent::agent::Agent;
use tau_tools_host::{
    path::Root,
    plugin::{CodingTools, Tool},
};

let root = Root::new("/path/to/project");

// All seven tools.
let agent = Agent::new(llm.clone()).plugin(CodingTools::new(root.clone()));

// Read-only: no `bash`, `edit` or `write`.
let read_only = [Tool::Read, Tool::Grep, Tool::Find, Tool::Ls];
let reviewer = Agent::new(llm).plugin(CodingTools::new(root).only(&read_only));
```

`bash`, and so `CodingTools` and `coding_tools`, exist only on Unix:
`bash` runs each command in its own process group.

## Features

| Feature    | Default | Effect                                                                                   |
| ---------- | ------- | ---------------------------------------------------------------------------------------- |
| `terminal` | off     | `bash` runs under a pseudo-terminal through `tau-terminal` (libghostty-vt), and streams the raw bytes in its details |

Without `terminal`, `bash` uses pipes and `tau-terminal` is not built.
With it, `Bash::with_terminal(false)` goes back to pipes.

## Testing

```sh
cargo nextest run --release -p tau-tools-host
cargo nextest run --release -p tau-tools-host --features terminal
```

- Several suites are Unix only (`bash`, `agent`, `bash_launch`,
  `artifact_ranges`, `bash_artifacts`). `tests/bash_terminal.rs` needs
  the `terminal` feature.
- `tests/agent.rs` runs every tool inside a real run, with
  `tau_testing::scripted::ScriptedModel` choosing the calls and an
  in-memory store from `tau-store-sqlite`.
- Most suites hold Hegel property tests (`hegeltest`).

A benchmark times `grep` on a generated tree. `FILES` and `FILE_KB` set
its size, and `ITERS` how many times each case runs:

```sh
cargo bench -p tau-tools-host --bench grep
FILES=20000 FILE_KB=16 cargo bench -p tau-tools-host --bench grep
```

## Further reading

- [Coding tools reference](../../../docs/reference/tools.md)
- [Agent commands' environment](../../../docs/reference/environment.md)
- [Bash structured results](../../../docs/reference/codemode-bash.md)
- [Full-file artifacts from read](../../../docs/reference/codemode-artifact-files.md)
- [Code Mode command artifacts](../../../docs/reference/codemode-command-artifacts.md)
- [Code Mode artifact ranges](../../../docs/reference/codemode-artifact-ranges.md)
- [0004: Coding tools live in an optional crate](../../../docs/decisions/0004-coding-tools-optional.md)
- [0010: bash output is a terminal, emulated by libghostty-vt](../../../docs/decisions/0010-terminal-rendering.md)
- [0025: Agent commands run in an environment plugins give](../../../docs/decisions/0025-agent-commands-run-in-an-environment-plugins-give.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
