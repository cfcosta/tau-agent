# tau-mcp

The interface half of tau's MCP plugin. It holds the `mcpServers`
configuration format, the names MCP tools get, servers' prompts as
composer commands, the Servers page and MCP tools' cards. It does not
connect to servers: that is `tau-mcp-host`'s job. So a phone can draw
the page and the cards without linking the MCP client.

## What it provides

| Item                   | What it is                                                       |
| ---------------------- | ---------------------------------------------------------------- |
| `McpUi`                | The `UiPlugin`: Servers page, tool cards, sidebar row, commands  |
| `NAME`                 | The plugin's name, `tau-mcp`                                     |
| `config`               | The `mcpServers` format, its checks, and where servers come from |
| `config::McpConfig`    | One file's servers: `parse`, `from_value`, `to_json`             |
| `config::ServerConfig` | One validated entry, with its `Transport` and `Exposure`         |
| `config::Sources`      | Every server tau would run, merged, with errors and approvals    |
| `config::Settings`     | The page's servers, approved repository servers, servers off     |
| `names`                | `mcp__<server>__<tool>` names: `tool_names`, `namespace`         |
| `prompts`              | Prompts as `/mcp__<server>__<prompt> key=value` commands         |
| `info`                 | What a server lists: tool `Annotations`, resources, prompts      |
| `ui::Servers`          | The page's data: `ServerRow`, `ToolRow`, `PendingRow`, ...       |
| `ui::Act`              | What a page action asks the host                                 |
| `demo`                 | Sample servers for tau-ui's demo (feature `demo`)                |

Servers come from three places, merged in this order: the user's
`~/.config/tau/mcp.json` (`config::USER_FILE`), the plugin's settings,
and the repository's `.tau/mcp.json` (`config::REPO_FILE`). A
repository's servers wait for the user's approval.

## How it fits

This is the interface half (decisions 0017 and 0030). Its partner is
`tau-mcp-host`, which connects to the servers, adds their tools to each
run, signs in with OAuth and answers the page's actions. It re-exports
`NAME` and reads `config`, `info` and `names` from here.

It builds on `tau-ui-plugin` and `tau-ui-kit`, and draws with gpui. It
has no tau-agent or rmcp dependency.

Used by:

- `tau-mcp-host`, the host half;
- `tau-ui-remote`, which registers `McpUi` before `CodemodeUi`, so the
  direct tools tau-mcp adds in `start` get Luau signatures;
- `tau-ui`, with the `demo` feature.

## Usage

Reading a config file's text:

```rust
use tau_mcp::config::McpConfig;

let (config, errors) = McpConfig::parse(
    r#"{ "mcpServers": { "git": {
        "command": "uvx",
        "args": ["mcp-server-git"],
        "exposure": "codemode"
    } } }"#,
);
assert!(errors.is_empty());
assert_eq!(config.servers[0].name, "git");
```

An invalid entry is reported in `errors` and skipped; the others are
kept.

## Features

| Feature | What it adds                                                      |
| ------- | ----------------------------------------------------------------- |
| `demo`  | `demo::servers`, the servers tau-ui's demo shows. Off by default. |

## Testing

```sh
cargo nextest run --release -p tau-mcp
```

`tests/config.rs` and `tests/names.rs` are Hegel property tests: config
printing and parsing round-trips, merging is last-wins by name, `${VAR}`
expansion, tool exposure by exact name then first glob, and tool names
that are valid, distinct and independent of listing order. The
workspace's `hegel.toml` sets the case counts. Nothing needs network or
credentials.

## Further reading

- [MCP servers reference](../../../docs/reference/mcp.md)
- [0018: MCP servers and Codemode, as two plugins](../../../docs/decisions/0018-codemode-and-mcp.md)
- [0017: Plugins bring their own UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [0029: Every plugin's settings in a pane of their own](../../../docs/decisions/0029-every-plugins-settings-in-a-pane.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
