# MCP servers (`tau-mcp`)

- Status: not built. Decided in
  [0018](../decisions/0018-codemode-and-mcp.md).
- Date: 2026-10-01

`tau-mcp` connects to MCP servers and adds their tools to the agent.
By default the model sees them like any other tool, and Codemode
scripts can call them too. It follows pi's MCP extension
(`coding-agent/src/extensions/mcp`, audited in
[research/pi-codemode.md](../research/pi-codemode.md)). The client is
`rmcp` ([research](../research/codemode-libraries.md)).

## Configuration

### Where servers come from

In order, a later entry replacing an earlier one with the same name:

1. the user's file, `~/.config/tau/mcp.json`;
2. the servers added on the plugin's page in tau-ui, saved as the
   plugin's settings. The page refuses a name the user's file already
   has;
3. the repository's file, `<repository>/.tau/mcp.json`.

A repository's server connects only once the user approves it on the
plugin's page. The approval is saved in the plugin's settings with a
hash of the entry, so a commit that changes the entry asks again.
pi reads a project's file only when the project is trusted; tau asks
per server.

### The format

The common `mcpServers` shape:

```json
{
  "mcpServers": {
    "linear": {
      "url": "https://mcp.linear.app/mcp",
      "headers": { "Authorization": "Bearer ${LINEAR_TOKEN}" },
      "description": "Linear issues and projects."
    },
    "git": {
      "command": "uvx",
      "args": ["mcp-server-git"],
      "exposure": "codemode",
      "toolExposure": { "git_push": "hidden" }
    }
  }
}
```

- **Every server:**
  - `exposure`: `direct` (the default), `codemode` or `hidden`;
  - `toolExposure`: tool name or `*` glob to exposure. An exact name
    wins, then the first pattern that matches, in the file's order;
  - `description`: one sentence, for the server list and for search.
    Without one, the first line of the server's `instructions`;
  - `enabled`: default true;
  - `timeout`: seconds per call, default 60. A progress notification
    starts it again.
- **Stdio** (`command`, and `type` absent or `"stdio"`): `command` is
  one executable, not a shell line; `args`; `env`; `cwd`, relative to
  the repository. `~/` is expanded in `command`, `args` and `cwd`.
- **HTTP** (`url`, and `type` absent, `"http"` or `"streamable-http"`):
  `url` (http or https) and `headers`.
- `type: "sse"` is refused: "SSE servers are not supported; use the
  server's streamable HTTP endpoint".
- `env` and `headers` values expand `${VAR}` from tau's environment. A
  variable that is not set fails that server, naming the variable.
  pi's `!command` values are left out.
- **Names** match `^[A-Za-z0-9_-]+$`. Two names that differ only in
  `-` and `_` clash, since they share a namespace.
- An invalid entry is reported on the page and skipped; the others
  still connect.
- OAuth, `auth.provider` and the `oauth` block are left for later.

## Connections

- **Owned by the agent.** The plugin holds one connection per enabled
  server, shared by every run, and closes them when it is dropped.
- **Started in the background** when the plugin is built. The plugin
  does not wait for them, except as below.
- **States:** `connecting`, `connected`, `disconnected`, `failed`,
  `closed`, shown on the page with the last error.
- **Reconnects** lazily: the next call to a dropped server connects
  again. HTTP connects retry transient errors (408, 429, 5xx but 501,
  network errors) after 250 ms and 1 s. **Tool calls are never
  retried**: they can have side effects.
- **Protocol:** rmcp's `ClientLifecycleMode::Auto`, 2026-07-28
  preferred, 2025-11-25 as the fallback.
- **Lists:** `tools/list` with every page. On
  `notifications/tools/list_changed` the server's tools are listed
  again.
- **Stdio close:** rmcp's `cancel()`, then the child's process group
  gets SIGTERM and, after 2 s, SIGKILL.
- **Roots:** the repository's directory.
- **Server logs** (`notifications/message`) go to tau's log.

## Tools

### Names

`mcp__<server>__<tool>`, every character outside `[A-Za-z0-9_]`
replaced by `_`. A name over 64 characters, or one that collides with
another, is cut and gets `_` and the first 8 hex digits of
`sha256(server \0 tool)`; every tool in a collision gets one, so the
result does not depend on order. A server's namespace is
`mcp__<server with - as _>`.

### Exposure

| Exposure   | Declared to the model     | Callable from Codemode |
| ---------- | ------------------------- | ---------------------- |
| `direct`   | yes, from the run's start | yes                    |
| `codemode` | no                        | yes                    |
| `hidden`   | no                        | no                     |

- **Direct tools are per run.** `Plugin::start` adds them with
  `RunPlan::add_tool`, after waiting up to 10 s for servers that have
  direct tools and are still connecting. A server that answers later,
  or a tool it adds mid-run, is declared from the next run on. A run's
  tools never change once it starts.
- **Codemode tools** are `Nested`: the loop resolves them by name at
  call time from the plugin's tool source, so a script reaches a server
  that connected after the run started. Before a codemode call runs, it
  waits for the servers its script names (`mcp__<server>`), or for all
  of them when it uses `search_tools`, `describe_namespace` or
  `ALL_TOOLS`, up to the call's timeout.
- **Withdrawn tools** fail with "Tool mcp__x__y is no longer offered by
  server x".

### Definitions

- **Description:** the tool's description, else its title, else
  `MCP tool <tool> from server <server>`. The server's `instructions`
  never go into a description.
- **Parameters:** the tool's `inputSchema`, with `type: "object"` and
  `properties: {}` added if missing. Validation is tau-agent's.
- **Output schema:** a `CallToolResult` wrapper,
  `{ content, structuredContent?: <the tool's outputSchema>, isError }`,
  so Codemode renders `CallToolResult<T>`.
- **Annotations:** the MCP hints (`readOnlyHint`, `destructiveHint`,
  `idempotentHint`, `openWorldHint`) are kept in the tool's metadata,
  for the constitution and the interface.
- **Execution mode:** `Parallel`.
- **Progress** notifications become `ToolUpdate`s.
- **Cancel:** the run's token cancels the request
  (`notifications/cancelled`). The server may still finish it.

### Results

- **For the model:**
  - text blocks as text, images as images;
  - a resource link as
    `[Resource <uri> "<title>" (<mime>, <size>): <description>]`;
  - an embedded text resource as its text; an embedded binary one
    saved to a 0600 file in `$TMPDIR`, its path given;
  - with no content blocks, `structuredContent` as pretty JSON;
  - text over 20 KB cut in the middle, at UTF-8 boundaries, in
    Codemode's format, the full text written to
    `$TMPDIR/tau-mcp-<hex>.txt`;
  - `isError` makes the call fail; with no text, it says
    `MCP tool <server>/<tool> returned an error`.
- **For scripts:** `ToolOutput::structured` holds the whole
  `CallToolResult` (`content`, `structuredContent`, `isError`), never
  cut. An `isError` result carries it too, so a script gets it back
  instead of an error.

## The server list

At `start`, the plugin puts an `<mcp_servers>` block in
`plan.context`, at most 4,096 characters:

```
<mcp_servers>
MCP servers connected to this agent. Their direct tools are declared to you. Call the tools of `codemode` servers from codemode scripts: find them with `search_tools(query, { namespace = name })` and read a server's instructions and tool names with `describe_namespace(name)`.
- mcp__linear (direct): Linear issues and projects.
- mcp__git (codemode): Git operations on the repository.
</mcp_servers>
```

Each description is at most 250 characters, shortened to fit; servers
that still do not fit are dropped with `- … N more servers; find their
tools with search_tools()`. The block is fixed for the run. pi appends
a new block when servers change mid-session; tau shows the change from
the next run.

## Tests

- **Config, as properties:** parsing and printing a config round-trips;
  merging is last-wins by name; names that differ only in `-` and `_`
  clash; `${VAR}` expansion leaves text without `${` unchanged; the
  exposure of a tool follows exact name, then first glob.
- **Names, as properties:** every name matches `^[A-Za-z0-9_]{1,64}$`;
  distinct (server, tool) pairs get distinct names; the names do not
  depend on the order tools are listed.
- **Results, as properties:** the 20 KB cut keeps a prefix and a suffix
  at char boundaries within the limit; text under it is unchanged.
- **Against a server:** an in-process rmcp server (rmcp's `server`
  feature in dev-dependencies) over a duplex stream and over a child
  process: listing, calling, structured content, `isError`, progress,
  cancel, `list_changed`, a server that drops and reconnects, a call
  that is not retried.
- **In the loop:** direct tools are declared and called by the model;
  codemode tools are not declared and a script calls them; hidden tools
  are neither; a server that connects mid-run shows from the next run.
