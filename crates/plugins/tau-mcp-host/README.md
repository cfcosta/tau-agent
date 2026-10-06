# tau-mcp-host

Connects a tau agent to MCP servers and adds their tools. This is the
host half of tau's MCP plugin: it starts stdio servers, connects to
streamable HTTP ones, signs in with OAuth, keeps one connection per
server shared by every run, and turns each server's tools, resources
and prompts into what a run can use. The MCP client is `rmcp`, and only
the private `client` module and `auth` touch it.

## What it provides

| Item               | What it is                                                               |
| ------------------ | ------------------------------------------------------------------------ |
| `McpPlugin`        | The agent plugin: servers' tools, the `<mcp_servers>` block              |
| `McpPluginBuilder` | Builds one from config files, settings and extra servers                 |
| `McpHost`          | The `HostHalf` tau-ui registers: one plugin per scope                    |
| `Host`             | `McpHost`'s host state: the pools, sign-in and sign-out                  |
| `RunServers`       | A run's servers: its scope's plugin, shared by its runs                  |
| `connection`       | `Connection`, one per server, and its `State` and `Status`               |
| `pool`             | `Pool`: connections kept across changes to the servers                   |
| `tool::McpTool`    | One MCP tool as a tau tool                                               |
| `resources`        | `list_mcp_resources`, `list_mcp_resource_templates`, `read_mcp_resource` |
| `prompts`          | Getting a prompt from its server: `Prompt`, `prompt_text`                |
| `results`          | What a `CallToolResult` becomes, for the model and for scripts           |
| `auth`             | OAuth sign-in: `begin`, `SignIn`, `TokenStore`, `secrets_filter`         |

Each server has an exposure. `direct` tools are declared to the model.
`codemode` tools are only callable from Codemode scripts, through a tool
source. `hidden` tools are not added. Results for the model are cut at
5,000 tokens; scripts get the whole result.

Sign-ins are kept in `~/.config/tau/mcp-auth.json` (`auth::AUTH_FILE`),
owner-only. tau never opens a browser by itself: a server that answers
401 waits in the `needs-auth` state until the user signs in from the
Servers page.

## How it fits

This is the host half (decision 0030). Its partner is `tau-mcp`, the
interface half, which owns the config format, tool names, the Servers
page and the cards. This crate re-exports `tau_mcp::NAME`.

It builds on `tau-agent`, `tau-ai`, `tau-mcp` and `tau-ui-plugin`.
`tau-ui` depends on it and registers `McpHost` in
`tau_ui::hosted::halves`. On the host, the user's servers run once in a
pool shared by every repository; a repository's servers, and user
servers with a relative `cwd`, run in that repository's own pool.

Add `McpPlugin` before `Codemode` (`tau-codemode-host`): Codemode lists
the signatures of the tools in the run's plan at `start`, and tau-mcp
adds its direct tools in its own `start`.

## Usage

```rust
use tau_agent::agent::Agent;
use tau_codemode_host::Codemode;
use tau_mcp_host::McpPlugin;

// Inside a tokio runtime: `build` starts connecting in the background.
let mcp = McpPlugin::builder()
    .user_dir(config_dir)   // ~/.config/tau: mcp.json and mcp-auth.json
    .repo(repository)       // <repo>/.tau/mcp.json, relative cwds, roots
    .build();
let agent = Agent::new(llm.clone())
    .plugin(mcp.clone())
    .plugin(Codemode::new(None));

// Later: each server's connection, and what was skipped.
for connection in mcp.connections() {
    println!("{}: {:?}", connection.name(), connection.status());
}
println!("{:?}", mcp.config_errors());
mcp.shutdown().await;
```

The builder also takes `settings`, `env` (for `${VAR}`), `home` (for
`~/`), `startup_wait`, `spill_dir`, and `server`, which adds a
`tau_mcp::config::ServerConfig` after the files, such as an in-process
server through `Transport::Stream`.

## Testing

```sh
cargo nextest run --release -p tau-mcp-host
```

Most tests run against an in-process rmcp server (`tests/common`, using
rmcp's `server` feature in dev-dependencies) over a duplex stream. OAuth
tests use a local authorization server and loopback callback. Many are
Hegel properties; the workspace's `hegel.toml` sets their case counts.

`tests/stdio` has its own `main` (`harness = false`): the test binary
runs itself as a stdio server in a child process, since libtest's output
would corrupt the server's stdout. It still works under nextest.

`examples/live.rs` checks tau-mcp and tau-codemode against real servers
(server-everything, mcp-server-git, mcp-server-fetch). It needs Node, uv
and git, and the network to fetch the servers:

```sh
nix shell nixpkgs#nodejs nixpkgs#uv nixpkgs#git -c \
  cargo run -p tau-mcp-host --example live -- [--model gpt-5.5]
```

It prints `ok` or `FAIL` per check and exits with the number failed.
With `--model`, it also runs the agent on the signed-in ChatGPT account
in `~/.config/tau/chatgpt`.

## Further reading

- [MCP servers reference](../../../docs/reference/mcp.md)
- [0018: MCP servers and Codemode, as two plugins](../../../docs/decisions/0018-codemode-and-mcp.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
- [Codemode libraries research](../../../docs/research/codemode-libraries.md),
  on choosing rmcp
- [Environment](../../../docs/reference/environment.md), for how a
  repository's stdio servers are launched
