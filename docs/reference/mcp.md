# MCP servers (`tau-mcp`)

- Status: built in `crates/plugins/tau-mcp` (`tau-mcp`), with its
  interface: `McpUi`, a `UiPlugin`
  ([0017](../decisions/0017-plugins-bring-their-ui.md)) that tau-ui
  registers ("The interface"). Decided in
  [0018](../decisions/0018-codemode-and-mcp.md). Where the build
  differs from the first design, this page says so; the differences
  are listed in "Deviations".
- Date: 2026-10-01

`tau-mcp` connects to MCP servers and adds their tools to the agent,
with their resources as three tools and their prompts as composer
commands. HTTP servers that ask sign in with OAuth ("Signing in").
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
hash of the entry, so a commit that changes the entry asks again: the
SHA-256, in hex, of the name, a NUL and the entry as tau prints it
(defaults left out), so reformatting the file does not ask again. The
settings are
`{ "mcpServers": { ... }, "approvedRepoServers": [hash], "disabledServers": { ... } }`.
Until approved, a repository server is reported as pending and not
connected.
pi reads a project's file only when the project is trusted; tau asks
per server.

### Turning servers off

The page turns any server off or on again without editing a file. The
settings keep the names it turned off:

```json
{
  "disabledServers": {
    "user": ["linear"],
    "repos": { "/home/me/src/tau": ["git", "db"] }
  }
}
```

- `user`: the user's servers (the user's file's and the settings')
  turned off in every repository. It does not reach a repository's own
  server of the same name.
- `repos`: per repository, keyed by its directory as tau lists it, the
  servers turned off there: its own, or the user's in it alone. A name
  stays listed after its server goes, and holds again if a server of
  that name comes back. A repository left with none is dropped.
- A server is on when its entry is (`enabled`, default true) and
  neither list turns it off: entry ∧ ¬user ∧ ¬repository. An entry's
  `enabled: false` is turned on only by editing the entry, so the page
  locks that switch; a repository's page likewise locks the switch of
  a user server turned off in every repository. Turning a server off
  never changes its approval.

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
  `url` (http or https), `headers`, and `oauth`, how to sign in
  ("Signing in"), only without an `Authorization` header.
- `type: "sse"` is refused: "SSE servers are not supported; use the
  server's streamable HTTP endpoint".
- `env`, `headers` and `oauth.clientSecret` values expand `${VAR}`
  from tau's environment. A
  variable that is not set fails that server, naming the variable:
  "the environment variable `X` is not set". `NAME` is a letter or
  `_`, then letters, digits and `_`; any other `${` is left as it is.
  pi's `!command` values are left out.
- **Names** match `^[A-Za-z0-9_-]+$`. Two names that differ only in
  `-` and `_` clash, since they share a namespace.
- An invalid entry is reported on the page and skipped; the others
  still connect. Unknown keys are ignored. A name that clashes with an
  earlier one is skipped: the first in merge order wins.
- pi's `auth.provider` (a `/login` provider's token sent to a server)
  is left out; `auth` is an unknown key.

## Connections

- **Owned by the agent.** The plugin holds one connection per enabled
  server, shared by every run, and closes them when it is dropped. In
  tau-ui the host keeps them: the user's servers once for every
  repository, a repository's own per repository ("The interface").
- **Started in the background** when the plugin is built. The plugin
  does not wait for them, except as below.
- **States:** `connecting`, `connected`, `disconnected`, `failed`,
  `needs-auth` ("Signing in"), `closed`, shown on the page with the last
  error.
- **Reconnects** lazily: the next call to a dropped or failed server
  connects again. HTTP connects retry transient errors (408, 429, 5xx
  but 501, network errors) after 250 ms and 1 s. **Tool calls are never
  retried**: they can have side effects. Reading a resource and getting
  a prompt are read-only, so one the connection drops under is sent
  once more, on a new connection. A connect, listing the tools
  included, gives up after 30 s.
- **Protocol:** rmcp's `ClientLifecycleMode::Auto`, 2026-07-28
  preferred, 2025-11-25 as the fallback. `Auto` falls back only for a
  server that does not know `server/discover`; one that knows it but
  offers only 2025-11-25 gets a second connection that initializes.
- **Lists:** `tools/list` with every page. On
  `notifications/tools/list_changed` the server's tools are listed
  again. On 2026-07-28 that notification comes only through
  `subscriptions/listen`, which tau opens when the server says its tool
  list changes; on 2025-11-25 it is a plain notification. A
  disconnected server keeps its last tools, so a call to one connects
  again. A server whose capabilities offer resources gets its resources
  and resource templates listed too, and one that offers prompts its
  prompts, every page, and again on `resources/list_changed` and
  `prompts/list_changed` (through `subscriptions/listen` on
  2026-07-28). A server without the capability is not asked; a list
  that fails is left empty and does not fail the connect.
- **Lost HTTP sessions.** A server that answers a request in a session
  with 404 gets a new session (rmcp's), and the request is posted
  again. So does one that answers it with an error naming no request
  (`"id"` absent), as the TypeScript SDK's examples answer, with 400, a
  session they do not know, after a restart: the server rejected the
  request without running it, so posting it again runs no tool twice.
  Without this, every call to such a server after its restart waited
  out its timeout.
- **Stdio close:** rmcp's `cancel()`, then the child's process group
  gets SIGTERM and, after 2 s, SIGKILL.
- **Roots:** the repository's directory, as a `file://` URI, when the
  plugin has one. In tau-ui a user server shared by every repository
  has none; give it a relative `cwd` (`"."`) to run it per repository,
  with the repository as its root.
- **Server logs** (`notifications/message`) and a stdio server's
  stderr go to `tracing`, target `tau_mcp::server`: tau has no log of
  its own yet, so the host subscribes to see them, with
  `auth::secrets_filter()` ("Signing in", "Logs").
- **HTTP** uses tau's TLS: reqwest with rustls, `ring` and the webpki
  roots, as Jev does, with no redirects (so headers never reach another
  host).

## Signing in

OAuth applies to an HTTP server whose entry sends no `Authorization`
header, when tau has a configuration directory to keep the sign-in in.
It follows pi (`extensions/mcp`, [research](../research/pi-codemode.md))
and the MCP authorization spec; the protocol is rmcp's `auth` module
(discovery, registration, PKCE, refreshing, scopes), and tau adds the
loopback, the checks on the callback, the grants file and its own HTTP
(rustls, `ring`, the webpki roots).

### The `oauth` block

```json
{
  "url": "https://mcp.example.com/mcp",
  "oauth": {
    "clientId": "registered-beforehand",
    "clientSecret": "${EXAMPLE_SECRET}",
    "callbackPort": 53682,
    "callbackUrl": "http://localhost:53682/callback",
    "scope": "read write",
    "clientName": "tau on my laptop",
    "authServerMetadataUrl": "https://auth.example.com/.well-known/oauth-authorization-server"
  }
}
```

Every field is optional, and an empty string or `null` is the same as
leaving it out. Without the block, sign-in registers a client and asks
for the scopes the server names.

- `clientId`, `clientSecret`: a client registered beforehand; no
  registration then. A secret needs a client id and expands `${VAR}`;
  it is sent with the token requests (HTTP Basic, or in the body when
  the server takes only that) and never saved in the grants file.
- `callbackUrl`: the redirect URI, plain `http` on a loopback host
  (`127.0.0.0/8`, `localhost` or `[::1]`), with a port, no user, query
  or fragment. `callbackPort` alone is `http://127.0.0.1:<port>/callback`;
  given both, the ports must agree. Without either, any free port on
  127.0.0.1, path `/callback`.
- `scope`: the scopes to ask for, separated by spaces. Without it, those
  the server names: its challenge's `scope`, then its protected
  resource metadata's, then the authorization server's
  `scopes_supported`; `offline_access` is added when the authorization
  server offers it.
- `clientName`: a registered client's name, `tau` by default.
- `authServerMetadataUrl`: the authorization server's metadata, when
  discovery from the server cannot find it.
- The block is refused with an `Authorization` header, a secret without
  a client, a callback that is not loopback or names no port, a port
  outside 1 to 65535, or a metadata URL that is not http or https.

### When a server asks

- A connect answered 401 (with or without `WWW-Authenticate`), or 403
  with `error="insufficient_scope"`, leaves the connection in
  `needs-auth`, with the challenge and the scope it asked for. Nothing
  opens a browser: the page shows "needs sign-in" and Sign in.
- While it waits, calls, reads and prompts fail at once, "the server
  asks you to sign in: sign in on the MCP servers page", without asking
  the server again. A codemode script's wait does not wait for it.
- A connection keeps the sign-in it connected under (a random id each
  sign-in gets, which refreshes keep). Before each use it reads the
  grants file: when that id changed, because of a sign-in or sign-out
  here or in another process, it connects again. So `needs-auth` ends
  on the next use after someone signs in anywhere.

### Signing in

`auth::begin` and `SignIn::finish`; on the host, `Host::sign_in` behind
the page's Sign in:

1. Listen on the callback address. A client registered before on a port
   the configuration leaves free gets that port again if it is free, so
   the client is reused.
2. Find the authorization server: `authServerMetadataUrl`, else the
   challenge's `resource_metadata` (RFC 9728), the server's well-known
   protected resource metadata, then authorization server metadata
   (RFC 8414 or OpenID), as rmcp does it (same-origin and issuer checks;
   loopback only for a loopback server). A server that publishes none
   gets the 2025-03-26 defaults, `/authorize`, `/token`, `/register`.
3. The client: the configured one; else the one registered before, if
   its redirect URI and issuer are the same; else registration
   (RFC 7591), a public client (`token_endpoint_auth_method: none`,
   `application_type: native`) named `clientName`.
4. The scopes: as above; for more scopes (`insufficient_scope`), those
   granted, then the ones asked for.
5. The authorization URL: `response_type=code`, PKCE S256 (a fresh
   verifier of 43 to 128 unreserved characters), a fresh `state`, and
   `resource` (RFC 8707). The page opens it in the browser.
6. The callback, within 5 minutes: requests to another path get a 404,
   answers whose `state` is missing, given twice or another's get a 400,
   and the wait goes on. With this attempt's `state`, an `error` ends
   the sign-in, else the `code` (and `iss`, checked against the issuer,
   RFC 9207) goes to the token endpoint with the verifier and redirect
   URI. The browser gets a page saying how it went.
7. The grant is saved, every connection that uses it connects again,
   and the page shows it. A failure is an alert. A second Sign in for the
   same grant ends the first.

### Using and refreshing

A connection whose grant is signed in sends `Authorization: Bearer`
with each request (rmcp's `AuthClient`). The access token is refreshed
before a request when it expires within 30 s, and once when the server
answers 401 to it; the request is then sent again. A refresh keeps the
refresh token when the answer has none. A refresh that the server turns
down (`invalid_grant`), or no refresh token, ends in `needs-auth`. A
refresh holds a lock file, so two processes do not refresh one grant at
once.

### Signing out

Sign out (`Host::sign_out`) forgets the grant's tokens, scopes and
sign-in id, keeping its client for the next sign-in, and connects every
connection that used it again; the server then asks again. Tokens are
not revoked at the authorization server.

### The grants file

`~/.config/tau/mcp-auth.json` (`TokenStore`), owner-only (0600, its
directory 0700 when tau makes it), written whole through a temporary
file and a rename under `mcp-auth.json.lock`; refreshes hold
`mcp-auth.json.refresh.lock`. One grant per server URL and configured
client id (`GrantKey`):

```json
{
  "grants": [
    {
      "url": "https://mcp.example.com/mcp",
      "clientId": "issued-by-registration",
      "clientSecret": "only one registration issued",
      "redirectUri": "http://127.0.0.1:53682/callback",
      "metadata": { "issuer": "https://auth.example.com", "token_endpoint": "…" },
      "issuer": "https://auth.example.com",
      "tokens": {
        "access_token": "…",
        "token_type": "Bearer",
        "expires_in": 3600,
        "refresh_token": "…"
      },
      "receivedAt": 1790000000,
      "scopes": ["read"],
      "account": "ada@example.com",
      "signedIn": "5b0c…"
    }
  ]
}
```

- `client` (the configured client id) is there for a configured
  client; `resource` when the resource indicator is not the URL.
- `tokens` is the token response as it came; `account` is an ID token's
  `email` (else `preferred_username`, `name`, `sub`), read for the page
  and never trusted.
- No token, secret or code is logged or printed: `Grant`, `OAuthConfig`,
  `Callback` and `SignIn` leave them out of `Debug`, and the page shows
  the account, issuer, scopes, expiry and whether it refreshes.

### Logs

rmcp 3.5's sign-in code logs, at debug level, the authorization code
(`start exchange code for token`) and the whole token response
(`exchange token result`), whose extra fields may hold an ID token, in
the targets `rmcp::transport::auth` and `rmcp::transport::common::auth`
(`auth::SECRET_TARGETS`). Two things keep them out:

- tau runs the code exchange with no subscriber
  (`WithSubscriber::with_subscriber(NoSubscriber)`), so those lines reach
  none, global or scoped, whatever its filter.
- Every host that sets up a subscriber adds `auth::secrets_filter()`, a
  `tracing_subscriber::filter::Targets` used as a global filter: it lets
  everything through but those targets below info, which no other
  filter, nor a user's `RUST_LOG`, can widen. For an `EnvFilter`, the
  same as directives is `auth::LOG_DIRECTIVES`
  (`rmcp::transport::auth=info,rmcp::transport::common::auth=info`),
  added last. tau sets up no subscriber today (not tau-ui, not the
  examples, not the tests but the one below); one that is added must
  carry the filter.

```rust
use tracing_subscriber::layer::SubscriberExt as _;
let subscriber = tracing_subscriber::registry()
    .with(tau_mcp::auth::secrets_filter())
    .with(tracing_subscriber::fmt::layer());
```

Checked when rmcp moves: its new debug lines in those targets, and any
other target that prints a code or a token.

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
- **The wait is Codemode's.** tau-mcp's tool source implements
  `ToolSource::ready`: it waits for the named servers (by namespace or
  name), or all, to finish connecting, and connects a named one that
  dropped or failed. Codemode decides which servers a script names.

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
  `idempotentHint`, `openWorldHint`) are kept for the constitution and
  the interface: `McpTool::annotations()` (and `McpPlugin::tool(name)`
  to find the tool), and every call's `details`,
  `{ server, tool, annotations, structuredContent? }`, the last when the
  result has one of at most 20 KB as JSON, for the call's card.
- **Execution mode:** `Parallel`.
- **Progress** notifications become `ToolUpdate`s: the message and
  `(progress/total)` as text, `{ progress, total, message }` as
  details. Every notification the server sent before its result
  reaches the run, in order, before the call ends: tau routes progress
  as the transport receives it, because rmcp hands notifications to
  their handler in tasks of their own that may run after the result.
  Updates after the result are dropped.
- **Cancel:** the run's token cancels the request
  (`notifications/cancelled`). The server may still finish it.

### Results

- **For the model:**
  - text blocks as text, images as images;
  - a resource link as
    `[Resource <uri> "<title>" (<mime>, <size>): <description>. Read it with read_mcp_resource (server "<server>")]`,
    the title its `title`, else its `name`, each part but the hint
    left out when missing;
  - an embedded text resource as its text; an embedded binary one
    saved to a 0600 file `$TMPDIR/tau-mcp-<hex>.<ext>`, as
    `[Resource <uri> (<mime>) saved to <path>]`; audio the same way;
  - with no content blocks, `structuredContent` as pretty JSON;
  - text over 5,000 tokens (about 20 KB, four characters a token, all
    text blocks together) merged into one block and cut in the middle,
    half kept from each end, at character boundaries, in Codemode's
    format (`tau_agent::output`), the full text written to
    `$TMPDIR/tau-mcp-<hex>.txt`; images follow it;
  - `isError` makes the call fail; with no text, it says
    `MCP tool <server>/<tool> returned an error`.
- **For scripts:** `ToolOutput::structured` holds the whole
  `CallToolResult` (`content`, `structuredContent`, `isError`), never
  cut. An `isError` result carries it too, so a script gets it back
  instead of an error.

## Resources

As pi and Codex expose them: three tools of the plugin's, not of a
server's.

| Tool                          | Arguments         | For the model                   | For scripts                    |
| ----------------------------- | ----------------- | ------------------------------- | ------------------------------ |
| `list_mcp_resources`          | `{ server? }`     | `{ resources }` as pretty JSON  | `{ resources: [...] }`         |
| `list_mcp_resource_templates` | `{ server? }`     | `{ resourceTemplates }` as JSON | `{ resourceTemplates: [...] }` |
| `read_mcp_resource`           | `{ server, uri }` | the contents                    | `{ server, uri, contents }`    |

- **Exposure:** the widest among the servers, on and not hidden, whose
  capabilities offer resources (`resources::exposure`): declared from
  the run's start (added in `start`) when one of them is `direct`,
  `Nested` when the widest is `codemode`, absent when there is none.
  The servers' `toolExposure` does not count.
- **Listing** gives the lists last listed, each item with its `server`
  first, then the resource's `uri`, `name`, `title`, `description`,
  `mimeType` and `size` (a template's `uriTemplate` instead of `uri`),
  all servers' or `server`'s. It first waits for the servers to finish
  connecting, and connects a named one that dropped or failed. An
  unknown server fails, naming those that offer resources; a named one
  that does not offer them, or is not connected, fails saying so. The
  text is cut at 5,000 tokens as a call's is.
- **Reading** sends `resources/read`, tried twice as above. Its
  contents: text as text; an `image/*` blob as an image; any other blob
  saved to a 0600 file in the spill directory, as
  `[Resource <uri> (<mime>) saved to <path>]`; the text cut at 5,000 tokens.
  Scripts get the contents whole.
- **MCP apps** are left out everywhere: a resource or template whose
  URI starts `ui://` or whose type is `text/html;profile=mcp-app` (case
  and spaces aside) is not listed, its contents are dropped from a
  read, and reading a `ui://` URI fails. tau shows no MCP apps.

## Prompts

A server's prompts are the composer's commands, not the model's tools.

- **Names:** `/mcp__<server>__<prompt>`, named as tools are ("Names"),
  so they fit in 64 characters and never collide.
- **Arguments:** `key=value`, separated by whitespace. A value may be
  quoted, whole or in part, with `"..."` (where `\` takes the next
  character as it is) or `'...'` (as it is). The menu shows them as
  `name=… [style=…]`, required ones first.
- **Checks** before `prompts/get`: an argument the prompt does not take,
  one given twice, or a required one missing fails with the command's
  usage: "/mcp__git__commit needs the argument `changes`. Usage:
  /mcp__git__commit changes=<changes> [style=<style>]". The window
  checks them as the user runs the command, and the host again.
- **The messages** become the text a person sends, one after another
  with a blank line between, whatever their role: text as it is, an
  embedded text resource as its text, a resource link as a call's
  result shows it, and images, audio and binary resources named in
  brackets (`[Image (image/png)]`). The text fills the composer, for
  the person to read and send.
- **Where:** the composer lists the prompts of the servers of the
  repository it is in (the open run's, else the one new runs start in),
  or the user's alone outside one.

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

Each description is at most 250 characters, on one line, shortened
with `…`; servers that do not fit are dropped with `- … N more servers;
find their tools with search_tools()`. A server's description is its
entry's, else the first line of its instructions; without either the
line ends at the exposure. Every enabled server that is not `hidden`
is listed, connected or not. The block is fixed for the run. pi
appends a new block when servers change mid-session; tau shows the
change from the next run.

## The interface

`McpUi` (`src/ui/`) is the plugin with its UI. tau-ui registers it
before tau-codemode, whose `start` lists the run's tools after tau-mcp
added the direct ones.

### On the host

- **The user's servers are shared.** A server from the user's file or
  the settings (or one the host adds, `Host::set_servers`) runs once,
  in a pool every repository shares. One whose `cwd` is relative (not
  absolute, not `~/`) starts in the repository, so it runs once per
  repository instead, with the repository as its root and its `cwd`
  under it; shared servers get no roots. A repository's own server
  runs in its pool, and one that names a user server wins in that
  repository while the others keep the shared one. So does a user
  server the repository turned off for itself.
- **One plugin per scope.** A scope is a repository (the user's file,
  the settings and its `.tau/mcp.json`), or the user's servers alone.
  The host keeps an `McpPlugin` per scope over the connections it uses,
  built the first time a run in the repository starts, or Connect on
  the page asks for it; building it starts the connections it uses
  that never started. Until then the page lists the servers as
  configured, "not started", but a shared server as another scope
  started it. A repository with no enabled, approved server adds
  nothing to its runs.
- **Shared by runs.** Each run gets a wrapper (`RunServers`) that hands
  the scope's plugin's `start` and tool source to the run, so every run
  in a repository uses the same connections.
- **Rebuilt incrementally.** Each time a scope is used (a run starts,
  the catalog is drawn, an action runs) its servers are read again: the
  user's file from tau's config directory, the settings, the
  repository's file. When the merged servers, errors or pending
  approvals differ from what its plugin was built from, the plugin is
  built again over the pools, which compare each server's entry, as
  parsed, where it came from and whether it is on, with the last one
  (`pool::diff`): an unchanged server keeps its connection, an added
  or changed one gets a new connection, and a removed, changed or
  turned-off one is let go. A connection let go closes once the runs
  going on that hold it end; they keep what they started with. Files
  are not watched: an edit by hand shows the next time the scope is
  used.
- **Drawn again on changes.** Each connection a scope's plugin uses
  reports a change of its state or of what its server lists
  (`Connection::watch`); the host asks the interface to draw the
  catalog again, once per burst of changes (50 ms). So the page shows a
  server as connected once it is, not as it was when the page last
  asked.
- **Closed with the host.** Dropping the host shuts down every
  connection, runs going on included.
- **Settings.** The host half reads and saves the plugin's settings
  through `HostCx::settings` and `HostCx::save_settings`, and the user's
  file from `HostCx::config_dir`.

### The Servers page

`Link::page("servers").param("repo", name)`: a repository's servers,
or, without a repository, the user's and the settings' alone.

- **Each server:** its name; where it comes from (user file, settings,
  repository file); `shared` when it is one connection for every
  repository, `per repository` for a user server that is not; how it is
  reached, the command and arguments or the
  URL, with variables and headers by name only, since their values may
  hold tokens; its exposure; its state and last error (`not started`
  before it starts, `disabled` when off); its description; a switch that
  turns it off or on again, saying where (every repository on the
  user's page, this repository on a repository's), or why it is locked;
  its sign-in, when OAuth applies: Sign in while it waits (Sign in
  again when signed in and it asks for more), Sign out when signed in,
  and a line with who signed in where, the scopes, whether it
  refreshes, and the scope it asks for;
  and its tools, each with its exposure, its name as tools
  call it, and badges for its hints (read-only, destructive,
  idempotent, open world); then how many resources, resource templates
  and prompts it offers, and the first 20 of each: a resource's URI,
  title and type, a template's URI template, a prompt's command and
  arguments, each with its description's first line.
- **Waiting for approval:** each repository server not yet approved,
  with its whole entry as tau prints it, and Approve.
- **Skipped entries**, each with where and why.
- **Actions**, sent to the host half as JSON (`Act`):
  - `approve { repo, server, hash }` saves `hash` in
    `approvedRepoServers`, only if it is the hash of the entry pending
    now: an entry that changed since the page showed it is refused;
  - `add { name, entry }` adds a server to the settings: refused for a
    name the user's file has (valid entry or not), one already added, a
    name whose namespace another takes, or an entry that does not parse.
    The entry is saved as tau prints it, defaults left out;
  - `edit { name, entry }` and `remove { name }`, for the servers the
    page added only;
  - `enable { repo, name, enabled }` for any server of that repository,
    or, without one, of the user's: saved in `disabledServers`, never in
    an entry. Refused for a name that is not there, for turning on a
    server whose entry is off, and, in a repository, for turning on a
    user server off in every repository;
  - `reconnect { repo, server }` builds the scope's plugin if it was
    not, and connects `server`, or every server, again;
  - `sign_in { repo, server }` starts signing in, waiting up to 60 s for
    discovery and registration, and replies `sign_in { server, url }`,
    which opens the browser; when the browser comes back the host
    refreshes the page, or alerts why not;
  - `sign_out { repo, server }` signs out;
  - `prompt { repo, command, arguments }` gets a prompt in that scope,
    waiting up to 2 minutes, and replies `prompt { text }`, which fills
    the composer, or `prompt_failed { error, command }`, which shows the
    error and puts the command back in the composer.
    The editor checks a new server as the host does before it sends it,
    and says why it will not; removing asks twice.
- **Commands:** each prompt of a server that is on, as
  `Manifest::listed_commands` gives the composer ("Prompts").
- **The sidebar:** under each repository, "MCP", with its servers
  counted and a badge for the approvals waiting. The Plugins screen
  links to the page.

### Elsewhere

- **Catalog:** "MCP servers: 3 servers · 2 connected · 1 needs sign-in ·
  1 needs approval", counting servers by name across the user's and every
  repository's, connected where any scope connected them; "Connects to
  MCP servers and adds their tools" without any.
- **A run's plugin list:** its repository's servers in the same words,
  in amber while an approval or a sign-in waits, in red when a server
  failed;
  nothing for a repository without servers.
- **Tool cards:** a card for every `mcp__` tool: the server and the
  tool (from the call's details once it ended, else from the
  repository's servers), its hints as badges, the arguments, and the
  result: `structuredContent` as pretty JSON when the details carry it,
  else the text, at most 40 lines. A failed call is red with the
  result's first line. Calls a codemode script made are rows of its
  card, as tau-ui folds nested calls there.

## The crate

- `auth`: `begin`, `SignIn` (`url`, `redirect_uri`, `finish`),
  `SignInRequest`, `TokenStore` (`get`, `put`, `update`, `sign_out`,
  `fingerprint`), `Grant`, `GrantKey`, `Loopback`, `read_callback`,
  `Callback`, `CallbackError`, `pkce_challenge`, `valid_verifier`,
  `account`, `SECRET_TARGETS`, `LOG_DIRECTIVES`, `secrets_filter`. With `client`, the only modules that touch rmcp.
- `config`: `McpConfig::parse`/`to_json`, `merge`, `Sources::load`
  (`<user dir>/mcp.json`, the settings, `<repo>/.tau/mcp.json`),
  `Sources::disable`, `Read`, `Settings`, `Disabled`, `Off`,
  `repo_key`, `PendingApproval`, `expand_vars`, `expand_home`,
  `exposure_of`, `ServerConfig::per_repo`, `OAuthConfig`,
  `HttpConfig::uses_oauth`, `callback_address`.
- `pool`: `Pool` (`new`, `update`, `get`, `connections`, `shutdown`)
  and `diff`, for the host.
- `names`: `tool_names`, `namespace`.
- `results`: `map_result`, `resource_contents`, `resource_link`,
  `cut_middle`, `truncate`, `Spill`.
- `connection`: `Connection` (`new`, `start`, `connect`, `status`,
  `protocol`, `watch`, `tools`,
  `resources`, `templates`, `prompts`, `offers_resources`,
  `offers_prompts`, `instructions`, `call`, `read_resource`,
  `get_prompt`, `settled`, `shutdown`, `oauth`, `auth_need`,
  `sign_in_request`, `restart`), `State`, `Status`, `AuthNeed`, `ToolInfo`,
  `ResourceInfo`, `TemplateInfo`, `PromptInfo`, `PromptArgument`,
  `Annotations`, `CallFailure`, `Environment`. Only its private
  `client` module touches rmcp.
- `resources`: `ResourceTool`, `Kind`, `exposure`, `is_app`.
- `prompts`: `Prompt`, `prompts`, `command_names`, `parse_arguments`,
  `format_arguments`, `check_arguments`, `usage`, `arguments_hint`,
  `prompt_text`.
- `ui`: `McpUi`, its `Host` (with `sign_in`, `sign_out`,
  `token_store`), the page's data (`Servers`, `ServerRow`, `AuthRow`,
  `ToolRow`, `PendingRow`), `Act` and `apply` (what an action does to
  the settings), `server_entry`, `summary`, `summary_with_sign_in`;
  `ui::page` and `ui::card`.
- `tool::McpTool`; `McpPlugin` and `McpPluginBuilder`:

```rust
let mcp = McpPlugin::builder()
    .user_dir(config_dir)        // ~/.config/tau
    .repo(repository)            // .tau/mcp.json, cwd, roots
    .settings(settings)          // the page's servers and approvals
    .build();                    // starts connecting; needs a runtime
let agent = Agent::new(llm).plugin(mcp.clone());
mcp.connections();               // states, errors, tools
mcp.config_errors();
mcp.pending_approvals();
mcp.tool("mcp__linear__list_issues"); // with its annotations
mcp.resource_tools();            // list_mcp_resources, ... if any
mcp.prompts();                   // each with its command
mcp.get_prompt("mcp__git__commit", "changes=x", &cancel).await;
mcp.reconnect("linear");
mcp.shutdown().await;            // also on drop, in the background
```

`user_dir` is also where `mcp-auth.json` keeps sign-ins; without it,
OAuth does not apply and a 401 fails the server. The builder also takes
`env` (for `${VAR}`), `home` (for `~/`),
`startup_wait`, `spill_dir`, and `server(ServerConfig)`, which adds a
server after the files and settings: an in-process one through
`Transport::Stream(Dial)`, which no file can name.

## Against real servers

`crates/plugins/tau-mcp/examples/live.rs` checks tau-mcp and tau-codemode
against real MCP servers. Node, uv and git come from nixpkgs:

```sh
nix shell nixpkgs#nodejs nixpkgs#uv nixpkgs#git -c \
  cargo run -p tau-mcp --example live -- [--model gpt-5.6-luna]
```

It writes a temporary `mcp.json` and repository (never the user's
files) and starts:

- `everything`: `npx -y @modelcontextprotocol/server-everything stdio`,
  `direct`;
- `everything_http`: the same server's streamable HTTP mode on a free
  port, `codemode`;
- `git`: `uvx mcp-server-git` over the temporary repository,
  `codemode`;
- `fetch`: `uvx mcp-server-fetch`, `codemode`, against a page the
  example serves on localhost.

It prints `ok` or `FAIL` for each check and exits with the number
failed. The checks: every server connects, with the protocol it agreed
on; tools, resources, templates and prompts are listed, and
`list_changed` lists tools and resources the server adds later; calls
with text, structured content, images, errors, progress and roots;
cancelling a call ends it at once and the connection serves the next;
reading resources and templates; prompts as commands, with arguments and
their usage error; what the model reads of resource links, embedded
resources, images, structured content and the resource tools; every
real tool's Luau signature parses; Codemode scripts calling four tools
on three servers in `parallel`, `search_tools` and `describe_namespace`
on the real catalog, an `isError` result returned and an image passed
on; a killed stdio server and a restarted HTTP server reached again on
the next call; and closing the plugin ends every server process. With
`--model`, it runs the agent on the signed-in ChatGPT account (tau's
`~/.config/tau/chatgpt`) for at most 6 turns, and checks the model made
a direct MCP call and a codemode script that called MCP tools (about
$0.002 with `gpt-5.6-luna`).

As run on 2026-10-01, against server-everything 2026.8.31 (TypeScript
SDK 1.30.1), mcp-server-git and mcp-server-fetch 2026.8.18 (Python SDK
1.30.0): every server speaks 2025-11-25, so each connect goes through
rmcp's fallback from `server/discover` to `initialize`.

## Deviations

From the design above, as first written:

- **Pools on the host.** The design had the agent own the connections;
  tau-ui's host keeps a pool shared by every repository for the user's
  servers and one per repository for the rest, and a plugin per
  repository over them. A user server with a relative `cwd` runs per
  repository; a shared one gets no roots.
- **No file watching.** The files are read again whenever a scope is
  used, not watched.
- **Details carry the structured result**, at most 20 KB of it as
  JSON, for the card: tau-ui keeps a call's text and details, not its
  structured output.
- **A legacy fallback rmcp lacks:** a second connection with
  `initialize` for servers that know `server/discover` but offer only
  2025-11-25.
- **HTTP status from text.** rmcp hands a failed HTTP response on as
  `HTTP <status>: <body>` only, so the retry rule reads the status from
  it. It must be checked when rmcp moves.
- **Logs go to `tracing`**, not a log file: tau has none yet.
- **Connect timeout** of 30 s, which the design did not have.
- **Programmatic servers** (`McpPluginBuilder::server`,
  `Transport::Stream`), for embedding and tests.
- **A named failed server reconnects** in `ready`; `start` reconnects
  nothing, and `McpPlugin::reconnect` is for the page.
- **Audio** is saved to a file like a binary resource; the design
  named only text, images and resources.
- **Names** are distinct unless two SHA-256 prefixes of 8 hex digits
  collide, which the rule does not resolve.
- **Resources are listed from the connection's lists**, kept up to date
  by `list_changed`, not paged live with a cursor as Codex's tools are;
  the tools take no `cursor`.
- **Prompts fill the composer** rather than being sent: their text comes
  from a server, so the person reads it first.
- **Signing in waits for the user.** pi signs in from its `/mcp`
  command; tau never opens a browser on its own, and a run's call to a
  server that waits fails at once.
- **A registered client is reused** only on its port and with the same
  issuer; otherwise sign-in registers again.
- **The resource indicator** other than the server's URL is known to
  rmcp only through discovery, so a connection to such a server runs
  discovery again before using its grant.
- **No revocation** on sign-out: the tokens are forgotten, not revoked.

## Tests

- **Config, as properties:** parsing and printing a config round-trips;
  merging is last-wins by name; names that differ only in `-` and `_`
  clash; `${VAR}` expansion leaves text without `${` unchanged; the
  exposure of a tool follows exact name, then first glob.
- **Names, as properties:** every name matches `^[A-Za-z0-9_]{1,64}$`;
  distinct (server, tool) pairs get distinct names; the names do not
  depend on the order tools are listed.
- **Results, as properties:** the cut is `tau_agent::output`'s, whose
  tests keep a prefix and a suffix within the budget and the whole text
  in the spill file; text under it is unchanged.
- **Against a server:** an in-process rmcp server (rmcp's `server`
  feature in dev-dependencies) over a duplex stream, on 2026-07-28 and
  on 2025-11-25: listing every page, calling, structured content,
  `isError`, progress, cancel, `list_changed`, a server that drops and
  reconnects, a call that is not retried. As a property: however many
  progress notifications a server sends right before its result, on
  calls side by side, each call gets all of its own, in order, before
  it returns. Watched from another thread over 200 connects, a
  connection that reads as connected already counts its tools in its
  generation, so a run that waited never sees the tool list from
  before. Over a child process, the
  test binary run as a stdio server: a call, and closing ends the
  server and a process it started. A local HTTP server answering 503
  gets three tries; one answering 404, one.
- **Resources and prompts** (`tests/resources.rs`, `tests/prompts.rs`):
  as properties, the resource tools' exposure is the widest among the
  servers that offer resources, for any servers in any order; MCP apps
  are recognized whatever their case and spacing; `key=value`
  arguments, quoted or not, read back as written; a prompt's arguments
  are accepted exactly when none is unknown or given twice and every
  required one is there. Against the in-process server, on both
  protocols: every page of resources, templates and prompts, MCP apps'
  left out; reading text, an image and other binary; `list_changed`
  for resources and prompts; a read the connection drops under, sent
  once more; a server without the capabilities; prompts with arguments.
- **Signing in** (`tests/auth.rs`, `tests/oauth.rs`): as properties,
  every verifier of 43 to 128 unreserved characters is valid and its
  challenge is the unpadded base64url SHA-256, and any other string is
  no verifier; the callback gives the code back exactly for this
  attempt's `state`, given once, on its path, and is a mismatch for any
  other, a missing or a doubled one, even with an `error`; whatever is
  put in, signed out of and removed from the grants file, in any order,
  it holds what a map would, reads back the same, is 0600, and no
  grant's `Debug` shows a token or secret; a callback URL is accepted
  exactly when it is plain http on a loopback host with a port and no
  query, and its redirect URI keeps host, port and path; config
  round-trips with `oauth` blocks. Against a local authorization server
  and the in-process MCP server behind a bearer check, over a small
  tokio HTTP responder, the browser a GET that follows the redirect to
  the loopback: a 401 waits in `needs-auth` without asking anyone, and
  sign-in discovers, registers, exchanges with PKCE (every verifier
  checked) and connects on the next use; a token about to expire is
  refreshed first; one turned down is refreshed and the call sent
  again; a sign-out in another process waits again and the next sign-in
  reuses the client; `insufficient_scope` asks for the granted scopes
  and the new one; a configured client skips registration, sends its
  expanded secret and uses `authServerMetadataUrl`; the callback ignores
  other states and paths; the host's page shows the sign-in and its
  actions sign in and out; under a global subscriber that takes every
  level of every target, a sign-in, a refresh and calls leave no code,
  token, ID token, verifier or secret in anything it is given, though
  rmcp's sign-in debug lines reach it, and a layer behind
  `secrets_filter` gets none of them below info; and the filter clamps
  exactly those targets. In gpui's test app, the page draws a server
  waiting and one signed in, its buttons ask the host, and the host's
  answer opens the browser.
- **The server list, as a property:** at most 4,096 characters,
  descriptions at most 250, kept servers in order, the overflow count
  right.
- **The pools** (`tests/pool.rs`): as a property, for any old and new
  servers, `diff` keeps the unchanged servers on in both, connects the
  added and changed ones on now, and lets go of the removed, changed
  and turned-off ones that were on, and the pool keeps exactly the
  connections it says it keeps. Against in-process servers: changing
  one entry dials that server again and no other, while the old
  connection stays with whoever holds it; two repositories and the
  user's servers alone share one connection to a user server, dialed
  once and shown connected and shared; an approved repository server
  of the same name wins in its repository.
- **The interface** (`tests/ui.rs`): as properties, whatever the page
  asks in whatever order, the settings' servers are a model's (a new
  name the user's file and other namespaces leave free; edits only to
  the page's own; turning on and off never touches an entry; every
  entry parses; the settings survive saving); whatever is turned on
  and off, the settings say what was last asked of each name at each
  level, survive saving and say nothing once all is on; a server is on
  exactly when its entry is and no override turns it off, with the
  reason first of entry, everywhere, repository; and an approval holds
  for the hash of the entry shown, which reading the file again, laid
  out otherwise, does not change, while any other hash is refused and
  a changed entry waits again. Against a host over temporary
  directories: approving through `act` saves the hash shown; the page
  refuses a name the user's file has and edits, turns off and removes
  its own; it turns a user server off in one repository, leaving the
  shared connection to the rest, and refuses to turn on one its entry
  turns off or, from a repository, one off everywhere; a repository
  keeps its plugin until its servers change, and then keeps the
  connections whose entries did not; a user server with a relative
  `cwd` runs per repository; Connect starts a repository's servers. In
  gpui's test app: the editor sends only what it may; the page draws
  on a computer and a phone, with the editor open and with locked
  switches; the sidebar and the run's line count servers and
  approvals; the card names the server and shows the structured result
  or the text; a repository's prompts are its commands, a mistake in
  one's arguments is shown and the command put back, and the host's
  answer fills the composer. Against an in-process server on the host,
  the page lists its resources, templates and prompts and the host gets
  a prompt. And the design test.
- **In the loop**, with `ScriptedModel`: direct tools are declared and
  called by the model; codemode tools are not declared and a tool calls
  them through the loop (Codemode's scripts are its own crate's
  tests); hidden tools are neither; a server that connects mid-run
  shows from the next run; a withdrawn tool fails with its message; the
  model lists and reads a direct server's resources; with only
  codemode servers the resource tools are `Nested`; without a server
  that offers resources, or only hidden ones, there are none.
