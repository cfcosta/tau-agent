//! The plugin in the loop (`docs/reference/mcp.md`, "Tests"): direct
//! tools are declared and called by the model; codemode tools are not
//! declared and a tool calls them through the loop; hidden tools are
//! neither; a server that connects mid-run shows from the next run; a
//! withdrawn tool fails. And the `<mcp_servers>` block's limits, as a
//! property.

mod common;

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use common::State as Fixture;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::{InputBlock, Message, UserContent};
use tau_mcp::{
    DESCRIPTION_LIMIT,
    McpPlugin,
    SERVERS_INTRO,
    SERVERS_LIMIT,
    config::{Exposure, ServerConfig, Transport},
    servers_block,
};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tokio_util::sync::CancellationToken;

fn plugin(
    fixture: &Arc<Fixture>,
    configure: impl FnOnce(&mut ServerConfig),
) -> McpPlugin {
    let mut config =
        ServerConfig::new("srv", Transport::Stream(fixture.dial()));
    configure(&mut config);
    McpPlugin::builder()
        .env(Arc::new(|_| None))
        .home(None)
        .server(config)
        .startup_wait(Duration::from_secs(5))
        .build()
}

fn text(blocks: &[InputBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect()
}

/// The tools request `n` declared.
fn declared(llm: &ScriptedModel, n: usize) -> Vec<String> {
    llm.requests()[n]
        .settings
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

/// The last tool result request `n` carried: its text and whether it
/// failed.
fn last_result(llm: &ScriptedModel, n: usize) -> (String, bool) {
    llm.requests()[n]
        .transcript
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::ToolResult(result) => {
                Some((text(&result.content), result.is_error))
            }
            _ => None,
        })
        .expect("a tool result")
}

/// The first user message request `n` carried.
fn first_input(llm: &ScriptedModel, n: usize) -> String {
    llm.requests()[n]
        .transcript
        .iter()
        .find_map(|message| match message {
            Message::User(user) => Some(match &user.content {
                UserContent::Text(text) => text.clone(),
                UserContent::Blocks(blocks) => text(blocks),
            }),
            _ => None,
        })
        .expect("a user message")
}

async fn run(agent: &Agent, store: &Store) {
    agent.start("go", store).outcome().await.unwrap();
}

/// Calls the tool its `name` names with `args`, through the loop, after
/// waiting for the namespaces in `wait`; answers with the result, the
/// catalog's tool names and its namespaces.
struct Caller;

fn caller_schema() -> &'static Value {
    static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "args": {"type": "object"},
                "wait": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["name"]
        })
    })
}

#[async_trait]
impl AgentTool for Caller {
    fn name(&self) -> &str {
        "caller"
    }
    fn description(&self) -> &str {
        "Calls a tool."
    }
    fn parameters(&self) -> &Value {
        caller_schema()
    }
    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        if let Some(wait) = args["wait"].as_array() {
            let wait: Vec<String> = wait
                .iter()
                .map(|name| name.as_str().unwrap().to_owned())
                .collect();
            ctx.catalog().ready(Some(&wait), &ctx.cancel).await;
        }
        let catalog = ctx.catalog();
        let tools: Vec<&str> =
            catalog.tools().iter().map(|t| t.name()).collect();
        let namespaces: Vec<Value> = catalog
            .namespaces()
            .iter()
            .map(|space| {
                json!({
                    "name": space.name,
                    "description": space.description,
                    "instructions": space.instructions,
                    "tools": space.tools,
                })
            })
            .collect();
        let name = args["name"].as_str().unwrap();
        let result = match ctx.call(name, args["args"].clone()).await {
            Ok(output) => {
                json!({"ok": text(&output.content), "structured": output.structured, "details": output.details})
            }
            Err(ToolError::Output(output)) => {
                json!({"err": text(&output.content), "structured": output.structured, "details": output.details})
            }
            Err(error) => json!({"err": error.to_string()}),
        };
        Ok(ToolOutput::text(
            json!({"result": result, "tools": tools, "namespaces": namespaces})
                .to_string(),
        ))
    }
}

/// Direct tools are declared from the run's start and called by the
/// model; the server list is in the run's context; an `isError` result
/// fails the call.
#[tokio::test(flavor = "multi_thread")]
async fn direct_tools_are_declared_and_called_by_the_model() {
    let fixture = Fixture::new(false);
    let plugin = plugin(&fixture, |config| {
        config.description = Some("A test server.".into());
    });
    let store = Store::memory().await.unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("mcp__srv__echo", json!({"text": "hi"})))
        .turn(|t| t.tool_call("mcp__srv__fail", json!({})))
        .turn(|t| t.text("done"));
    let agent = Agent::new(llm.clone()).plugin(plugin.clone());
    run(&agent, &store).await;

    let tools = declared(&llm, 0);
    for name in ["echo", "fail", "slow", "hang", "crash"] {
        assert!(tools.contains(&format!("mcp__srv__{name}")), "{tools:?}");
    }
    assert_eq!(last_result(&llm, 1), ("hi".to_owned(), false));
    assert_eq!(
        last_result(&llm, 2),
        ("MCP tool srv/fail returned an error".to_owned(), true)
    );
    let input = first_input(&llm, 0);
    assert!(input.contains(&format!(
        "<mcp_servers>\n{SERVERS_INTRO}\n- mcp__srv (direct): A test server.\n</mcp_servers>"
    )), "{input}");

    let echo = plugin.tool("mcp__srv__echo").unwrap();
    assert_eq!(echo.annotations().read_only, Some(true));
    assert_eq!(echo.description(), "Echoes its text.");
    assert_eq!(
        echo.output_schema().unwrap()["properties"]["structuredContent"],
        json!({"type": "object", "properties": {"echo": {"type": "string"}}})
    );
    plugin.shutdown().await;
}

/// Codemode tools are not declared, and the model cannot call them, but
/// a tool calls them through the loop and gets the structured result,
/// an `isError` one included; the call's details carry it for its card. The namespace carries the server's
/// description and instructions.
#[tokio::test(flavor = "multi_thread")]
async fn codemode_tools_are_called_from_tools_only() {
    let fixture = Fixture::new(false);
    let plugin =
        plugin(&fixture, |config| config.exposure = Exposure::Codemode);
    let store = Store::memory().await.unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("mcp__srv__echo", json!({"text": "direct"})))
        .turn(|t| {
            t.tool_call(
                "caller",
                json!({"name": "mcp__srv__echo", "args": {"text": "nested"}, "wait": ["mcp__srv"]}),
            )
        })
        .turn(|t| t.tool_call("caller", json!({"name": "mcp__srv__fail", "args": {}})))
        .turn(|t| t.text("done"));
    let agent = Agent::new(llm.clone()).tool(Caller).plugin(plugin.clone());
    run(&agent, &store).await;

    assert_eq!(declared(&llm, 0), ["caller"]);
    let (direct, failed) = last_result(&llm, 1);
    assert!(failed && direct.contains("not found"), "{direct}");

    let (nested, failed) = last_result(&llm, 2);
    assert!(!failed);
    let nested: Value = serde_json::from_str(&nested).unwrap();
    assert_eq!(nested["result"]["ok"], json!("nested"));
    assert_eq!(
        nested["result"]["structured"],
        json!({
            "content": [{"type": "text", "text": "nested"}],
            "structuredContent": {"echo": "nested"},
            "isError": false
        })
    );
    // The call's details name it for its card, with its hints and the
    // structured result.
    let details = &nested["result"]["details"];
    assert_eq!(
        (&details["server"], &details["tool"]),
        (&json!("srv"), &json!("echo"))
    );
    assert_eq!(details["annotations"]["readOnlyHint"], json!(true));
    assert_eq!(details["structuredContent"], json!({"echo": "nested"}));
    assert_eq!(
        nested["namespaces"],
        json!([{
            "name": "mcp__srv",
            "description": "Use echo to echo.",
            "instructions": "Use echo to echo.\nMore details.",
            "tools": ["mcp__srv__echo", "mcp__srv__fail", "mcp__srv__slow", "mcp__srv__hang", "mcp__srv__crash"]
        }])
    );

    let (failing, _) = last_result(&llm, 3);
    let failing: Value = serde_json::from_str(&failing).unwrap();
    assert_eq!(
        failing["result"]["err"],
        json!("MCP tool srv/fail returned an error")
    );
    assert_eq!(failing["result"]["structured"]["isError"], json!(true));
    let input = first_input(&llm, 0);
    assert!(
        input.contains("- mcp__srv (codemode): Use echo to echo.\n"),
        "{input}"
    );
    plugin.shutdown().await;
}

/// A hidden tool is neither declared nor callable from tools; a glob
/// makes `hang` a codemode tool.
#[tokio::test(flavor = "multi_thread")]
async fn hidden_tools_are_nowhere() {
    let fixture = Fixture::new(false);
    let plugin = plugin(&fixture, |config| {
        config.tool_exposure = vec![
            ("crash".into(), Exposure::Hidden),
            ("h*".into(), Exposure::Codemode),
        ];
    });
    let store = Store::memory().await.unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "caller",
                json!({"name": "mcp__srv__crash", "args": {}}),
            )
        })
        .turn(|t| t.text("done"));
    let agent = Agent::new(llm.clone()).tool(Caller).plugin(plugin.clone());
    run(&agent, &store).await;

    let tools = declared(&llm, 0);
    assert!(
        !tools
            .iter()
            .any(|t| t.ends_with("crash") || t.ends_with("hang")),
        "{tools:?}"
    );
    assert!(tools.contains(&"mcp__srv__echo".to_owned()));
    let (hidden, _) = last_result(&llm, 1);
    let hidden: Value = serde_json::from_str(&hidden).unwrap();
    assert_eq!(
        hidden["result"]["err"],
        json!("Tool mcp__srv__crash not found")
    );
    assert!(
        !hidden["tools"]
            .as_array()
            .unwrap()
            .contains(&json!("mcp__srv__crash"))
    );
    assert!(
        hidden["tools"]
            .as_array()
            .unwrap()
            .contains(&json!("mcp__srv__hang"))
    );
    assert!(plugin.tool("mcp__srv__crash").is_none());
    plugin.shutdown().await;
}

/// A server that connects after a run started is declared from the
/// next run on; within the run, a tool reaches it through the source.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_connects_mid_run_shows_from_the_next_run() {
    let fixture = Fixture::new(false);
    fixture.gate(false);
    let mut config =
        ServerConfig::new("srv", Transport::Stream(fixture.dial()));
    config.description = Some("Late.".into());
    let plugin = McpPlugin::builder()
        .env(Arc::new(|_| None))
        .home(None)
        .server(config)
        .startup_wait(Duration::from_millis(200))
        .build();
    let store = Store::memory().await.unwrap();
    let gate = fixture.clone();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("caller", json!({"name": "mcp__srv__echo", "args": {"text": "late"}, "wait": ["srv"]})))
        .turn(|t| t.text("first done"))
        .turn(|t| t.text("second done"));
    let agent = Agent::new(llm.clone()).tool(Caller).plugin(plugin.clone());
    let opener = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        gate.gate(true);
    });
    run(&agent, &store).await;
    opener.await.unwrap();

    assert_eq!(declared(&llm, 0), ["caller"]);
    let (late, _) = last_result(&llm, 1);
    let late: Value = serde_json::from_str(&late).unwrap();
    assert_eq!(late["result"]["ok"], json!("late"));
    // The block is fixed by the configuration, so it is there already.
    assert!(first_input(&llm, 0).contains("- mcp__srv (direct): Late."));

    run(&agent, &store).await;
    assert!(declared(&llm, 2).contains(&"mcp__srv__echo".to_owned()));
    plugin.shutdown().await;
}

/// Removes the server's tool `echo` and waits until the plugin knows.
struct Withdraw {
    fixture: Arc<Fixture>,
    plugin: McpPlugin,
}

#[async_trait]
impl AgentTool for Withdraw {
    fn name(&self) -> &str {
        "withdraw"
    }
    fn description(&self) -> &str {
        "Withdraws echo."
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| json!({"type": "object", "properties": {}}))
    }
    async fn call(
        &self,
        _args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        self.fixture.remove_tool("echo").await;
        while self.plugin.tool("mcp__srv__echo").is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(ToolOutput::text("withdrawn"))
    }
}

/// A tool the server withdrew during the run fails with its own message.
#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawn_tool_fails() {
    let fixture = Fixture::new(true);
    let plugin = plugin(&fixture, |_| {});
    let store = Store::memory().await.unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("withdraw", json!({})))
        .turn(|t| t.tool_call("mcp__srv__echo", json!({"text": "gone"})))
        .turn(|t| t.text("done"));
    let agent = Agent::new(llm.clone())
        .tool(Withdraw {
            fixture: fixture.clone(),
            plugin: plugin.clone(),
        })
        .plugin(plugin.clone());
    run(&agent, &store).await;
    assert!(declared(&llm, 1).contains(&"mcp__srv__echo".to_owned()));
    assert_eq!(
        last_result(&llm, 2),
        (
            "Tool mcp__srv__echo is no longer offered by server srv".to_owned(),
            true
        )
    );
    plugin.shutdown().await;
}

/// The tool source's `ready` returns once named servers settle, and at
/// once for a cancelled wait.
#[tokio::test(flavor = "multi_thread")]
async fn ready_stops_on_cancel() {
    let fixture = Fixture::new(false);
    fixture.gate(false);
    let plugin = plugin(&fixture, |_| {});
    let source = tau_agent::plugin::Plugin::tool_source(&plugin).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    source.ready(None, &cancel).await;
    assert!(source.tools().is_empty());
    fixture.gate(true);
    source
        .ready(Some(&["mcp__srv".to_owned()]), &CancellationToken::new())
        .await;
    assert_eq!(source.tools().len(), 5);
    plugin.shutdown().await;
}

/// The block stays within 4,096 characters, each description within
/// 250; the servers it keeps come first, in order, and a last line
/// counts the rest.
#[hegel::test(test_cases = 200)]
fn the_server_list_fits_its_limits(tc: TestCase) {
    let servers: Vec<(String, usize)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::text().max_size(400),
            gs::integers().max_value(1_usize)
        ))
        .max_size(60),
    );
    let servers: Vec<(String, Exposure, String)> = servers
        .into_iter()
        .enumerate()
        .map(|(index, (description, exposure))| {
            (
                format!("mcp__server_{index}"),
                [Exposure::Direct, Exposure::Codemode][exposure],
                description,
            )
        })
        .collect();
    let Some(block) = servers_block(&servers) else {
        assert!(servers.is_empty());
        return;
    };
    assert!(
        block.chars().count() <= SERVERS_LIMIT,
        "{}",
        block.chars().count()
    );
    assert!(block.starts_with(&format!("<mcp_servers>\n{SERVERS_INTRO}\n")));
    assert!(block.ends_with("</mcp_servers>"));
    let lines: Vec<&str> = block.lines().collect();
    let body = &lines[2..lines.len() - 1];
    let (kept, overflow) = match body.last() {
        Some(last) if last.starts_with("- … ") => {
            (&body[..body.len() - 1], Some(*last))
        }
        _ => (body, None),
    };
    for (line, (namespace, exposure, _)) in kept.iter().zip(&servers) {
        let prefix = format!("- {namespace} ({exposure})");
        assert!(line.starts_with(&prefix), "{line}");
        let description =
            line[prefix.len()..].strip_prefix(": ").unwrap_or_default();
        assert!(description.chars().count() <= DESCRIPTION_LIMIT);
    }
    match overflow {
        Some(line) => assert_eq!(
            line,
            format!(
                "- … {} more servers; find their tools with search_tools()",
                servers.len() - kept.len()
            )
        ),
        None => assert_eq!(kept.len(), servers.len()),
    }
}

/// A direct server with resources declares the resource tools, and the
/// model lists and reads its resources.
#[tokio::test(flavor = "multi_thread")]
async fn the_model_reads_a_resource() {
    let fixture = Fixture::with_features(false);
    let plugin = plugin(&fixture, |_| {});
    let store = Store::memory().await.unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("list_mcp_resources", json!({})))
        .turn(|t| {
            t.tool_call(
                "read_mcp_resource",
                json!({"server": "srv", "uri": "file:///notes.txt"}),
            )
        })
        .turn(|t| {
            t.tool_call(
                "list_mcp_resource_templates",
                json!({"server": "nope"}),
            )
        })
        .turn(|t| t.text("done"));
    let agent = Agent::new(llm.clone()).plugin(plugin.clone());
    run(&agent, &store).await;

    let tools = declared(&llm, 0);
    for name in [
        "list_mcp_resources",
        "list_mcp_resource_templates",
        "read_mcp_resource",
    ] {
        assert!(tools.contains(&name.to_owned()), "{tools:?}");
    }
    let (listed, failed) = last_result(&llm, 1);
    assert!(!failed);
    let listed: Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(
        listed["resources"][0],
        json!({"server": "srv", "uri": "file:///notes.txt", "name": "notes", "title": "Notes", "mimeType": "text/plain"})
    );
    assert!(!listed.to_string().contains("ui://"));
    assert_eq!(
        last_result(&llm, 2),
        ("Remember the milk.".to_owned(), false)
    );
    assert_eq!(
        last_result(&llm, 3),
        (
            "Unknown MCP server `nope`; servers with resources: srv".to_owned(),
            true
        )
    );
    plugin.shutdown().await;
}

/// With only codemode servers offering resources, the resource tools are
/// `Nested`: not declared, called from tools, with `{ server, uri,
/// contents }` for scripts.
#[tokio::test(flavor = "multi_thread")]
async fn codemode_servers_make_the_resource_tools_nested() {
    let fixture = Fixture::with_features(false);
    let plugin =
        plugin(&fixture, |config| config.exposure = Exposure::Codemode);
    let store = Store::memory().await.unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "caller",
                json!({
                    "name": "read_mcp_resource",
                    "args": {"server": "srv", "uri": "file:///notes.txt"},
                    "wait": ["mcp__srv"]
                }),
            )
        })
        .turn(|t| t.text("done"));
    let agent = Agent::new(llm.clone()).tool(Caller).plugin(plugin.clone());
    run(&agent, &store).await;

    assert_eq!(declared(&llm, 0), ["caller"]);
    assert_eq!(plugin.resource_exposure(), Some(Exposure::Codemode));
    let (nested, failed) = last_result(&llm, 1);
    assert!(!failed);
    let nested: Value = serde_json::from_str(&nested).unwrap();
    assert_eq!(nested["result"]["ok"], json!("Remember the milk."));
    assert_eq!(
        nested["result"]["structured"],
        json!({
            "server": "srv",
            "uri": "file:///notes.txt",
            "contents": [{"uri": "file:///notes.txt", "mimeType": "text/plain", "text": "Remember the milk."}]
        })
    );
    for name in ["list_mcp_resources", "read_mcp_resource"] {
        assert!(
            nested["tools"].as_array().unwrap().contains(&json!(name)),
            "{nested}"
        );
    }
    plugin.shutdown().await;
}

/// Without a server that offers resources, or with only hidden ones,
/// there are no resource tools.
#[tokio::test(flavor = "multi_thread")]
async fn no_resource_tools_without_servers_that_offer_them() {
    let plain = Fixture::new(false);
    let hidden = Fixture::with_features(false);
    let plugin = McpPlugin::builder()
        .env(Arc::new(|_| None))
        .home(None)
        .server(ServerConfig::new("plain", Transport::Stream(plain.dial())))
        .server({
            let mut config =
                ServerConfig::new("secret", Transport::Stream(hidden.dial()));
            config.exposure = Exposure::Hidden;
            config
        })
        .startup_wait(Duration::from_secs(5))
        .build();
    let store = Store::memory().await.unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "caller",
                json!({"name": "list_mcp_resources", "args": {}, "wait": ["plain", "secret"]}),
            )
        })
        .turn(|t| t.text("done"));
    let agent = Agent::new(llm.clone()).tool(Caller).plugin(plugin.clone());
    run(&agent, &store).await;

    assert!(!declared(&llm, 0).iter().any(|t| t.contains("resource")));
    let (result, _) = last_result(&llm, 1);
    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        result["result"]["err"],
        json!("Tool list_mcp_resources not found")
    );
    assert_eq!(plugin.resource_exposure(), None);
    assert!(plugin.resource_tools().is_empty());
    plugin.shutdown().await;
}
