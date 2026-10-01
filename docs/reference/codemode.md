# Codemode (`tau-codemode`)

- Status: the engine is built (`crates/plugins/codemode`, behind a
  `Host` trait); the plugin that wires it to `ToolCtx::call` is not.
  Decided in [0018](../decisions/0018-codemode-and-mcp.md).
- Date: 2026-10-01

`codemode` is a tool whose input is a Luau script. The script runs in
a sandbox in the harness and calls the agent's other tools, and Jev,
as functions. The model uses it to chain calls, to run them in
parallel, and to filter large results down before it reads them.

This follows pi's Codemode (`packages/codemode` and
`coding-agent/src/extensions/codemode`, audited in
[research/pi-codemode.md](../research/pi-codemode.md)) with Lua in
place of JavaScript. Keep the strings and limits written here; where
tau differs from pi, it says so.

## The tool

- **Name:** `codemode`. Exposure `ModelOnly`: scripts cannot call it.
- **Arguments:** `{ code: string }`, described as "Luau source. The
  script is the body of a function: `return` works. May start with a
  `-- @options: {"max_output_tokens": 1000}` line."
- **Options line:** an optional first line
  `-- @options: <JSON object>`. Keys:
  - `max_output_tokens`: a non-negative integer, default 10,000;
  - `timeout_ms`: a positive integer up to 2^31 − 1. No default: a
    script runs until it ends or the run is cancelled, as in pi.

  Another key, bad JSON, an options line with no code after it, or
  empty code fails the call before anything runs, with only the
  message:
  - ``@options only supports `max_output_tokens` and `timeout_ms`; got `yield` ``
  - `@options is not valid JSON: <serde's error>`
  - `@options must be a JSON object.`
  - `@options must be followed by code on the next lines.`
  - ``The code is empty: pass Luau source in `code`.``
  - `` `max_output_tokens` must be a non-negative integer.``
  - `` `timeout_ms` must be a positive integer up to 2147483647.``

  The options line stays in the source the VM runs: it is a Luau
  comment, so error line numbers match the model's input.

- **Execution mode:** `Parallel`. Two codemode calls in one batch run
  side by side in separate VMs.

## The sandbox

- **One fresh VM per call:** `mlua::Lua` with Luau, every safe library,
  globals installed, then `sandbox(true)`. Nothing is shared between
  calls but the store.
- **Memory:** 256 MiB (`set_memory_limit`). Going past it fails the
  script with `not enough memory`.
- **CPU:** an interrupt (`set_interrupt`) checks the cancel token and
  the deadline. It returns `VmState::Yield` every 4,096 ticks, so a
  loop with no call in it neither blocks a tokio worker nor outlives a
  cancel. It yields only the threads the engine runs (the script's and
  those `parallel` starts): a yield inside a coroutine the script made
  would reach that coroutine's `resume` as if the script had yielded.
  A loop there still stops, because once the deadline passes or the
  run is cancelled the interrupt raises an error at every tick, which
  `pcall` cannot outrun.
- **What a script cannot reach:** files, processes, the network,
  timers, `require`, `loadstring` of bytecode. mlua adds a `require`
  that reads files, so the engine removes it. Only the globals below
  reach outside the VM, and every one of them goes through the loop or
  the plugin.
- **Output:** at most 64 MiB of output text and images, which live
  outside the VM's memory limit; past it `text`, `print` and `image`
  raise.
- **Stalls:** pi fails a script that waits on a promise no call can
  settle. Luau has no promises: a script either runs or waits on a
  host call, so it cannot stall.
- **Ending:** the engine resets its suspended threads and collects
  garbage before it drops the VM. A pending call's future holds the VM,
  so without this the two would keep each other alive and the call
  would never be cancelled.

## Globals

```luau
-- Tools: every tool callable from tools, by its name.
tools.read({ path = "Cargo.toml" })            --> string or table
tools.mcp__linear__list_issues({ team = "PI" }) --> CallToolResult

-- Several calls at once; results in argument order.
local a, b = parallel(
    function() return tools.grep({ pattern = "todo" }) end,
    function() return tools.ls({ path = "src" }) end
)
parallel_settled(f1, f2) --> { { ok = true, value = ... }, { ok = false, error = "..." } }

-- Output, in the order it is made.
text(value)       -- strings as they are, other values as JSON
print(...)        -- like text, arguments joined by tabs
image(item)       -- a data: URL, { image_url = ... }, or an MCP ImageContent
exit()            -- ends the script now, successfully
return value      -- appended like text

-- The store: JSON values kept for later codemode calls in this conversation.
store(key, value) -- value nil deletes the key
load(key)         --> a copy, or nil

-- Discovery.
ALL_TOOLS                       --> { { name, description }, ... }
search_tools(query, { limit = 8, namespace = "mcp__linear" })
describe_tool(name)             --> its description and Luau signature, or nil
describe_namespace(name)        --> { name, description, instructions, tools }, or nil

-- Jev (only when the run has it).
jev.noul({ state = s, question = "...", yes = "...", no = "..." })
    --> { probability = 0.93 }
jev.choice({ state = s, question = "...", options = { bug = "...", feature = "..." } })
    --> { choice = "bug", confidence = 0.8, probabilities = { ... } }
jev.score({ state = s, question = "...", levels = { "low", "mid", "high" } })
    --> { score = 2, confidence = 0.7, probabilities = { ... } }
jev.ask({ state = s, questions = { a = { kind = "noul", question = "..." }, ... } })
    --> { a = { probability = ... }, ... }   -- one round trip

-- JSON shapes.
json.null          -- JSON null inside a table
array({})          -- an empty table that encodes as [] instead of {}
```

### Values

- JSON `null` is `json.null`; JSON arrays carry mlua's array
  metatable, which `array(t)` sets, so `[]` comes back as `[]`. A table
  without it is an array when its keys are exactly `1..n`, `n > 0`,
  and an object otherwise.
- Numbers are doubles: integral values within ±2^53 come back as JSON
  integers, non-finite ones as `null` (as `JSON.stringify` does).
  Object keys come back sorted.
- `text(nil)` appends `null`; `print` shows `nil` as `nil`. `return a,
b` appends each value that is not `nil`. A return value JSON cannot
  hold (a function) fails the script with
  `The script's return value: a function cannot be encoded as JSON`.
- Globals that reach out return `(ok, ...)` to a small Luau prelude
  that raises a string error, so `pcall` gets a message, not a
  userdata. Argument errors carry the caller's line
  (`codemode:3: store(): the key must be a string`); a tool's error
  text is raised as it is.

### Tool calls

- A call goes through `ToolCtx::call`, so the loop repairs and
  validates the arguments and runs every plugin's `before_tool` and
  `after_tool_result`, as for a call the model makes. A tool a plugin
  blocks fails in the script.
- Callable tools are the run's tools whose exposure is `Direct` or
  `Nested`, and the tool sources' tools (`ToolCtx::catalog`).
  `codemode` and other `ModelOnly` tools are not callable.
- **Names:** a tool's name is its key in `tools`. Names are already
  identifiers (MCP tools are sanitized by tau-mcp), so there is no
  second, normalized name as in pi.
- **The value a call returns:**
  - the tool's `structured` output, when it has an output schema and
    the output carries one, even for an error result that carries one
    (an MCP `isError` result is returned, not raised; the script checks
    `isError`);
  - otherwise the output's text, joined;
  - a failed call with no structured output raises a Lua error whose
    message is the tool's error text. `pcall` catches it. When nothing
    catches it, the result shows it with the script's line, as
    `codemode:4: tool broke`.
- **Images** a tool returns are not shown to the model unless the
  script passes them to `image()`.
- **Parallel calls:** a call inside a `parallel` function yields its
  coroutine; `parallel` awaits all its coroutines together, so their
  tool calls run at once. `parallel` raises the first error after every
  function has finished; `parallel_settled` never raises. A script that
  ends with calls still running has them cancelled. pi's sequential
  rule holds: among one script's calls, a `Sequential` tool's call
  waits for the others and runs alone.
  Jev requests are limited to 4 in flight, as pi limits classifier
  calls.
- The first error `parallel` raises is the first in argument order.
  `exit()` in one of its functions ends the script at once, and the
  others' calls are cancelled.
- Prototyped: mlua 0.12 awaits several `into_async` threads inside an
  async callback of the same VM, and their calls overlap. `parallel`
  is built that way; the call-handle fallback is not needed.

### The store

- `exit()` ends the script even inside `pcall`: the engine stops
  polling it, and its writes are kept.
- **What pi does:** "Codemode state is maintained as part of the
  session transcript instead of the file system." Values are JSON, at
  most 256 KiB of JSON text each and 1 MiB in all. Keys are strings.
- **In tau:** a successful script's writes become one plugin record,
  `{ "store": { "set": { key: value }, "delete": [key] } }`, through
  `ToolCtx::plugin()`: `publish` once step 1 of
  [0017](../decisions/0017-plugins-bring-their-ui.md) lands, `record`
  and `report` until then. A failed script writes nothing.
- **Loading:** each call folds the plugin's records along the run's
  fork chain (`PluginCtx::records`), oldest first, into the snapshot.
  Records that do not parse are skipped. A fork sees the values its
  ancestors wrote before it forked, and not those written after: the
  same branch rule as pi.
- `load` returns a copy: changing it does not change the store.

### Jev

- `jev` exists when the plugin was built with a Jev (in tau-ui, a
  metered Jev when a TypeSafe key is saved). Without one, `jev` is nil,
  and the description says so.
- `state` is any JSON value; `question` is the instructions. The
  question kinds and limits are tau-jev's (at most 255 choice options,
  2 to 10 score levels). Answers are checked as tau-jev checks them.
- Each request's usage is charged to the run
  (`PluginCtx::charge`), so it counts toward the run's limits and the
  plugin's cost. The call rows in `details` carry it; their ids are
  `<parent>/jev/<n>`, apart from the tools' `<parent>/<n>`.
- A failed request raises a Lua error with the error's text.

### Discovery

- `search_tools` ranks with BM25 (k1 1.2, b 0.75) as pi's tool search
  does. A tool's document is its name, its name with `_` as spaces, its
  description, its input schema's property names and descriptions, and
  its namespace's name, description and instructions. Tokens split on
  camelCase and on anything not a letter or digit, are lowercased, drop
  21 stop words, and lose a naive plural (`queries` → `query`, `boxes`
  → `box`, `files` → `file`). Tools that match no term are left out.
- Unlike pi, a tool whose name is the whole query ranks first.
- Namespace names match without case, with `-` as `_`, and with or
  without `mcp__`, in `search_tools`'s `namespace` and in
  `describe_namespace`.
- `describe_tool` returns the tool's signature (its description is the
  signature's comment), followed by the `CallToolResult` types when it
  uses them.

## The description

The description is fixed for the agent: it never lists tools, so the
prompt cache holds across runs and as servers connect. It is pi's
`DESCRIPTION_INTRO` rewritten for Luau:

```
Run Luau code to orchestrate and compose tool calls
- Evaluates the provided Luau code in a fresh sandbox as the body of a function: `return` works.
- Every callable tool is a function on the global `tools` table, for example `tools.read({ path = "a.txt" })`. MCP tools are named like `tools.mcp__linear__list_issues`.
- Tool functions take one table as their argument.
- Tool functions return a table or a string, as their signature says.
- A tool call that fails, is blocked, or gets invalid arguments raises an error carrying the tool's error text. Use pcall to catch it.
- Calls inside `parallel(f1, f2, ...)` run at the same time; it returns each function's result in order. `parallel_settled` returns { ok, value | error } for each instead of raising.
- Runs plain Luau: no files, no processes, no network, no timers, no require.
- Accepts raw Luau source, not JSON, quoted strings, or markdown code fences.
- You may start the code with a line like `-- @options: {"max_output_tokens": 1000, "timeout_ms": 60000}`.
- `max_output_tokens` sets the token budget for the script's output. Defaults to 10000 tokens.
- `timeout_ms` sets a hard deadline for the whole script. By default there is none.
- When the script ends, calls still running are cancelled.
- Tool calls are real and have side effects. If the script fails partway, earlier calls are not undone.
- Scripts have a 256 MB memory limit. Filter or aggregate large data instead of accumulating it.

Globals:
- `exit()`: ends the script successfully at once.
- `text(value)`: appends a text item. Non-string values are encoded as JSON.
- `print(...)`: appends its arguments, joined by tabs, like `text`.
- `image(item)`: appends an image: a base64 `data:` URL, `{ image_url = ... }`, or an MCP ImageContent block such as `result.content[1]`.
- `store(key, value)`: keeps a JSON value under a string key for later `codemode` calls in this conversation. `nil` deletes the key. Writes are kept only if the script succeeds.
- `load(key)`: returns a copy of the stored value, or `nil`.
- `ALL_TOOLS`: `{ name, description }` for every callable tool.
- `search_tools(query, { limit?, namespace? })`: the callable tools that best match the query (default limit 8), as `ALL_TOOLS` entries.
- `describe_tool(name)`: a tool's description and Luau signature, or `nil`.
- `describe_namespace(name)`: `{ name, description, instructions, tools }` for a namespace such as an MCP server, or `nil`.
- `json.null` is JSON null; `array({})` is an empty JSON array.
- `return value` appends the value like `text`.
```

Then, when the plugin has Jev, a `Jev:` section with the `jev`
functions and their Luau types. Then:

```
Some tools may not be declared to you, such as MCP tools that are only for scripts. They are still callable on `tools` and listed in `ALL_TOOLS`. Find one with `search_tools(query)`, and read a server's instructions with `describe_namespace(name)`.
```

### Signatures in the run's context

At `start`, the plugin puts the Luau signatures of the run's `Direct`
tools (`RunPlan::tools`) in `plan.context`, within 3,000 tokens
(chars / 4), shared round-robin across namespaces, cheapest first, as
pi's `selectCatalog` does. Tools that do not fit are left to
`search_tools`. `Nested` tools are never listed: there are many, and
they change as servers connect.

### Luau signatures from JSON Schema

A hand-written renderer over `serde_json::Value`, after pi's
`schemaToType` (`codemode/src/declarations.ts`):

| Schema                        | Luau                                                             |
| ----------------------------- | ---------------------------------------------------------------- |
| `string`, `number`, `integer` | `string`, `number`, `number`                                     |
| `boolean`, `null`             | `boolean`, `nil`                                                 |
| `array` with `items`          | `{ T }`; tuples `{ any }`                                        |
| `object` with `properties`    | `{ a: T, b: T? }`, sorted; not required gets `?`                 |
| `additionalProperties: T`     | `{ [string]: T }`                                                |
| `enum`, `const`               | singleton unions: `"a" \| "b"`, or the base type for non-strings |
| `anyOf`, `oneOf`              | `A \| B`                                                         |
| `allOf`                       | `A & B`                                                          |
| local `$ref`                  | expanded, at most 32 expansions; recursive or remote: `any`      |
| anything else                 | `any`                                                            |

- A property with a description goes on its own line with a `--`
  comment; otherwise the object stays on one line. Comments split at
  `\r` as well as `\n`, and other control characters become spaces:
  Luau ends a comment at `\r` and stops reading at NUL.
- A property whose name is not an identifier is `["name"]: T`.
  Control characters in it are three-digit escapes (`\031`), so a digit
  after one is not read into it. A name holding NUL cannot be written
  in Luau: it falls to the `[string]: any` indexer.
- An object with no properties is `{ [string]: any }`; a `type` list
  is a union, and `null` in a union makes it optional (`T?`).
- An input type over 16,000 characters becomes `any`.
- The return type comes from the tool's output schema, else `string`.
  MCP results render as `CallToolResult<T>`, with a shared
  `CallToolResult` type block, as pi does.
- A tool renders as:

  ```luau
  -- Read a file.
  function tools.read(args: { path: string, offset: number?, limit: number? }): string
  ```

## The result

- **Content:** a text block `Script completed` or `Script failed`, then
  `Wall time X.X seconds`, then `Output:`, then the output items in the
  order they were made.
- **On failure** a last text item: `Script error:` and the error with
  its line, then
  `Tool calls made before the failure (they are not undone): read (ok), bash (error)`
  (`, and N more` past the rows' cap) or `No tool calls were made.`
  Heads for other failures:
  `Script timed out: it ran past its timeout_ms of N ms.`,
  `Script cancelled: the run was cancelled before the script finished.`,
  `Script sandbox failed: …`.
- **Budget:** the text items are measured at chars / 4. Past
  `max_output_tokens`, they are merged (joined by newlines) and cut in
  the middle, half kept
  from each end at UTF-8 boundaries, as
  `Warning: truncated output (original token count: N)\nTotal output lines: L\n\n<head>…N tokens truncated…<tail>\n\n[Full output: <path> (read it with offset/limit)]`,
  the full text written to `$TMPDIR/tau-codemode-<hex>.txt`. Images
  follow.
- **`is_error`** on failure.
- **`details`:** `{ calls, complete, store, usage }`.
  - `calls`: at most 256 rows
    `{ id, name, args, status, ms, error, cost }`, `args` cut at 200
    characters and `error` at 500 (a cut ends in `…`); `complete` is
    false past the cap. The interface draws the card from these.
    Compaction reads them for the files a script read or changed.
  - `store`: the writes, when the script succeeded.
  - `usage`: Jev's usage, summed.

## Events

Each nested call has `ToolStart` and `ToolEnd` with
`parent: Some(<the codemode call's id>)`. Nested ids are
`<parent>/<n>`, numbered from 1 in the order the loop receives the
calls. Its updates are `ToolUpdate`s under its own id, with the same
`parent`. The interface puts them inside the codemode card, live.

## Tests

Port pi's (listed in the research), with property tests where a rule
holds for every input:

- **Renderer:** every schema renders to text Luau parses
  (`Lua::load(...).into_function()` on a `local x: T`); objects keep
  every property; required fields have no `?`; recursion terminates.
- **Value mapping:** JSON → Lua → JSON is the identity for every JSON
  value, `null` and empty arrays included.
- **Truncation:** under budget the text is unchanged; over it, the
  head and tail are prefixes and suffixes of the full text, at char
  boundaries, within the budget.
- **Store fold:** folding records equals applying the writes in order;
  malformed records are skipped; a failed script writes nothing.
- **Options line:** parse ∘ print is the identity; unknown keys fail.
- **Sandbox, as examples:** return values; errors with line numbers;
  `pcall` of a failing tool; parallel calls overlap in time; a tight
  loop times out; cancel stops a tight loop; the memory limit; no
  `io`, `os.execute`, `require`; globals are read-only; calls still
  running at the end are cancelled.
- **In the loop** (`tau-testing::ScriptedModel`): a codemode call
  makes one transcript result however many nested calls it makes; a
  plugin's `before_tool` blocks a nested call; Jev's usage reaches the
  run's total; the store holds across runs and follows forks.
