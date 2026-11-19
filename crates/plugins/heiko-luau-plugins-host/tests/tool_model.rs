//! Luau plugins' tools and hooks sharing a run, against a model of what
//! the run must come to. Two plugins each have a tool that answers at
//! once and one that calls another tool through the loop, and count every
//! call their `before_tool` and `after_tool_result` see. A scripted model
//! asks for turns of calls side by side, each a leaf, a Rust tool, or a
//! relay through a plugin's tool to another call, nested. Whatever the
//! plan, the run ends (a run that waits on itself is a failure, not a
//! hang), each call answers what the model says, and every plugin saw
//! every call, the nested ones too, once before and once after.
//!
//! What it holds to failed twice before: the loop stopped polling the
//! calls beside one whose `after_tool_result` waited on the run's state,
//! and a Luau tool held that state while the calls it made went through
//! every plugin's `before_tool`.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use hegel::{TestCase, generators as gs};
use heiko_agent::{
    agent::Agent,
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use heiko_ai::message::{InputBlock, Message};
use heiko_luau_plugins::{NAME, Record};
use heiko_luau_plugins_host::{
    agent::{Active, LuauPlugins},
    runtime::{Files, load},
};
use heiko_testing::{block_on_io, scripted::ScriptedModel};
use serde_json::{Value, json};

/// Which plugin a tool is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    A,
    B,
}

impl Side {
    fn letter(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }
}

/// One call the run makes.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    /// A plugin's tool that answers its letter.
    Leaf(Side),
    /// The Rust tool `echo`, answering its text.
    Echo(u8),
    /// A plugin's tool that calls `inner` through the loop, and answers
    /// what it answered, in brackets.
    Relay(Side, Box<Call>),
}
hegel::pretty_print_as_debug!(Call);

impl Call {
    /// The tool and its arguments. Every call's arguments hold a key: a
    /// table with none crosses as a list.
    fn tool(&self) -> (String, Value) {
        match self {
            Self::Leaf(side) => {
                (format!("leaf_{}", side.letter()), json!({ "tag": 1 }))
            }
            Self::Echo(n) => ("echo".into(), json!({ "text": n.to_string() })),
            Self::Relay(side, inner) => {
                let (target, args) = inner.tool();
                (
                    format!("relay_{}", side.letter()),
                    json!({ "target": target, "args": args }),
                )
            }
        }
    }

    /// What the model says it answers.
    fn answer(&self) -> String {
        match self {
            Self::Leaf(side) => side.letter().into(),
            Self::Echo(n) => n.to_string(),
            Self::Relay(_, inner) => format!("({})", inner.answer()),
        }
    }

    /// The calls it is, its nested ones with it: what every plugin's
    /// hooks see.
    fn calls(&self) -> u64 {
        match self {
            Self::Relay(_, inner) => 1 + inner.calls(),
            _ => 1,
        }
    }
}

/// A call nested at most `depth` deep.
#[hegel::composite]
fn call(tc: &TestCase, depth: u8) -> Call {
    let most = if depth == 0 { 1 } else { 2 };
    let side = if tc.draw(gs::booleans()) {
        Side::A
    } else {
        Side::B
    };
    match tc.draw(gs::integers::<u8>().max_value(most)) {
        0 => Call::Leaf(side),
        1 => Call::Echo(tc.draw(gs::integers::<u8>().max_value(9))),
        _ => Call::Relay(side, Box::new(tc.draw(call(depth - 1)))),
    }
}

/// A plugin whose tools end in `letter`, counting what its hooks see.
fn plugin(letter: &str) -> String {
    format!(
        r#"
local heiko = require("heiko")
return heiko.plugin {{
  uses = {{ tools = {{ "echo", "leaf_a", "leaf_b", "relay_a", "relay_b" }} }},
  tools = {{
    leaf_{letter} = {{
      description = "Answers {letter}.",
      call = function() return "{letter}" end,
    }},
    relay_{letter} = {{
      description = "Calls a tool through the loop.",
      parameters = {{ type = "object" }},
      call = function(args)
        return "(" .. heiko.await(heiko.cmd.tool(args.target, args.args)) .. ")"
      end,
    }},
  }},
  before_tool = function(_, model) model.before = (model.before or 0) + 1 end,
  after_tool_result = function(_, model) model.after = (model.after or 0) + 1 end,
}}
"#
    )
}

/// `echo`, answering its text.
struct Echo;

#[async_trait]
impl AgentTool for Echo {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Answers its text."
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
        Ok(ToolOutput::text(args["text"].as_str().unwrap_or_default()))
    }
}

fn active(name: &str, source: &str) -> Active {
    let files = Files {
        source: source.into(),
        ..Files::named(name)
    };
    let loaded = block_on_io(load(name, files)).unwrap();
    let settings = loaded.declaration.default_settings();
    Active { loaded, settings }
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

#[hegel::test(test_cases = 60)]
fn luau_tools_and_hooks_keep_to_their_model(tc: TestCase) {
    let turns: Vec<Vec<Call>> = tc.draw(
        gs::vecs(gs::vecs(call(2)).min_size(1).max_size(4))
            .min_size(1)
            .max_size(3),
    );
    holds(&turns);
}

/// Found by the model with the loop that stopped polling the calls
/// beside one whose `after_tool_result` waited: two of a plugin's tools
/// side by side.
#[test]
fn two_luau_tools_run_side_by_side() {
    holds(&[vec![Call::Leaf(Side::B), Call::Leaf(Side::B)]]);
}

/// Found by the model with the Luau tool that held the run's state: a
/// plugin's tool calls another through the loop, whose `before_tool`
/// hooks take that state.
#[test]
fn a_luau_tool_calls_a_luau_tool() {
    holds(&[vec![Call::Relay(Side::B, Box::new(Call::Leaf(Side::B)))]]);
}

/// The run of `turns` keeps to the model.
fn holds(turns: &[Vec<Call>]) {
    let mut llm = ScriptedModel::new();
    for turn in turns {
        let turn = turn.clone();
        llm = llm.turn(move |mut t| {
            for call in &turn {
                let (tool, args) = call.tool();
                t = t.tool_call(&tool, args);
            }
            t
        });
    }
    let llm = llm.turn(|t| t.text("done"));
    let store = block_on_io(heiko_store_sqlite::memory()).unwrap();
    let plugins = LuauPlugins::new(
        vec![active("alpha", &plugin("a")), active("beta", &plugin("b"))],
        json!({}),
        None,
    );
    let agent = Agent::new(llm.clone()).tool(Echo).plugin(plugins);
    let outcome = block_on_io(async {
        tokio::time::timeout(Duration::from_secs(10), agent.run("go", &store))
            .await
            .expect("the run waited on itself")
    })
    .unwrap();
    assert_eq!(outcome.text, "done");

    // Each call answered what the model says, in order, none failed.
    let answered: Vec<(String, bool)> = llm
        .requests()
        .last()
        .unwrap()
        .transcript
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => {
                Some((text(&result.content), result.is_error))
            }
            _ => None,
        })
        .collect();
    let expected: Vec<(String, bool)> = turns
        .iter()
        .flatten()
        .map(|call| (call.answer(), false))
        .collect();
    assert_eq!(answered, expected);

    // Every plugin saw every call, once before and once after.
    let calls: u64 = turns.iter().flatten().map(Call::calls).sum();
    let mut last: BTreeMap<String, Value> = BTreeMap::new();
    for body in block_on_io(store.records(&outcome.run.0, NAME)).unwrap() {
        if let Ok(Record::State { plugin, state }) = serde_json::from_str(&body)
        {
            last.insert(plugin, state);
        }
    }
    for plugin in ["alpha", "beta"] {
        let state = &last[plugin];
        assert_eq!(state["before"], json!(calls), "{plugin}'s before_tool");
        assert_eq!(
            state["after"],
            json!(calls),
            "{plugin}'s after_tool_result"
        );
    }
}
