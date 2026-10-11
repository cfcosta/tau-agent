//! The `tau` module's half for agents (ADR 0037): what an agent's
//! `agent.luau` declares, and its tests, run by the runtime Luau
//! plugins share.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use tau_luau_plugins_host::runtime::{Files, Root, evaluate, run_tests};
use tokio_util::sync::CancellationToken;

/// An agent's repository: `agent.luau`, required as `agent`.
const AGENT: Root = Root {
    file: "agent.luau",
    module: "agent",
    subject: "_agent",
};

const TRIAGE: &str = r#"
local tau = require("tau")

return tau.agent {
  name = "pr-triage",
  triggers = {
    pull_requests = {
      poll = { tool = "mcp__github__list_pull_requests", args = { state = "open" } },
      every = "10m",
      key = function(pr) return pr.number end,
    },
    releases = {
      poll = { http = { url = "https://example.com/releases", headers = { authorization = tau.secret("github") } } },
      every = "1h",
      key = "id",
    },
    builds = {
      stream = { sse = "https://ci.example.com/events" },
      parse = function(frame, state)
        if frame.event == "ping" then return nil end
        local build = json.decode(frame.data)
        return { id = "build:" .. build.number, data = build }
      end,
    },
    ci_status = { mcp = { server = "ci", uri = "ci://tau-agent/main" } },
    evening = { at = "18:00", days = { "Mon", "Fri" } },
    hourly = { every = "1h" },
    triage = { command = "pr-triage", hint = "<number>" },
    inbox = { message = true },
  },
}
"#;

fn agent(source: &str) -> Files {
    Files {
        root: AGENT,
        source: source.into(),
        ..Files::default()
    }
}

async fn describe(files: &Files) -> Result<Value, String> {
    let modules = Arc::new(files.modules()?);
    evaluate(
        "pr-triage",
        modules,
        "local tau = require(\"tau\")\n\
         return tau._describe_agent(require(\"agent\"))\n"
            .into(),
        Duration::from_secs(2),
        CancellationToken::new(),
    )
    .await
}

#[tokio::test]
async fn an_agent_declares_what_wakes_it() {
    let described = describe(&agent(TRIAGE)).await.unwrap();
    assert_eq!(described["name"], "pr-triage");
    assert_eq!(
        described["triggers"],
        json!([
            { "name": "builds", "kind": { "kind": "stream", "url": "https://ci.example.com/events" } },
            { "name": "ci_status", "kind": { "kind": "mcp", "server": "ci", "uri": "ci://tau-agent/main" } },
            { "name": "evening", "kind": { "kind": "schedule", "at": "18:00", "days": ["Mon", "Fri"] } },
            { "name": "hourly", "kind": { "kind": "schedule", "every": "1h", "days": [] } },
            { "name": "inbox", "kind": { "kind": "message" } },
            { "name": "pull_requests", "kind": { "kind": "poll", "source": "tool mcp__github__list_pull_requests", "every": "10m" } },
            { "name": "releases", "kind": { "kind": "poll", "source": "https://example.com/releases", "every": "1h" } },
            { "name": "triage", "kind": { "kind": "command", "command": "pr-triage", "hint": "<number>" } },
        ])
    );
}

#[tokio::test]
async fn a_trigger_that_is_not_one_says_why() {
    let cases = [
        ("{ every = \"1h\", at = \"09:00\" }", "not both"),
        ("{ every = \"1h\", command = \"x\" }", "must be one of"),
        ("{}", "must be one of"),
        ("{ poll = { tool = \"t\" }, every = \"1m\" }", "needs `key`"),
        ("{ at = \"09:00\", days = { \"Funday\" } }", "not a day"),
        ("{ stream = { sse = \"u\" } }", "needs `parse`"),
    ];
    for (trigger, why) in cases {
        let source = format!(
            "local tau = require(\"tau\")\n\
             return tau.agent {{ name = \"a\", triggers = {{ t = {trigger} }} }}\n"
        );
        let error = describe(&agent(&source)).await.unwrap_err();
        assert!(error.contains(why), "{trigger}: {error}");
    }
    let unnamed =
        "local tau = require(\"tau\")\nreturn tau.agent { triggers = {} }\n";
    let error = describe(&agent(unnamed)).await.unwrap_err();
    assert!(error.contains("needs a name"), "{error}");
}

const TESTS: &str = r#"
local tau = require("tau")
local t = tau.test

t.case("a ping is not an event", function()
  local agent = t.agent {}
  t.equal(agent:parse("builds", { event = "ping", data = "" }), nil)
end)

t.case("a build frame is an event keyed by its number", function()
  local agent = t.agent {}
  local event = agent:parse("builds", { event = "build", data = '{"number":7,"state":"failed"}' })
  t.equal(event.id, "build:7")
end)

t.case("a poll fires only for keys it has not seen", function()
  local agent = t.agent { state = { pull_requests = { seen = { 412 } } } }
  local events = agent:poll("pull_requests", { { number = 412 }, { number = 413 } })
  t.equal(#events, 1)
  t.equal(events[1].id, "pull_requests:413")
end)

t.case("the first poll only records keys", function()
  local agent = t.agent {}
  t.equal(#agent:poll("releases", { { id = 1 }, { id = 2 } }), 0)
  t.equal(#agent:poll("releases", { { id = 2 }, { id = 3 } }), 1)
end)

t.case("it declares an evening schedule", function()
  local agent = t.agent {}
  t.equal(agent:describe().triggers[3].name, "evening")
end)

t.case("this one fails", function()
  t.equal(1, 2)
end)
"#;

#[tokio::test]
async fn an_agents_tests_run_against_a_fake_agent() {
    let mut files = agent(TRIAGE);
    files.tests.insert("triage".into(), TESTS.into());
    let modules = Arc::new(files.modules().unwrap());
    let results =
        run_tests("pr-triage", modules, &files, CancellationToken::new()).await;
    let outcome: Vec<(&str, bool)> = results
        .iter()
        .map(|result| (result.name.as_str(), result.passed))
        .collect();
    assert_eq!(
        outcome,
        [
            ("a ping is not an event", true),
            ("a build frame is an event keyed by its number", true),
            ("a poll fires only for keys it has not seen", true),
            ("the first poll only records keys", true),
            ("it declares an evening schedule", true),
            ("this one fails", false),
        ],
        "{results:?}"
    );
}

#[tokio::test]
async fn an_agents_folder_reads_from_its_paths() {
    let paths = vec![
        ("agent.luau".to_owned(), TRIAGE.as_bytes().to_vec()),
        ("lib/x.luau".to_owned(), b"return {}".to_vec()),
        ("AGENTS.md".to_owned(), b"# pr-triage".to_vec()),
    ];
    let files = Files::from_paths(&paths, AGENT).unwrap();
    assert_eq!(files.root, AGENT);
    assert!(files.libs.contains_key("x"));
    let missing = Files::from_paths(&paths[1..], AGENT).unwrap_err();
    assert!(missing.contains("agent.luau"), "{missing}");
    // `agent` is its own module's name: a lib cannot take it.
    let mut taken = agent(TRIAGE);
    taken.libs.insert("agent".into(), "return {}".into());
    assert!(taken.modules().unwrap_err().contains("taken"));
}
