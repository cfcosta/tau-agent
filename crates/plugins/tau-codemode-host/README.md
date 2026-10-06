# tau-codemode-host

The `codemode` tool, as a plugin. The tool's input is a Luau script. The
script runs in a sandbox inside the harness and calls the run's other
tools, and Jev, as functions. The model uses it to chain calls, run them
in parallel, and filter large results before it reads them. This crate
is Codemode's host half: the engine, the plugin and what the host does
for it.

## What it provides

| Item                     | What it is                                                   |
| ------------------------ | ------------------------------------------------------------ |
| `Codemode`               | The agent plugin: adds `codemode` and lists tool signatures  |
| `CodemodeTool`           | The `codemode` tool the plugin adds (`Codemode::tool`)       |
| `CodemodeHost`           | The `HostHalf` the app registers: the plugin for each run    |
| `run`, `Request`         | Run one script in a fresh VM and get an `Outcome`            |
| `Host`                   | What a script reaches outside its VM; tests implement it     |
| `ToolEntry`, `Namespace` | The tools and namespaces a `Host` offers                     |
| `ToolCall`, `ToolReply`  | One call from a script, and its answer                       |
| `options`                | The `-- @options:` line: `parse`, `Options`, `Source`        |
| `signature`              | Luau signatures from JSON Schema (`describe`, `render_tool`) |
| `search`                 | BM25 over the callable tools, for `search_tools`             |
| `inference`              | `InferRequest`, the nested `infer` tool's request            |
| `inference_budget`       | `Budget` and `Limits` shared by one run's `infer` attempts   |
| `jev`                    | The `jev` global's requests and answers, as JSON             |
| `json`, `value`          | Bounded JSON text, and JSON to Luau values and back          |
| `repository_modules`     | `RepositoryModules`: module versions kept per repository     |

Each call gets a fresh `mlua` VM with Luau's safe libraries, a 256 MiB
memory limit (`MEMORY_LIMIT`) and an interrupt that checks the deadline
and the cancel, yielding every `YIELD_EVERY` ticks. Output stops at
`MAX_OUTPUT_BYTES`. A script cannot reach files, processes or the
network. The globals that reach out (`tools`, `parallel`, `map`,
`search_tools`, `jev`, `require` and the rest) all go through `Host`.
The plugin also adds fixed nested tools that only scripts call: `infer`
and the `module_*` tools.

## How it fits

This is the host half (decision 0030). Its partner is `tau-codemode`,
the interface half, which holds the card, the records and their folds.
This crate re-exports the shapes its API returns: `Outcome`, `Rendered`,
`CallRow`, `CallStatus`, `Failure`, `Item` and `PLUGIN`.

It builds on `tau-agent`, `tau-ai`, `tau-codemode`, `tau-jev` and
`tau-ui-plugin`. `mlua` compiles Luau's C++, which is slow, so of the
apps only `tau` links this crate; `tau-ui` builds without it.

Used by:

- `tau`, the desktop app, which registers `CodemodeHost`;
- `tau-luau-plugins-host`, which runs Luau plugins' hooks in this
  sandbox;
- `tau-codemode-eval` (`crates/evals`);
- `tau-mcp-host`, in its tests and its `live` example.

`CodemodeHost` builds `Codemode::new(jev)` with the run's Jev, when there
is one, and with `with_repository` pointed at the repository's private
`codemode-modules` directory.

## Usage

Add the plugin after any plugin that adds tools in its own `start`, such
as tau-mcp's, so their signatures are listed:

```rust
use tau_agent::agent::Agent;
use tau_codemode_host::{Codemode, inference_budget::Limits};
use tau_tools_host::{path::Root, plugin::CodingTools};

let agent = Agent::new(llm.clone())
    .plugin(CodingTools::new(Root::new("/path/to/repo")))
    // `None`: no Jev, so `jev` is nil in scripts.
    .plugin(Codemode::new(None).with_inference_limits(Limits {
        max_calls: 8,
        ..Limits::default()
    }));
```

`with_inference_model` sets the model for `infer`, and `with_repository`
adds repository modules and `module_promote`. Without the plugin, a
`codemode` call fails: the tool needs its plugin's context.

To run a script without an agent, implement `Host` and call `run`:

```rust
use std::sync::Arc;
use tau_codemode_host::{CancellationToken, Request, options, run};

let source = options::parse("return tools.echo({ text = \"hi\" })")?;
let budget = source.options.max_output_tokens();
let outcome = run(host, Request {
    call_id: "call_1".into(),
    source,
    store: Default::default(),
    cancel: CancellationToken::new(),
})
.await;
let rendered = outcome.render(budget);
```

## Testing

```sh
cargo nextest run --release -p tau-codemode-host
```

`tests/common` has `FakeHost`, a `Host` with a few fake tools, for the
sandbox tests. Agent-level tests (`tests/plugin.rs`,
`tests/infer_tool.rs` and others) run the loop with `tau-testing`'s
`ScriptedModel` and an in-memory `tau-store-sqlite` store. Many tests are
Hegel properties (options, signatures, values, JSON, search, `map`,
module loading and promotion); the workspace's `hegel.toml` sets their
case counts. Nothing needs network or credentials, but the first build
compiles Luau.

## Further reading

- [Codemode reference](../../../docs/reference/codemode.md)
- [`map`](../../../docs/reference/codemode-map.md)
- [Module loading](../../../docs/reference/codemode-module-loading.md)
- [Module tools](../../../docs/reference/codemode-module-tools.md)
- [Module tests](../../../docs/reference/codemode-module-tests.md)
- [Module promotion](../../../docs/reference/codemode-module-promotion.md)
- [Repository modules](../../../docs/reference/codemode-repository-modules.md)
- [Inference](../../../docs/reference/codemode-inference.md),
  [request](../../../docs/reference/codemode-inference-request.md),
  [budget](../../../docs/reference/codemode-inference-budget.md) and
  [traces](../../../docs/reference/codemode-inference-traces.md)
- [Evaluation](../../../docs/reference/codemode-evaluation.md)
- [pi's Codemode, audited](../../../docs/research/pi-codemode.md)
- [0018: MCP servers and Codemode, as two plugins](../../../docs/decisions/0018-codemode-and-mcp.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
