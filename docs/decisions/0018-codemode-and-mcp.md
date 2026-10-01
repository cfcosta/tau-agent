# 0018: MCP servers and Codemode, as two plugins

- Status: accepted. Built: tau-agent's part (nested calls, exposure,
  structured output, tool sources, per-run tools), and both plugins
  with their interfaces, `tau-codemode` ([codemode.md](../reference/codemode.md))
  and `tau-mcp` with its Servers page ([mcp.md](../reference/mcp.md)).
  Amends
  [0005](0005-plugins.md): tools a plugin adds can be called by other
  tools, and a plugin can add tools for one run.
- Date: 2026-10-01

## Context

pi took MCP into its core and added Codemode next to it
([post](https://earendil.com/posts/you-said-no-mcp/), audited in
[research/pi-codemode.md](../research/pi-codemode.md)). Codemode is a
tool whose input is a script. The script runs in a sandbox on the
harness side and calls other tools as functions: it chains them, runs
them in parallel, and filters large results before the model reads
them. pi's sandbox is QuickJS compiled to WASM.

tau has neither. Its tools are called only by the model, one batch per
turn, and every result goes into the transcript.

## Decision

### Two plugin crates

- **`tau-mcp`** (`crates/plugins/mcp`): connects to MCP servers and
  adds their tools. Its reference is [mcp.md](../reference/mcp.md).
- **`tau-codemode`** (`crates/plugins/codemode`): the `codemode`
  tool, a Luau sandbox that calls any callable tool, MCP tools
  included, and asks Jev. Its reference is
  [codemode.md](../reference/codemode.md).

Codemode is useful without MCP: it drives `read`, `grep` and `bash`,
and asks Jev. MCP is useful without Codemode: its tools are ordinary
tools. Each crate is a `UiPlugin`
([0017](0017-plugins-bring-their-ui.md)).

### MCP tools are ordinary tools

Every MCP tool is declared to the model by default, like `read`, and
scripts can call it too. A server or a tool can instead be
`codemode` (scripts only, not declared) or `hidden`. pi defaults to
`codemode`; tau defaults to `direct`.

pi's `deferred` exposure and its `tool_search` tool are left out. A
run's tools are fixed for the whole run
([openai-websocket.md](../reference/openai-websocket.md)), so a tool
cannot be loaded in the middle of one. A tool a server adds during a
run is declared from the next run on, and scripts can call it at once.

### Lua, as Luau, through mlua

Scripts are Luau, run by `mlua` with its `luau`, `async`, `send` and
`serde` features ([research](../research/codemode-libraries.md)):

- `Lua::sandbox(true)` is a real sandbox, and Luau's standard library
  has no `io`, `os.execute`, `require` or `package`.
- `set_memory_limit` caps memory and `set_interrupt` enforces the
  deadline and the cancel. A loop with no call in it still stops.
- Luau parses type annotations and ignores them at run time. Tools are
  shown to the model as typed Luau signatures, and a script the model
  annotates still runs.
- mlua builds Luau from vendored C++. Plain cargo in `nix develop`
  works with the stdenv compiler; no flake change.

QuickJS (`rquickjs`), `boa`, `rhai` and `piccolo` were set aside: the
first two are JavaScript, `rhai` has no async, and `piccolo` has had no
release since 2024.

### The MCP client is `rmcp`

The official Rust SDK, 3.5, without default features, with `client`,
`transport-child-process`, `transport-streamable-http-client-reqwest`
and `reqwest-tls-no-provider`. That last one is the reqwest and rustls
setup tau already uses. rmcp's API moves between majors, so tau-mcp
keeps it behind one module.

### Servers come from files and from the interface

`tau-mcp` reads `mcpServers` from the user's `~/.config/tau/mcp.json`
and from the repository's `.tau/mcp.json`. Its page in tau-ui adds,
edits and removes the user's servers too. A repository's server runs a
command or reaches a URL that a commit chose, so it waits for the
user's approval in the interface before it connects.

### tau-agent gains nested calls

Codemode needs four things from the core, which other plugins can use
too:

1. **Nested calls.** `ToolCtx::call(name, args)` runs a tool from
   inside another, through the loop: lookup, argument repair,
   validation, every plugin's `before_tool` and `after_tool_result`,
   and events with the parent call's id. A constitution rule checks a
   `bash` a script runs exactly as it checks one the model runs. The
   result goes back to the calling tool, not into the transcript.
2. **Exposure.** `AgentTool::exposure()`: `Direct` (declared and
   callable from tools), `Nested` (callable from tools only) or
   `ModelOnly` (declared, never callable from tools, as `codemode`
   itself is, so a script cannot start a script).
3. **Structured output.** `ToolOutput::structured` and
   `AgentTool::output_schema()`. Scripts get the structured value; the
   model gets the content.
4. **A plugin's tools reach its run.** `ToolCtx::plugin()` gives a
   tool the `PluginCtx` of the plugin that added it, for the run: it
   charges Jev's usage and reads and writes Codemode's store records.

`tau-mcp` also needs per-run tools: `RunPlan::add_tool` in
`Plugin::start`, for the servers' direct tools, which are known only
once the servers have answered. This answers the open question in
[plugins.md](../reference/plugins.md): a run's tools are fixed once it
starts, so adding them in `start` costs nothing against the delta
rule. `RunPlan::tools()` lists the run's tools for Codemode's
signatures.

## Consequences

- A turn's tool calls stay one batch, but a codemode call can make
  hundreds of calls inside it. The transcript holds one result for it;
  the nested calls are in its `details`, and the interface shows them
  on its card.
- Hooks see nested calls with a parent id, so a plugin that keeps a
  per-call ledger (fast compaction) must ignore those, since they never
  reach the transcript.
- Every plugin crate that uses `tau-codemode` compiles Luau's C++ once.
- The phone builds Luau too, since tau-ui depends on `tau-codemode`.
  The NDK's clang++ compiles it, and `luau0-src` links it against the
  NDK's `c++_shared`. The APK carries `libc++_shared.so`: the Nix
  build (`nix build .#tau-phone-apk`) copies it from the NDK, and the
  `android` dev shell has cargo-ndk copy it into jniLibs
  (`CARGO_NDK_LINK_LIBCXX_SHARED`). The engine is in the phone's library though the
  phone never runs a script; leaving it out with a feature is open, as
  in decision 0017.
- MCP servers are processes and connections owned by the agent, not by
  a run: they outlive runs and close with the agent. In tau-ui the host
  keeps them: the user's servers once for every repository, a
  repository's own per repository, and closes them when it goes.
- Codemode's input is JSON `{ "code": string }`. pi constrains it with
  a Lark grammar so the model writes raw source; tau-ai has no custom
  tools yet, and that waits for them.
- Servers' resources reach the model and scripts through three tools
  of tau-mcp's, as pi and Codex have them (`list_mcp_resources`,
  `list_mcp_resource_templates`, `read_mcp_resource`), exposed as the
  widest server that offers resources. Reading is read-only, so a read
  is tried twice; tool calls still never are. MCP apps' resources are
  left out, as pi does.
- Servers' prompts are the person's, not the model's: composer
  commands, `/mcp__<server>__<prompt> key=value ...`, whose messages
  fill the composer. tau-ui-plugin gained commands a plugin lists from
  its data (`Manifest::listed_commands`) for them.
- HTTP servers sign in with OAuth when they ask (a 401), as pi's do:
  rmcp's `auth` does the protocol (discovery, registration, PKCE,
  refresh, more scopes), tau the loopback the browser returns to, the
  grants file `~/.config/tau/mcp-auth.json` and its own TLS. A server
  that asks waits in `needs-auth`; only the user's Sign in on the page
  opens a browser, since a run must never pop one up. Not
  `auth-client-credentials-jwt`: it pulls aws-lc-rs.
- Left for later: pi's `auth.provider` (a `/login` provider's token for
  a server), sampling and elicitation, resource subscriptions
  (`resources/subscribe`), and prompt argument completion
  (`completion/complete`).
