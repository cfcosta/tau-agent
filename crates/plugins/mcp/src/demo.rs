//! The MCP servers tau-ui's demo shows. A demo reaches no servers, so
//! these stand for what its host half would see once they started.

use crate::{
    info::Annotations,
    ui::{Defined, PendingRow, ServerRow, Servers, ToolRow},
};

/// tau-agent's servers as a host that started them sees them: two
/// connected, one failed, and one waiting for approval.
pub fn servers() -> Servers {
    let tool =
        |server: &str, tool: &str, exposure: &str, annotations| ToolRow {
            name: Some(format!("mcp__{server}__{tool}")),
            tool: tool.into(),
            description: None,
            exposure: exposure.into(),
            annotations,
        };
    let read_only = Annotations {
        read_only: Some(true),
        ..Annotations::default()
    };
    Servers {
        started: true,
        servers: vec![
            ServerRow {
                name: "linear".into(),
                defined: Defined::User,
                transport: "https://mcp.linear.app/mcp (headers Authorization)"
                    .into(),
                exposure: "direct".into(),
                enabled: true,
                description: Some("Linear issues and projects.".into()),
                state: Some("connected".into()),
                tools: vec![
                    tool("linear", "list_issues", "direct", read_only),
                    tool(
                        "linear",
                        "create_issue",
                        "direct",
                        Annotations::default(),
                    ),
                ],
                ..ServerRow::default()
            },
            ServerRow {
                name: "git".into(),
                defined: Defined::Settings,
                transport: "uvx mcp-server-git".into(),
                exposure: "codemode".into(),
                enabled: true,
                state: Some("connected".into()),
                tools: vec![
                    tool("git", "git_status", "codemode", read_only),
                    tool(
                        "git",
                        "git_push",
                        "hidden",
                        Annotations {
                            destructive: Some(true),
                            ..Annotations::default()
                        },
                    ),
                ],
                entry: Some(serde_json::json!({
                    "command": "uvx", "args": ["mcp-server-git"], "exposure": "codemode",
                    "toolExposure": { "git_push": "hidden" },
                })),
                ..ServerRow::default()
            },
            ServerRow {
                name: "browser".into(),
                defined: Defined::Settings,
                transport: "npx @playwright/mcp".into(),
                exposure: "codemode".into(),
                enabled: true,
                state: Some("failed".into()),
                error: Some(
                    "the environment variable `PLAYWRIGHT_BROWSERS` is not set"
                        .into(),
                ),
                entry: Some(
                    serde_json::json!({ "command": "npx", "args": ["@playwright/mcp"], "exposure": "codemode" }),
                ),
                ..ServerRow::default()
            },
        ],
        pending: vec![PendingRow {
            name: "db".into(),
            transport: "./scripts/db-mcp --readonly".into(),
            entry: serde_json::json!({ "command": "./scripts/db-mcp", "args": ["--readonly"] }),
            hash: "4f1c…".into(),
        }],
        errors: Vec::new(),
        user_names: ["linear".to_owned()].into(),
        user_file: Some("~/.config/tau/mcp.json".into()),
        repo_file: Some("~/Code/cfcosta/tau-agent/.tau/mcp.json".into()),
    }
}
