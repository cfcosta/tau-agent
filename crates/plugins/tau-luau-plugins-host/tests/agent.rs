//! Luau plugins in real runs (ADR 0027): a scripted model calls a
//! plugin's tool, which draws its card; `before_tool` blocks a call;
//! `before_stop` keeps the run going once; `turn_end` counts turns into
//! the plugin's state, stored with the run with the view drawn from it;
//! and a plugin whose hooks keep failing is turned off.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    event::RunEvent,
    tool::{AgentTool, RunId, ToolCtx, ToolOutput},
};
use tau_luau_plugins::{NAME, Record};
use tau_luau_plugins_host::{
    agent::{Active, LuauPlugins},
    runtime::{Files, load},
    testing::PluginTesting,
};
use tau_store::Store;
use tau_testing::{block_on_io, scripted::ScriptedModel};

const EVERY_DAY: &str = r#"
local tau = require("tau")
local ui = tau.ui

return tau.plugin {
  name = "no-deploys",
  uses = { tools = { "bash" } },
  settings = { default = { days = { "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday" } } },
  tools = {
    deploy_window = {
      description = "Whether now is a safe time to deploy.",
      call = function(args, ctx)
        return { ok = not tau.contains(ctx.settings.days, ctx.now.weekday) }
      end,
      card = function(call, result, ctx)
        return ui.badge(result.ok and "safe" or "wait", result.ok and "good" or "warn")
      end,
    },
  },
  before_tool = function(call, ctx)
    if call.name == "bash" and call.args.command:find("deploy") then
      return tau.block("No deploys on " .. ctx.now.weekday .. ".")
    end
  end,
  before_stop = function(stop, ctx)
    if not stop.text:find("tests") then
      return tau.continue("Say which tests you ran.")
    end
  end,
  turn_end = function(turn, ctx)
    ctx.state.turns = (ctx.state.turns or 0) + 1
  end,
  view = function(state, ctx)
    return { status = ui.status("on", tostring(state.turns or 0) .. " turns") }
  end,
}
"#;

/// `bash`, answering with what it ran.
struct Bash;

#[async_trait]
impl AgentTool for Bash {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Runs a command."
    }

    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(|| json!({ "type": "object" }));
        &SCHEMA
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(format!("ran {}", args["command"])))
    }
}

fn active(name: &str, source: &str) -> Active {
    let files = Files {
        plugin: source.into(),
        libs: BTreeMap::new(),
        ..Files::default()
    };
    let loaded = block_on_io(load(name, files)).unwrap();
    let settings = loaded.declaration.default_settings();
    Active { loaded, settings }
}

fn records(store: &Store, run: &RunId) -> Vec<Record> {
    block_on_io(store.records(&run.0, NAME))
        .unwrap()
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .collect()
}

#[test]
fn a_plugin_takes_part_in_a_run() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("deploy_window", json!({})))
        .turn(|t| t.tool_call("bash", json!({ "command": "make deploy" })))
        .turn(|t| t.text("done"))
        .turn(|t| t.text("ran the tests"));
    let store = block_on_io(tau_store_sqlite::memory()).unwrap();
    let plugins = LuauPlugins::new(
        vec![active("no-deploys", EVERY_DAY)],
        json!({ "kind": "main", "repo": "r", "model": "m" }),
        None,
    );
    let agent = Agent::new(llm.clone()).tool(Bash).plugin(plugins);
    let (outcome, events) = block_on_io(async {
        let mut run = agent.start("deploy it", &store);
        let events: Vec<RunEvent> = run.events().collect().await;
        (run.outcome().await.unwrap(), events)
    });
    assert_eq!(outcome.text, "ran the tests");

    // The tool answered, with its card in the details.
    let window = events
        .iter()
        .find_map(|event| match event {
            RunEvent::ToolEnd { output, .. }
                if output
                    .details
                    .as_ref()
                    .is_some_and(|d| d["plugin"] == "no-deploys") =>
            {
                output.details.clone()
            }
            _ => None,
        })
        .expect("the tool's result");
    assert_eq!(window["value"], json!({ "ok": false }));
    assert_eq!(
        window["card"],
        json!({ "piece": "badge", "text": "wait", "tone": "warn" })
    );

    // The deploy was blocked, and the stop held once.
    let asked = format!("{:?}", llm.requests().last().unwrap().transcript);
    assert!(asked.contains("No deploys on"), "{asked}");
    assert!(asked.contains("Say which tests you ran."), "{asked}");

    // Four turns counted, and the view followed the count.
    let stored = records(&store, &outcome.run);
    let last_state = stored.iter().rev().find_map(|record| match record {
        Record::State { state, .. } => Some(state.clone()),
        _ => None,
    });
    assert_eq!(last_state, Some(json!({ "turns": 4 })));
    let last_view = stored.iter().rev().find_map(|record| match record {
        Record::View { view, .. } => Some(view.clone()),
        _ => None,
    });
    assert_eq!(last_view.unwrap()["status"]["detail"], "4 turns");
    assert!(
        !stored
            .iter()
            .any(|record| matches!(record, Record::Error { .. })),
        "{stored:?}"
    );
}

#[test]
fn a_plugin_whose_hooks_keep_failing_is_turned_off() {
    let broken = r#"
local tau = require("tau")
return tau.plugin {
  name = "broken",
  before_tool = function(call, ctx) error("no luck") end,
}
"#;
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("bash", json!({ "command": "a" })))
        .turn(|t| t.tool_call("bash", json!({ "command": "b" })))
        .turn(|t| t.tool_call("bash", json!({ "command": "c" })))
        .turn(|t| t.tool_call("bash", json!({ "command": "d" })))
        .turn(|t| t.text("done"));
    let store = block_on_io(tau_store_sqlite::memory()).unwrap();
    let plugins =
        LuauPlugins::new(vec![active("broken", broken)], json!({}), None);
    let agent = Agent::new(llm.clone()).tool(Bash).plugin(plugins);
    let outcome = block_on_io(agent.run("go", &store)).unwrap();
    assert_eq!(outcome.text, "done");
    // Every call ran: a failing hook allows.
    let asked = format!("{:?}", llm.requests().last().unwrap().transcript);
    for command in ["a", "b", "c", "d"] {
        assert!(
            asked.contains(&format!("ran \\\"{command}\\\"")),
            "{command}: {asked}"
        );
    }
    let stored = records(&store, &outcome.run);
    let errors = stored
        .iter()
        .filter(|record| matches!(record, Record::Error { .. }))
        .count();
    assert_eq!(errors, 3, "it stops being asked once off: {stored:?}");
    assert!(
        stored
            .iter()
            .any(|record| matches!(record, Record::Off { .. }))
    );
}

#[test]
fn a_run_tests_a_plugin_in_its_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("greet");
    std::fs::create_dir_all(folder.join("tests")).unwrap();
    std::fs::write(
        folder.join("plugin.luau"),
        r#"
local tau = require("tau")
return tau.plugin {
  name = "greet",
  tools = { hello = { description = "Hello.", call = function() return "hi" end } },
}
"#,
    )
    .unwrap();
    std::fs::write(
        folder.join("tests/basic.luau"),
        r#"
local t = require("tau").test
t.case("says hi", function() t.equal(t.run{}:tool("hello"), "hi") end)
t.case("says bye", function() t.equal(t.run{}:tool("hello"), "bye") end)
"#,
    )
    .unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("plugin_test", json!({ "plugin": "greet" })))
        .turn(|t| t.tool_call("plugin_test", json!({ "plugin": "../x" })))
        .turn(|t| t.tool_call("plugin_test", json!({ "plugin": "missing" })))
        .turn(|t| t.text("done"));
    let store = block_on_io(tau_store_sqlite::memory()).unwrap();
    let agent = Agent::new(llm.clone())
        .plugin(PluginTesting::new(dir.path().to_owned()));
    block_on_io(agent.run("test greet", &store)).unwrap();
    let asked = format!("{:?}", llm.requests().last().unwrap().transcript);
    assert!(asked.contains("greet loads; 1 of 2 tests pass."), "{asked}");
    assert!(asked.contains("says bye failed"), "{asked}");
    assert!(asked.contains("is not a plugin's folder"), "{asked}");
    assert!(asked.contains("missing"), "{asked}");
}

const COUNTER_V1: &str = r#"
local tau = require("tau")
return tau.plugin {
  name = "counter",
  tools = {
    mark = {
      description = "Counts a mark.",
      call = function(args, ctx)
        ctx.state.n = (ctx.state.n or 0) + 1
        return "v1 " .. ctx.state.n
      end,
    },
  },
}
"#;

const COUNTER_V2: &str = r#"
local tau = require("tau")
return tau.plugin {
  name = "counter",
  tools = {
    mark = {
      description = "Counts a mark.",
      call = function(args, ctx)
        ctx.state.n = (ctx.state.n or 0) + 1
        return "v2 " .. ctx.state.n
      end,
    },
  },
}
"#;

/// Versions a test sets while a run goes on.
#[derive(Clone, Default)]
struct Settable(std::sync::Arc<std::sync::Mutex<Vec<Active>>>);

#[async_trait]
impl tau_luau_plugins_host::agent::Versions for Settable {
    async fn now(&self) -> Vec<Active> {
        self.0.lock().unwrap().clone()
    }
}

/// `swap`: makes `to` the active versions.
struct Swap {
    versions: Settable,
    to: Vec<Active>,
}

#[async_trait]
impl AgentTool for Swap {
    fn name(&self) -> &str {
        "swap"
    }

    fn description(&self) -> &str {
        "Changes the plugins."
    }

    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(|| json!({ "type": "object" }));
        &SCHEMA
    }

    async fn call(
        &self,
        _args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        *self.versions.0.lock().unwrap() = self.to.clone();
        Ok(ToolOutput::text("swapped"))
    }
}

/// `relay`: calls `mark` as code mode would, from a tool.
struct Relay;

#[async_trait]
impl AgentTool for Relay {
    fn name(&self) -> &str {
        "relay"
    }

    fn description(&self) -> &str {
        "Calls mark."
    }

    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(|| json!({ "type": "object" }));
        &SCHEMA
    }

    async fn call(
        &self,
        _args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let output = ctx
            .call("mark", json!({}))
            .await
            .map_err(|error| ToolError::from(error.to_string()))?;
        Ok(ToolOutput::text(output.text_content()))
    }
}

/// What each `relay` and `mark` call gave, in order.
fn results(events: &[RunEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolEnd { output, .. } => Some(output.text_content()),
            _ => None,
        })
        .filter(|text| text != "swapped")
        .collect()
}

/// A plugin that becomes active while a run goes on works in it from its
/// next request, reached from a tool as code mode reaches it, without
/// changing the tools the requests declare.
#[test]
fn a_plugin_that_becomes_active_mid_run_works_at_once() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("swap", json!({})))
        .turn(|t| t.tool_call("relay", json!({})))
        .turn(|t| t.text("done"));
    let store = block_on_io(tau_store_sqlite::memory()).unwrap();
    let versions = Settable::default();
    let plugins = block_on_io(LuauPlugins::live(
        std::sync::Arc::new(versions.clone()),
        json!({ "kind": "chat", "repo": "tau-plugins", "model": "m" }),
        None,
    ));
    let agent = Agent::new(llm.clone())
        .tool(Swap {
            versions,
            to: vec![active("counter", COUNTER_V1)],
        })
        .tool(Relay)
        .plugin(plugins);
    let events: Vec<RunEvent> = block_on_io(async {
        let mut run = agent.start("mark one", &store);
        let events = run.events().collect().await;
        run.outcome().await.unwrap();
        events
    });
    assert_eq!(results(&events), ["v1 1", "v1 1"], "{events:#?}");
    for request in llm.requests() {
        let tools: Vec<&str> = request
            .settings
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(!tools.contains(&"mark"), "declared: {tools:?}");
    }
}

/// A plugin's new version takes over at the run's next request, with the
/// state the earlier one left.
#[test]
fn a_new_version_takes_over_with_its_state() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("mark", json!({})))
        .turn(|t| t.tool_call("swap", json!({})))
        .turn(|t| t.tool_call("mark", json!({})))
        .turn(|t| t.text("done"));
    let store = block_on_io(tau_store_sqlite::memory()).unwrap();
    let versions = Settable::default();
    *versions.0.lock().unwrap() = vec![active("counter", COUNTER_V1)];
    let plugins = block_on_io(LuauPlugins::live(
        std::sync::Arc::new(versions.clone()),
        json!({ "kind": "chat", "repo": "tau-plugins", "model": "m" }),
        None,
    ));
    let agent = Agent::new(llm)
        .tool(Swap {
            versions,
            to: vec![active("counter", COUNTER_V2)],
        })
        .plugin(plugins);
    let events: Vec<RunEvent> = block_on_io(async {
        let mut run = agent.start("mark twice", &store);
        let events = run.events().collect().await;
        run.outcome().await.unwrap();
        events
    });
    assert_eq!(results(&events), ["v1 1", "v2 2"], "{events:#?}");
}
