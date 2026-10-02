//! The `codemode` tool's description and argument text. Fixed for the
//! agent: it never lists tools, so the prompt cache holds across runs
//! and as servers connect.

/// The plugin's name, under which its store records are kept.
pub const PLUGIN: &str = "tau-codemode";

/// The tool's name.
pub const NAME: &str = "codemode";

/// The description of the `code` argument.
pub const CODE_DESCRIPTION: &str = "Luau source. The script is the body of a \
function: `return` works. May start with a `-- @options: \
{\"max_output_tokens\": 1000}` line.";

/// pi's `DESCRIPTION_INTRO`, rewritten for Luau.
pub const INTRO: &str = r#"Run Luau code to orchestrate and compose tool calls
- Evaluates the provided Luau code in a fresh sandbox as the body of a function: `return` works.
- Every callable tool is a function on the global `tools` table, for example `tools.read({ path = "a.txt" })`. MCP tools are named like `tools.mcp__linear__list_issues`.
- Tool functions take one table as their argument.
- Tool functions return a table or a string, as their signature says.
- A tool call that fails, is blocked, or gets invalid arguments raises an error carrying the tool's error text. Use pcall to catch it.
- Calls inside `parallel(f1, f2, ...)` run at the same time; it returns each function's result in order. `parallel_settled` returns { ok, value | error } for each instead of raising.
- `map(items, fn, concurrency?)` maps a marked array with at most `concurrency` running callbacks (default 4, range 1..32), passing `(item, index)`. It returns ordered `{ ok = true, value = ... }` or `{ ok = false, error = string }` records. `nil` becomes `json.null`; only the first callback return is used. At most 10000 items.
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
- `json.encode(value)` returns compact JSON; `json.decode(text)` parses JSON. Each text is at most 1 MiB. Decode refuses integer literals outside ±2^53 instead of rounding them. Failures raise string errors.
- `return value` appends the value like `text`."#;

/// The `Jev:` section, when the plugin has a Jev.
pub const JEV: &str = r#"Jev:
- `jev` asks Jev, a classifier model, typed questions about a state. `state` is any JSON value; `question` is the instructions. Each call is one request; at most 4 run at once. A failed request raises an error.

```luau
type Probabilities = { [string]: number }
jev.noul(args: { state: any, question: string, yes: string?, no: string? }): { probability: number }
jev.choice(args: { state: any, question: string, options: { [string]: string } }): { choice: string, confidence: number, probabilities: Probabilities }
jev.score(args: { state: any, question: string, levels: { string } }): { score: number, confidence: number, probabilities: Probabilities }
jev.ask(args: { state: any, questions: { [string]: { kind: "noul" | "choice" | "score", question: string, yes: string?, no: string?, options: { [string]: string }?, levels: { string }? } } }): { [string]: any }
```
- `choice` takes 1 to 255 options; `score` takes 2 to 10 levels, lowest first, and its `score` is 0 for the lowest level.
- `jev.ask` asks several questions in one request and returns each answer under its id."#;

/// Said when the plugin has no Jev.
pub const NO_JEV: &str = "Jev is not available in this run: `jev` is nil.";

/// The closing paragraph.
pub const NOT_DECLARED: &str = "Some tools may not be declared to you, such as MCP tools that are only for scripts. They are still callable on `tools` and listed in `ALL_TOOLS`. Find one with `search_tools(query)`, and read a server's instructions with `describe_namespace(name)`.";

/// The tool's whole description.
pub fn description(has_jev: bool) -> String {
    let jev = if has_jev { JEV } else { NO_JEV };
    format!("{INTRO}\n\n{jev}\n\n{NOT_DECLARED}")
}
