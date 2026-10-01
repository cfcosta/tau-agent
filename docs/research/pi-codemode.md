# pi's MCP and Codemode

- Status: research, for [0018](../decisions/0018-codemode-and-mcp.md).
- Date: 2026-10-01

- Repo: https://github.com/earendil-works/pi
- Commit audited: `8ce69e9d2b171d173fe4b6b2b6256f1f4411e69d` (2026-10-01, "fix(mcp): treat empty and null optional OAuth fields as absent"), package version 0.99.2
- Post: https://earendil.com/posts/you-said-no-mcp/ (2026-09-29)
- All paths below are relative to `pi/packages/`.

## 0. What the post says (beyond the summary)

- MCP moved into core because the same machinery (an interpreter sandbox and tool metadata for deferred, codemode-only and direct tools) also serves Jev (a classifier model) and other things.
- "MCP should be much closer to OpenAPI with intelligent tool discovery": tools should return **structured data** and be **discoverable by documentation/description**, not dumped into context.
- "In a Codemode world one needs to decide if the tool is available to the LLM or only the codemode part of the LLM." Hence the per-tool exposure metadata (`direct` / `deferred` / `codemode` / `hidden`).
- Codemode "runs where the harness runs" (trusted side, not where bash runs). "Because Codemode also runs on the harness side, its state is also maintained as part of the session transcript instead of the file system."
- JS was chosen because "small versions of JavaScript can be shipped as WASM binaries and allow reasonable levels of protection."
- Codemode is loaded automatically when MCP is configured, or added via `defaultTools`.
- The example script uses `tools.mcp__linear__list_issues(...)`, `models.getModelOfType(...)`, `models.classify(...)`, a hand-rolled 4-worker pool over `Promise.all`, `store("frustration", results)`, and `return {...}`. The UI shows a log of nested calls (`✓ mcp__linear__list_comments {"issueId":"PI-4714"} 255ms`, "... (331 earlier calls)"), then the returned JSON. The final assistant text says "The per-issue verdicts are stored in codemode under frustration, so I can dig into any of them without fetching the issues again."

## 1. Package layout

| Package                                      | Role                                                                                                                                    |
| -------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `mcp/` (`@earendil-works/pi-mcp`)            | Homegrown MCP client (no official SDK). Only dependency: `cross-spawn`. OAuth is adapted from the MIT TS SDK v1.29.0 (`mcp/LICENSES/`). |
| `codemode/` (`@earendil-works/pi-codemode`)  | Standalone sandbox: QuickJS-in-WASM in a worker thread. Only dependency: `quickjs-wasi@3.6.2`. No pi deps.                              |
| `coding-agent/src/extensions/mcp/`           | Built-in MCP extension: config, connections, tool registration, `/mcp` UI, resources, OAuth glue.                                       |
| `coding-agent/src/extensions/codemode/`      | Built-in `codemode` tool (description, loadout handling, execute, renderer).                                                            |
| `coding-agent/src/extensions/tool-search/`   | BM25 ranker + `tool_search` tool (shared with codemode `searchTools()`).                                                                |
| `coding-agent/src/core/nested-tool-calls.ts` | `ctx.executeTool()` runner: nested calls go through the full tool pipeline.                                                             |
| `coding-agent/src/core/mcp-servers.ts`       | Config types, validation, exposure resolution, extension-registered servers.                                                            |
| `agent/examples/mcp-codemode/`               | Minimal example wiring MCP + codemode on `pi-agent-core`.                                                                               |

## 2. MCP configuration

Doc: `coding-agent/docs/mcp.md` (whole file; the best single reference).

### Location and precedence

- User: `~/.pi/agent/mcp.json`; project: `<cwd>/.pi/mcp.json`, read **only if project trust is granted** (`extensions/mcp/config.ts:115-124`). A project entry replaces a user entry of the same name.
- Format is the common `{"mcpServers": {...}}` shape. Extra top-level key: `autoEnableCodemode` (bool, default true; project overrides user) (`config.ts:89-90`).
- Extensions can add servers with `pi.registerMcpServer(name, config)` (`core/mcp-servers.ts:264-303`); a file-configured server with the same namespace wins (`extensions/mcp/index.ts:313-330`).

### Server entry (`core/mcp-servers.ts:24-109`, validation `:205-262`)

Common fields:

- `exposure`: `"codemode"` (default) | `"deferred"` | `"direct"` | `"hidden"`; alias `"codemode-deferred"` → `codemode` (`:17-22`).
- `toolExposure`: `{ "<tool name or glob with *>": exposure }`. Exact name wins, then first matching pattern in object order (`getMcpToolExposure`, `:191-199`).
- `description`: one sentence. Used in the `mcp_servers` system prompt section, in tool-search ranking text, and returned by `describeNamespace()`. Falls back to the first line of server `instructions` once connected.
- `enabled` (default true), `timeout` (seconds, default 60, progress notifications reset it).

Stdio (`command` present, `type` absent or `"stdio"`): `command` (single executable, not a shell string), `args: string[]`, `env: Record<string,string>`, `cwd` (relative to session cwd). `~/` expanded in command/args/cwd (`extensions/mcp/runtime.ts:88-121`).

HTTP (`url` present, `type` absent or `"http"`/`"streamable-http"`): `url` (http/https), `headers`, `oauth` (`clientId`, `clientSecret`, `callbackPort`, `callbackUrl` (loopback only), `scope`, `clientName`, `authServerMetadataUrl`), `auth: { provider }` (send the token of a pi `/login` provider; **global file only**, https or loopback only).

- `type: "sse"` is rejected with a message pointing to streamable HTTP (`:227`).
- `env`/`headers`/`clientSecret` values support `${VAR}` and `!command` (whole value) via `resolveConfigValueOrThrow`.
- OAuth applies to HTTP servers with no `Authorization` header and no `auth` (`runtime.ts:81-86`). Tokens in `~/.pi/agent/mcp-auth.json`. Dynamic client registration, PKCE, refresh, step-up scope.
- Server names: `^[A-Za-z0-9_-]+$`. Names differing only in `-`/`_` clash (same namespace) (`config.ts:97-101`).
- Invalid entries are reported and skipped; others still connect.

### Lifecycle (`extensions/mcp/index.ts`)

- `session_start` (`:945-982`): load config, call `ensureDiscoveryActive` (activates `codemode` and/or `tool_search` **from config, before any server connects**), then lazy-import the MCP runtime after one `setImmediate` and connect **every enabled server in the background**. Not lazy-per-first-use (the pi-mcp-adapter third-party extension is the lazy one).
- `before_agent_start` (`:1009-1015`): the first prompt waits up to `startupWaitMs` (10 s) **only** for servers that have `direct` tools; then it sets the `mcp_servers` system prompt section. When the section changes later, pi appends the new section to the conversation (a mid-conversation system message) instead of rewriting earlier messages, so the prompt cache stays valid (docs/mcp.md:178).
- `tool_call` hook (`:1020-1036`): before a `codemode` call runs, wait for servers the script needs. `scriptNeedsServer` (`:216-219`) returns true if the source mentions `searchTools|describeNamespace|describeTool|ALL_TOOLS` (then all servers), or contains the literal `mcp__<server>` namespace. `tool_search` and resource tools wait for all pending servers.
- `turn_start`: reconnect servers whose stored OAuth tokens changed (for example `pi mcp login` run in another process).
- `mcp_servers_change`: connect/disconnect servers that extensions register or unregister mid-session.
- `session_shutdown`: close all. Stdio close: close stdin, SIGTERM, then SIGKILL to the process group.
- `McpServerConnection` (`runtime.ts:155-473`): states `connecting|connected|disconnected|needs-auth|failed|closed`. Reconnects lazily on the next call after a drop. HTTP connect retries on transient errors (408/429/5xx except 501, TypeError) with delays [250, 1000] ms. Read-only requests (resources) retry once; **tool calls are never retried**. `McpSessionExpiredError` (HTTP 404 session) retries once on a new session. Handles `notifications/tools/list_changed` (re-list, re-register) and `resources/list_changed`. Sends `roots` = session cwd. Logs `notifications/message` to `~/.pi/agent/mcp.log` (rotates at 5 MB).
- Withdrawn tools cannot be unregistered, so they are re-registered with exposure `hidden` (`index.ts:393-398`).

### Transport library

`@earendil-works/pi-mcp` (`mcp/src/`): `client.ts`, `transports/stdio.ts`, `transports/streamable-http.ts`, `transports/in-memory.ts`, `oauth/*`. Protocol `2025-11-25`, accepts `2025-06-18`, `2025-03-26`, `2024-11-05`. Supports paginated `tools/list`, `tools/call` with structured content, progress + timeout renewal, cancellation, HTTP sessions with GET stream and `Last-Event-ID` resume, server `ping`/`roots/list`. Out of scope: batch JSON-RPC, legacy SSE, sampling, elicitation, tasks, prompts.

## 3. How MCP tools reach the model

### Naming (`extensions/mcp/tools.ts:281-290`)

`mcp__<server>__<tool>`, every char outside `[A-Za-z0-9_]` → `_`. If longer than 64 chars or colliding, it is truncated and gets `_<sha256(server\0tool)[0..8]>`. All tools whose sanitized names collide get the hash (order-independent, like Codex) (`index.ts:360-377`). Namespace = `mcp__<server with - → _>` (`core/mcp-servers.ts:114-116`). The name is also the JS identifier.

### Exposure semantics (docs/mcp.md:165-204; `core/extensions/types.ts` `ToolExposure`)

| MCP exposure         | pi ToolExposure                                        | Declared to model            | Callable from codemode | Listed in codemode description                                                                                             |
| -------------------- | ------------------------------------------------------ | ---------------------------- | ---------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `codemode` (default) | `deferred` (sic, `toToolExposure`, `tools.ts:238-240`) | no                           | yes                    | no, found via `searchTools()` / `ALL_TOOLS`                                                                                |
| `deferred`           | `deferred`                                             | after `tool_search` loads it | yes                    | no                                                                                                                         |
| `direct`             | `direct`                                               | yes                          | yes (while active)     | in mode `on`: appended to the tool's own description; in mode `only`: listed in codemode, declaration hidden from requests |
| `hidden`             | `hidden`                                               | no                           | no                     | no                                                                                                                         |

The two MCP exposures `codemode` and `deferred` map to the same pi exposure. They differ only in which discovery tool the MCP extension activates (codemode vs tool_search). Callable tools = active `direct` tools + every registered `codemode`/`deferred` tool (`core/agent-session.ts` `_getCallableTools`). Codemode calls therefore do not depend on the active set and survive `/tree`, resume, fork.

pi `ToolExposure` also has `model-only`: declared, never callable from scripts. `codemode` and `tool_search` themselves use it, so scripts cannot start scripts.

### System-prompt server list (`index.ts:148-210`)

Section `<mcp_servers>`, capped at 4096 chars; each description at most 250 chars, shrunk to fit; the last servers are dropped with "- … N more servers; find their tools with searchTools()". Format: `- mcp__linear (codemode): <first line of description/instructions>`. Verbatim intro:

> MCP servers whose tools are not declared to you. Call the tools of `codemode` servers from codemode scripts: find them with `searchTools(query, { namespace })` and read a server's instructions and tool names with `describeNamespace(name)`. Load the tools of `tool_search` servers with `tool_search`.

Server `instructions` (from `initialize`) are **never** put in any tool description. Only `describeNamespace()` returns them.

### Tool definition (`tools.ts:453-525`)

- `description`: server description → title → `MCP tool <t> from server <s>`.
- `parameters`: the server's `inputSchema`, with `type: "object"` and `properties: {}` added if missing.
- `outputSchema`: always a `CallToolResult` wrapper schema `{content: array<object>, structuredContent?: <tool outputSchema>, isError: boolean, _meta: object}` (`createMcpResultSchema`, `:304-315`). Codemode detects this shape to render `Promise<CallToolResult<T>>`.
- `annotations`: the boolean MCP hints (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) for permission extensions.
- Progress notifications → `onUpdate` partial results.

### Results (`tools.ts:321-427`)

- **Model-facing** (direct calls): content blocks → text/images. Resource links become `[Resource <uri> "<title>" (mime, size): desc. Read it with read_mcp_resource (server "x")]`. Non-image binary embedded resources are saved to a 0600 temp file and the path is reported. Text blobs are decoded. With no content blocks, `structuredContent` is rendered as pretty JSON. Text over **20 KB** is middle-truncated (Codex format: `Warning: truncated output (original token count: N)\nTotal output lines: L\n\n<head…tail>\n\n[Full output: /tmp/pi-mcp-xxx.txt (read it with offset/limit)]`). `isError` → error result; if it has no text, `MCP tool s/t returned an error` is added.
- **Script-facing**: `structuredContent` of the pi tool result = the whole `CallToolResult` minus `_meta` (`content` as sent, `structuredContent`, `isError`), **never truncated**. A result with `isError` **resolves** inside scripts (the script checks `isError`); it is not rejected.

### Resources (`extensions/mcp/resources.ts`)

Codex-compatible tools `list_mcp_resources`, `list_mcp_resource_templates`, `read_mcp_resource`. They take the widest exposure among servers with resources. `ui://` and `text/html;profile=mcp-app` resources are omitted. Scripts receive `{ server, uri, contents }`.

### `tool_search` (`extensions/tool-search/tool.ts`)

- Schema: `{ query: string, limit?: number }` (default 8). Exposure `model-only`.
- BM25 (k1 1.2, b 0.75) over the document = name, name with `_` → space, description, recursive schema property names and descriptions, namespace name/description/instructions. Tokenizer splits camelCase and non-alphanumerics, drops ~20 stop words, applies naive plural stemming (`:39-157`).
- Loads the matches with `setActiveTools`, so they are declared from the next model call. This is recorded in the transcript and persists on that branch.
- Verbatim description (`:220`):

> # Tool discovery
>
> Searches over deferred tool metadata with BM25 and exposes matching tools for the next model call.
>
> Some of the tools, such as tools of MCP servers, may not have been provided to you upfront, and you should use this tool (`tool_search`) to search for the required tools. For MCP tool discovery, always use `tool_search`.

- promptSnippet: "Search for tools that are not loaded yet and load the matches".
- Result text: `Loaded N tools. They are available from your next call:\n- name: first line of description` or `No matching tools found.`

## 4. Codemode

### 4.1 Tool identity (`coding-agent/src/extensions/codemode/tool.ts`)

- Name `codemode` (`:48`). It is registered inactive (`defaultActive: false`, `index.ts:31-44`). It is activated by `--tools`, `defaultTools: ["+codemode"]`, or the MCP extension when a `codemode`-exposure server is configured and `autoEnableCodemode` is not false.
- Exposure `model-only` (`:424`): scripts cannot call codemode.
- Input schema (`:85-90`), verbatim:
  ```ts
  Type.Object({
    code: Type.String({
      description:
        'Raw JavaScript source. Top-level await and return work. May start with a `// @options: {"max_output_tokens": 1000}` line.',
    }),
  });
  ```
- `constrainedSampling: { type: "grammar", variants: { openai_lark: CODEMODE_SOURCE_GRAMMAR } }` (`:427`). For OpenAI-style custom/grammar tools, the model emits raw source and not a JSON-escaped string. Grammar (`codemode/src/source.ts:22-30`):
  ```
  start: options_source | plain_source
  options_source: OPTIONS_LINE NEWLINE SOURCE
  plain_source: SOURCE
  OPTIONS_LINE: /[ \t]*\/\/ @options:[^\r\n]*/
  NEWLINE: /\r?\n/
  SOURCE: /[\s\S]+/
  ```
- System prompt contribution (`:124-129`), verbatim:
  - snippet: `Run JavaScript that calls other tools (chains, loops, Promise.all, filtering large results)`
  - guideline: `Use codemode to batch or chain several tool calls, or to filter large tool output down to what you need, instead of issuing many individual tool calls. Batch independent calls in one codemode call using await Promise.allSettled([...]).`
- Settings (`core/settings-manager.ts:95-108`): `codemode.mode` = `"on"` (default) | `"only"`; `codemode.inlineBudget` (estimated tokens, default 3000).

### 4.2 Description (verbatim)

`DESCRIPTION_INTRO` (`tool.ts:131-157`):

```
Run JavaScript code to orchestrate/compose tool calls
- Evaluates the provided JavaScript code in a fresh QuickJS sandbox as the body of an async function: top-level `await` and `return` work.
- All nested tools are available on the global `tools` object, for example `await tools.read(...)`. Tool names are exposed as normalized JavaScript identifiers, for example `await tools.mcp__ologs__get_profile(...)`.
- Nested tool methods take an object as their input argument.
- Nested tools return either an object or a string, based on the description.
- A nested tool call that fails, is blocked, or gets invalid arguments rejects with an Error carrying the tool's error text.
- Runs raw JavaScript -- no Node, no file system, no network access, no timers.
- Accepts raw JavaScript source text, not JSON, quoted strings, or markdown code fences.
- You may optionally start the tool input with a first line like `// @options: {"max_output_tokens": 1000, "timeout_ms": 60000}`.
- `max_output_tokens` sets the token budget for the script's output. Defaults to 10000 tokens.
- `timeout_ms` sets a hard deadline for the whole script. By default there is none.
- When the JS code is fully evaluated, calls that are still running are cancelled and unawaited promises are silently discarded.
- Tool calls are real and have side effects. If the script fails partway, earlier calls are not undone.
- Scripts have a 256 MB memory limit; exceeding it throws `InternalError: out of memory`. Filter or aggregate large data instead of accumulating it.

- Global helpers:
- `exit()`: Immediately ends the current script successfully (like an early return from the top level).
- `text(value: string | number | boolean | undefined | null)`: Appends a text item. Non-string values are stringified with `JSON.stringify(...)` when possible.
- `image(imageUrlOrItem: string | { image_url: string } | ImageContent)`: Appends an image item. `image_url` should be a base64-encoded `data:` URL. To forward an MCP tool image, pass an individual `ImageContent` block from `result.content`, for example `image(result.content[0])`.
- `store(key: string, value: any)`: stores a serializable value under a string key for later `codemode` calls in the same session. Storing `undefined` deletes the key. Writes are kept only if the script succeeds.
- `load(key: string)`: returns the stored value for a string key, or `undefined` if it is missing.
- `ALL_TOOLS`: metadata for the enabled nested tools as `{ name, description }` entries.
- `searchTools(query: string, options?: { limit?: number; namespace?: string })`: resolves to the nested tools that best match the query (BM25, default limit 8), as `{ name, description }` entries like `ALL_TOOLS`.
- `describeTool(name: string)`: resolves to the description and declaration of a nested tool, or `undefined`.
- `describeNamespace(name: string)`: resolves to `{ name, description?, instructions?, tools }` for a namespace of nested tools, such as an MCP server: its usage instructions and the names of its tools, or `undefined`.
- `console.log(...)` and the other `console` methods append a text item like `text()`.
- `return value` at the top level appends the value like `text()`.
```

`DEFERRED_TOOLS_GUIDANCE` (`:220-221`):

```
Some nested tools may be omitted from this description, such as deferred tools and MCP tools. They are still available on the global `tools` object and listed in `ALL_TOOLS`.
To find one, call `await searchTools(query)` (pass `{ namespace }` to search one namespace), or filter `ALL_TOOLS` by `name` and `description`. `await describeNamespace(name)` returns a namespace's usage instructions and the names of its tools.
```

Then, conditionally (`createCodemodeDescription`, `:307-364`):

- `Shared MCP Types:` + a ```ts block with `MCP_TYPESCRIPT_PREAMBLE` (`codemode/src/declarations.ts:18-93`: `Role`, `Annotations`, `TextContent`, `ImageContent`, `AudioContent`, `ResourceLink`, `EmbeddedResource`, `ContentBlock`, `CallToolResult<TStructured>`). Included only if a **listed** tool has an MCP output schema.
- `Model API:` with `ModelType`, `ModelInfo`, `ClassifierQuestion/Answer/Context/Result` (`tool.ts:159-193`), plus `declare const models: { getModelsOfType, getAvailableOfType, getModelOfType, classify }` (`:196-218`).
- `Nested tools:` then, per group (no-namespace first, then namespaces alphabetically): `## mcp__x[ (tools not listed)| (some tools not listed)]\n<namespace description>`, and per tool `### \`id\` (\`raw\`)`+`renderToolSample`= description + "codemode tool declaration:" + ```ts`declare const tools: { name(args: T): Promise<R>; };` ```.
- **Budget** (`selectCatalog`, `:281-298`): sections are costed at chars/4. Round-robin across groups, cheapest first; a group drops out when its next tool doesn't fit. Every namespace is represented before any is complete.
- **Stability**: `deferred` (this includes every MCP `codemode`-exposure tool) is never listed, so the description does not change while servers connect. This keeps the prompt cache warm. (Test: "keeps the codemode description unchanged when the server connects".)
- `prepareLoadout` (`:376-410`) recomputes the description when the active set changes. Mode `on`: active direct tools that are callable get their codemode declaration appended to their **own** description, and codemode lists only non-direct callable tools. Mode `only`: codemode lists all callable tools, and the declarations of active direct tools are hidden from requests (`hiddenDeclarations`) but stay in the transcript.

### 4.3 TS declarations from JSON Schema (`codemode/src/declarations.ts`)

`schemaToType` (`:224-351`): const/enum → literal unions; anyOf/oneOf → union; allOf → intersection; arrays → `Array<T>` or tuples; objects on one line with sorted props and `?` for optional; multi-line with `// description` comments when any property has a description; `additionalProperties` → index signature; local `$ref` (`#/$defs`, `#/definitions`) expanded (max 32 expansions; recursive or remote → `unknown`). An input type over 16,000 chars → `unknown`. Doc comments escape `*/`.

### 4.4 Engine and sandbox (`codemode/src/runtime/`)

- Engine: **`quickjs-wasi` 3.6.2** (QuickJS compiled to WASI wasm). Not quickjs-emscripten. The wasm is compiled once per path and cached (`codemode/src/wasm.ts`). A bundled build passes `getQuickJSWasmPath()` and a worker specifier (`coding-agent/src/config.ts:488-512`).
- **One fresh `worker_threads` Worker + fresh VM per `execute()`** (~20 ms) (`host.ts:81-86`). Rationale: QuickJS is synchronous; a spinning script must not block the host event loop; `terminate()` gives clean kills.
- VM creation (`worker.ts:54-63`): `memoryLimit` (pi passes **256 MiB**, `execute.ts:48`), `maxStackSize: MAX_STACK_SIZE` (deep recursion → catchable `RangeError` instead of a wasm trap), `interruptHandler: () => Atomics.load(interrupt,0) !== 0` (a SharedArrayBuffer flag set by the host before `terminate()`; needed on Bun, where terminate cannot stop a wasm spin loop), and WASI `fd_write` replaced by a sink that discards engine stdout/stderr.
- Timeout: the sandbox default is 300 s, but **pi passes `Infinity` unless `// @options: {"timeout_ms": N}`** (`execute.ts:277`). Abort comes from the tool call's `AbortSignal` (user Esc).
- Imports into the VM: only the WASI shim (clock, random, discarded writes) and one host function `bridge`. No timers, fetch, process, require, modules, `WebAssembly`; dynamic `import()` rejected; `eval`/`Function` work but stay in the VM (tests `sandbox.test.ts:589-637`).
- **Deadlock detection**: after draining jobs, `stalled()` fails the script if it is unfinished and no host call is pending (`prelude-source.ts:331-342`). Verbatim message: `The script is waiting on a promise that can never settle: no tool call is pending, and timers do not exist here.`

### 4.5 Prelude and globals (`codemode/src/runtime/prelude-source.ts`)

Evaluated in the VM before the script. It keeps `bridge` in a closure so the script cannot reach it. It caches `JSON.stringify/parse`, `Promise.prototype.then` and `Error` against tampering.

- `tools`: frozen null-proto object. Each tool appears under both its `jsName` (identifier-normalized) and its raw name. The first wins on identifier collision.
- `ALL_TOOLS`: frozen `[{ name: jsName, description }]`. In pi, `description` = the full `renderToolSample` (description + TS declaration) (`execute.ts:239-241`).
- `text`, `image`, `exit`, `console.{log,info,warn,error,debug}`, `store`, `load`, and host "globals" (`searchTools`, `describeTool`, `describeNamespace`, `models.*`). Globals named `a.b` are grouped into a frozen namespace object. Names are validated, and reserved names are refused (`host.ts:24-34, 295-314`).
- The script is compiled as `(async (tools, console) => {<code>\n})` on line 1 so line numbers match (`worker.ts:146`). Errors are formatted V8-style `Name: message` + QuickJS frames, minus prelude frames (`codemode.js:3:...`).
- `image()` accepts a data URL, `{image_url}`, or an MCP `ImageContent`. It rejects http(s) URLs, validates base64, and **sniffs the magic bytes** (PNG/JPEG/GIF/WebP) and uses the detected MIME, because providers reject mismatches and a bad image would be resent every turn (`:226-259`).
- `exit()` reports success with the store writes, then throws a frozen sentinel to unwind.

### 4.6 Host bridge and parallelism

- Wire format (`protocol.ts`): worker→host `call{id,target:"tool"|"global",name,args(JSON string)}`, `output{item}`, `done{ok,value|error,writes}`, `crash`; host→worker `result{id,ok,payload}`. All values cross as JSON strings.
- Calling `tools.x(args)` creates a Promise, stores `{resolve,reject}` by id, and posts `call` (`prelude :86-100`). The host runs `tool.execute(args,{signal})` asynchronously, **without awaiting other calls**, so `Promise.all` gives true host-side concurrency (`host.ts:210-240`). On reply the worker calls `settle(id, ok, payload)` and then `drain()` (`executePendingJobs` + stall check).
- Errors: a host throw → `reject(new Error(message))` inside the script (catchable).
- When the script settles, `finish()` aborts every pending call's AbortController (calls are recorded as `cancelled`), sets the interrupt flag, and terminates the worker (`host.ts:242-273`). Unawaited promises are discarded.
- In pi, each sandbox tool's `execute` calls **`ctx.executeTool(name, args, {signal})`** (`execute.ts:240-267`). This goes to `NestedToolCallRunner` (`core/nested-tool-calls.ts`) → `runToolCall` with the session's `beforeToolCall`/`afterToolCall`. So validation, `tool_call`/`tool_result` extension hooks and permission gates apply exactly as for model calls. Nested ids are `<parentId>/<n>`, and `tool_execution_*` events carry `parentToolCallId`.
- Concurrency limits: nested calls are serialized through a queue if the agent runs tools sequentially or the tool has `executionMode: "sequential"` (`nested-tool-calls.ts:201-212`; a call already holding the queue doesn't deadlock its own nested calls). `models.classify` is limited to 4 in flight (`execute.ts:42, 89-103`). There is no other cap.
- **Built-in tools are callable from codemode**: any active `direct` tool (`read`, `bash`, `edit`, `write`, ...) plus every `codemode`/`deferred` tool. Built-ins without `outputSchema` resolve to their joined text. `bash` declares an `outputSchema` (`core/tools/bash.ts:56-65`: `{output, truncated, full_output_path?, exit_code, wall_time_seconds}`), so scripts get a structured result even for non-zero exit codes (test `agent-session-codemode.test.ts:453`).
- Value mapping (`toScriptValue`, `execute.ts:205-211`): if the tool has `outputSchema` and the result has `structuredContent` → resolve to it (even for error results that carry it, for example MCP `isError`). Otherwise → text. If `isError` without structured content → reject with the error text.

### 4.7 Output capture and result format (`execute.ts:217-320`)

- Output items (text/image) are kept in emission order. A successful `return value` is appended as text (`valueText`: strings as-is, else compact JSON). On failure, `Script error:\n<head>\n\n<call summary>` is appended, where head = stack | `Script timed out: …` | `Script aborted: …` | `Script sandbox failed: …`. The call summary is `Tool calls made before the failure (they are not undone): echo (ok), …` or `No tool calls were made.`
- Token budget: `max_output_tokens` (default 10,000; chars/4). Over budget, the text items are merged and head/tail kept (half each) with `Warning: truncated output (original token count: N)\nTotal output lines: L\n\n<head>…N tokens truncated…<tail>\n\n[Full output: /tmp/pi-codemode-<hex>.txt (read with offset/limit)]`. Images follow.
- Final content: `[{text: "Script completed|Script failed\nWall time X.X seconds\nOutput:\n"}, ...items]`, `isError` on failure, `details.calls` (nested call rows: id, name, args preview ≤200 chars, status, duration, error ≤500, cost), `usage` = summed classifier usage.
- Source errors (empty input, bad `@options`, unknown option keys, options line without code) throw `CodemodeSourceError` before execution and become a plain error result with just the message (for example ``@options only supports `max_output_tokens` and `timeout_ms`; got `yield` ``).
- `timeout_ms` must be a positive integer ≤ 2^31-1. `max_output_tokens` must be a non-negative safe integer.

### 4.8 "State in the session transcript"

There is **no VM persistence or replay**. Every call is a fresh VM. State has three parts:

1. **`store`/`load`**. The prelude holds a snapshot `key → JSON text`; `store`/`load` are synchronous. Limits: 256 Ki chars per value, 1 Mi total, keys must be strings, `undefined` deletes, `load` returns a fresh copy. On a **successful** run only, the writes `{set, delete}` are appended to the session as a **custom entry `customType: "codemode-store"`** (`execute.ts:298-302`, via `pi.appendEntry`). Before each run, `readCodemodeStore(sessionManager.getBranch())` folds all such entries from root to leaf (`execute.ts:117-127`), ignoring malformed ones. Because the session is a tree, **each branch sees only the values written on its own path** (test "loads the values written on the current branch"). This is what the post means by state living in the transcript and not on the filesystem.
2. **Nested call record**. Nested calls are **not** transcript tool results. A bounded record (≤256 calls, args ≤8 KiB each and ≤32 KiB total, errors ≤500 chars, `complete` flag) is attached as `nestedCalls` on the codemode tool-result message (`nested-tool-calls.ts:26-101`, `agent-session.ts:1069-1080`), with summed usage. Compaction reads it to compute read/modified file lists (`test/compaction-nested-calls.test.ts`).
3. **Script output** in the tool result content, as usual.

### 4.9 Discovery globals (`execute.ts:337-403`)

- `searchTools(query, {limit=8, namespace})`: the same `Bm25Ranker` and `createToolSearchDocument` as `tool_search`, over callable tools. Returns `{name: identifier, description: sample}`. The namespace filter accepts `mcp__dev-radius`, `mcp__dev_radius`, `dev-radius`, `dev_radius` (`isNamespaceName`, `:326-331`).
- `describeTool(name)`: raw or identifier name → sample string or `undefined`.
- `describeNamespace(name)` → `{ name, description?, instructions?, tools: [identifiers] }`.
- `models.*` (`:410-480`): the catalog with `headers` stripped (they may hold credentials). `classify` resolves the model by `provider`/`id` **from the registry** and ignores script-supplied baseUrl/headers. Provider errors resolve with `stopReason`/`errorMessage`; invalid arguments throw.

### 4.10 Renderer (`codemode/renderer.ts`)

TUI shows the script, live nested-call rows (✓/✗, name, args preview, duration, cost), then output. The "Script completed" header is hidden. Collapsed output is limited by wrapped lines.

## 5. Tests worth mirroring

- `codemode/test/sandbox.test.ts` (~45 cases): return value JSON round trip; top-level await; output ordering; invalid text/image args; wrapped base64; `exit()` keeps output and writes; partial output kept on failure; syntax/throw line numbers; V8-like stacks without prelude frames; non-Error throws; non-serializable return → script error; concurrent calls; identifier normalization; tool errors catchable; unknown tool rejects; unawaited calls aborted on return; store snapshot, copies, invalid/oversized writes, reserved names; globals and namespaced/spread globals; sync infinite loop timeout; Infinity timeout; never-settling promise fails fast; microtask spin timeout; abort cancels in-flight calls; parallel executions isolated; deep recursion → RangeError; missing worker or bad wasm → sandbox error; no host globals; eval stays in VM; dynamic import rejected; frozen tools/console.
- `codemode/test/declarations.test.ts`: schema → TS rendering, refs/recursion, budget → `unknown`, `CallToolResult<T>`, comment escaping.
- `coding-agent/test/suite/agent-session-codemode.test.ts`: parallel nested calls produce exactly 1 transcript toolResult; hooks see nested calls; usage aggregation; structured content replaced by `tool_result` hooks; images only via `image()`; failure keeps partial output + call list; `timeout_ms`; memory limit; output truncation and spill; bash structured; store persistence per branch; store fold ignoring malformed data; `models` auth isolation.
- `coding-agent/test/suite/agent-session-mcp.test.ts` (~35 cases): codemode-only tools hidden from the model but callable; direct model calls to them rejected; survive `/tree`; direct exposure; resources; per-tool overrides; describeNamespace returns instructions; `autoEnableCodemode: false`; reaching deferred tools through codemode; warning when neither discovery tool is active; not activating a foreign tool named `codemode` (identity check by schema reference, `isCodemodeTool`); first prompt not held for codemode servers; description stable on connect; servers section appended later; waits for named servers, including when the identifier differs from the name; no wait for unnamed servers; startupWaitMs cap; codemode cannot call itself or inactive direct tools; searchTools/describeTool; tool_search loads persist on branch.
- `coding-agent/test/mcp-extension.test.ts`: config merge/validation, `-`/`_` clashes, exposure patterns, provider auth only global, tool-name sanitizing/hash, result conversion, 20 KB middle cut at UTF-8 boundaries, session-expired retry, no-tools-capability servers, reconnect after drop, transient HTTP retry (tool calls not retried), OAuth re-sign-in, log file, servers-section sizing.

## 6. Notes for a Lua port in tau-agent (Rust)

These are inferences, not pi facts.

- **Engine**: `mlua` with Luau (sandbox mode, `set_interrupt` for cancellation/deadline, `set_memory_limit`) maps closely to quickjs-wasi's memoryLimit + interruptHandler. Run the VM on a dedicated thread (or `spawn_blocking`) so a spinning script never blocks the tokio runtime. Mirror pi's flag + kill design: an `AtomicBool` checked by the interrupt callback.
- **Async bridge**: Lua coroutines play the part of JS promises. `tools.x(args)` yields a request id to the host scheduler. The host spawns the tool future. A `parallel({...})`/`all(...)` helper (the Lua equivalent of `Promise.all`) starts several calls before yielding. Resume the coroutine when results arrive. Use mlua's `async` feature (`create_async_function`) or a hand-written scheduler. Implement pi's "stalled" check: coroutine suspended with zero pending calls means fail immediately.
- **Values**: pass JSON across the boundary as pi does (serde_json ↔ Lua tables via `mlua::LuaSerdeExt`). Watch array/object ambiguity for empty tables and `null` (pi relies on JSON semantics; use `mlua`'s `array_metatable` / `NULL` sentinel).
- **Contract to copy**: tool name normalization (`mcp__server__tool`, `[A-Za-z0-9_]`, 64 chars + hash); exposure enum and the "callable = active direct ∪ codemode ∪ deferred" rule; structuredContent vs text resolution rule; nested calls through the same permission/hook pipeline with `parent_tool_call_id`; `store`/`load` persisted as transcript entries folded along the branch, written only on success; output token budget with head/tail spill to a temp file; result header and "calls made before the failure (they are not undone)" summary; stable tool description (never list deferred tools) for prompt caching; `mcp_servers` system section appended (not rewritten) on change; BM25 tool search shared by the in-script `search_tools` and the model-facing `tool_search`.
- **Description**: the declarations would need to be Luau type annotations or LuaLS `---@param` style in place of TS. The JSON Schema → type renderer is ~150 lines and ports directly.
