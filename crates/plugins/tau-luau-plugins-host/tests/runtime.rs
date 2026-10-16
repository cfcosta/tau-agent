//! Loading a Luau plugin and calling its hooks (ADR 0027): what it
//! declares, what each hook answers, the state it keeps, what it may
//! reach, and how a hook fails.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_codemode_host::{ToolCall, ToolEntry, ToolReply};
use tau_luau_plugins::{Hooks, Uses};
use tau_luau_plugins_host::runtime::{
    Context,
    Files,
    Hook,
    NoReach,
    Reach,
    evaluate,
    load,
    long_string,
};
use tokio_util::sync::CancellationToken;

const FRIDAY: &str = r#"
local tau = require("tau")
local ui = tau.ui
local days = require("days")

return tau.plugin {
  name = "no-friday-deploys",
  description = "Blocks deploy commands on Fridays.",
  uses = { tools = { "exec_command" } },
  settings = {
    schema = { type = "object" },
    default = { days = { "Friday" } },
  },
  tools = {
    deploy_window = {
      description = "Whether now is a safe time to deploy.",
      call = function(args, ctx)
        return { ok = not days.blocked(ctx) }
      end,
      card = function(call, result, ctx)
        return ui.badge(result.ok and "safe" or "wait", result.ok and "good" or "warn")
      end,
    },
    run_it = {
      description = "Runs a command.",
      parameters = { type = "object", properties = { cmd = { type = "string" } } },
      call = function(args, ctx)
        return ctx.tools.exec_command({ cmd = args.cmd })
      end,
    },
    sneak = {
      description = "Reads a file it may not.",
      call = function(args, ctx)
        return ctx.tools.read({ path = "secret" })
      end,
    },
  },
  before_tool = function(call, ctx)
    if call.name == "exec_command" and call.args.cmd:find("deploy") and days.blocked(ctx) then
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
    ctx.log("turn " .. turn.turn)
  end,
  view = function(state, ctx)
    return { status = ui.status("on", tostring(state.turns or 0) .. " turns") }
  end,
  actions = {
    reset = function(args, ctx) ctx.state.turns = 0 end,
  },
}
"#;

const DAYS: &str = r#"
local tau = require("tau")
return {
  blocked = function(ctx)
    return tau.contains(ctx.settings.days, ctx.now.weekday)
  end,
}
"#;

fn files(plugin: &str) -> Files {
    Files {
        source: plugin.into(),
        libs: BTreeMap::from([("days".into(), DAYS.into())]),
        ..Files::default()
    }
}

fn context(weekday: &str) -> Context {
    Context {
        run: json!({ "id": "r1", "kind": "main", "turn": 1 }),
        now: json!({ "unix": 0, "iso": "2026-10-09T10:00:00Z", "weekday": weekday }),
        settings: json!({ "days": ["Friday"] }),
        state: json!({}),
    }
}

fn no_reach() -> Arc<dyn Reach> {
    Arc::new(NoReach)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_declares_what_it_is() {
    let loaded = load("no-friday-deploys", files(FRIDAY)).await.unwrap();
    let declared = &loaded.declaration;
    assert_eq!(declared.description, "Blocks deploy commands on Fridays.");
    assert_eq!(
        declared.uses,
        Uses {
            tools: vec!["exec_command".into()],
            jev: false,
            infer: false,
        }
    );
    let tools: Vec<(&str, bool)> = declared
        .tools
        .iter()
        .map(|tool| (tool.name.as_str(), tool.card))
        .collect();
    assert_eq!(
        tools,
        [("deploy_window", true), ("run_it", false), ("sneak", false)]
    );
    assert_eq!(
        declared.tools[0].parameters,
        json!({ "type": "object", "properties": {} })
    );
    assert_eq!(
        declared.hooks,
        Hooks {
            before_tool: true,
            before_stop: true,
            turn_end: true,
            run_end: false,
            view: true,
            settings_view: false,
        }
    );
    assert_eq!(declared.actions, ["reset"]);
    assert_eq!(declared.default_settings(), json!({ "days": ["Friday"] }));
    assert_eq!(loaded.digest.len(), 64);
}

#[tokio::test(flavor = "multi_thread")]
async fn hooks_answer_as_the_plugin_says() {
    let loaded = load("no-friday-deploys", files(FRIDAY)).await.unwrap();
    let call = |weekday: &'static str, hook: Hook, input: Value| {
        let loaded = loaded.clone();
        async move {
            loaded
                .call(
                    &hook,
                    input,
                    &context(weekday),
                    no_reach(),
                    CancellationToken::new(),
                )
                .await
        }
    };
    let deploy = json!({ "id": "c1", "name": "exec_command", "args": { "cmd": "make deploy" } });
    let blocked = call("Friday", Hook::BeforeTool, deploy.clone()).await;
    assert_eq!(blocked.error, None);
    assert_eq!(
        blocked.value,
        json!({ "decision": "block", "reason": "No deploys on Friday." })
    );
    let allowed = call("Monday", Hook::BeforeTool, deploy).await;
    assert_eq!(allowed.value, Value::Null);

    let stop = call(
        "Monday",
        Hook::BeforeStop,
        json!({ "text": "done", "turn": 2 }),
    )
    .await;
    assert_eq!(
        stop.value,
        json!({ "decision": "continue", "message": "Say which tests you ran." })
    );

    let window =
        call("Friday", Hook::Tool("deploy_window".into()), json!({})).await;
    assert_eq!(window.value, json!({ "ok": false }));
    let card = call(
        "Friday",
        Hook::Card("deploy_window".into()),
        json!({ "call": {}, "result": { "ok": false } }),
    )
    .await;
    assert_eq!(
        card.value,
        json!({ "piece": "badge", "text": "wait", "tone": "warn" })
    );

    // A hook it does not have answers nothing.
    let none = call("Friday", Hook::RunEnd, json!({})).await;
    assert_eq!((none.value, none.error), (Value::Null, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn state_carries_from_hook_to_hook() {
    let loaded = load("no-friday-deploys", files(FRIDAY)).await.unwrap();
    let mut ctx = context("Monday");
    for turn in 1..=3 {
        let outcome = loaded
            .call(
                &Hook::TurnEnd,
                json!({ "turn": turn }),
                &ctx,
                no_reach(),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.logs, [format!("turn {turn}")]);
        ctx.state = outcome.state;
    }
    assert_eq!(ctx.state, json!({ "turns": 3 }));
    let view = loaded
        .call(
            &Hook::View,
            Value::Null,
            &ctx,
            no_reach(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(
        view.value,
        json!({ "status": { "piece": "status", "text": "on", "detail": "3 turns", "tone": "neutral" } })
    );
    let reset = loaded
        .call(
            &Hook::Action("reset".into()),
            json!({}),
            &ctx,
            no_reach(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(reset.state, json!({ "turns": 0 }));
}

/// Answers `exec_command` with what it was asked; has `read` too, which the
/// plugin does not use. Keeps the name of every call that reached it.
#[derive(Default)]
struct Shell(std::sync::Mutex<Vec<String>>);

#[async_trait]
impl Reach for Shell {
    fn tools(&self) -> Vec<ToolEntry> {
        ["exec_command", "read"]
            .into_iter()
            .map(|name| ToolEntry {
                name: name.into(),
                description: String::new(),
                input_schema: json!({ "type": "object" }),
                output_schema: None,
                namespace: None,
                sequential: false,
            })
            .collect()
    }

    async fn call(&self, call: ToolCall) -> Result<ToolReply, String> {
        self.0.lock().unwrap().push(call.name.clone());
        Ok(ToolReply::success(json!(format!(
            "ran {}",
            call.args["cmd"]
        ))))
    }
}

/// A plugin's tool reaches the run's tools it declares in `uses`, and no
/// other: `read`, which the run has but the plugin does not declare, is
/// not there for it, and the run's `read` is never called.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_reaches_only_what_it_uses() {
    let loaded = load("no-friday-deploys", files(FRIDAY)).await.unwrap();
    let ctx = context("Monday");
    let shell = Arc::new(Shell::default());
    let ran = loaded
        .call(
            &Hook::Tool("run_it".into()),
            json!({ "cmd": "ls" }),
            &ctx,
            shell.clone(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!((ran.value, ran.error), (json!("ran \"ls\""), None));
    let sneaked = loaded
        .call(
            &Hook::Tool("sneak".into()),
            json!({}),
            &ctx,
            shell.clone(),
            CancellationToken::new(),
        )
        .await;
    let error = sneaked
        .error
        .as_deref()
        .unwrap_or_else(|| panic!("{sneaked:?}"));
    // `ctx.tools.read` is nil: the plugin never sees the tool.
    assert!(error.ends_with("attempt to call a nil value"), "{error}");
    assert_eq!(*shell.0.lock().unwrap(), ["exec_command"]);
}

/// A hook that fails says why and leaves the state as it was; one that
/// runs past its limit says so. The loop is in a card, whose limit is
/// 200 ms, so the test does not wait out a 2 s one.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_hook_says_why_and_keeps_the_state() {
    let plugin = r#"
local tau = require("tau")
return tau.plugin {
  name = "broken",
  tools = {
    slow = {
      description = "Never draws its card.",
      call = function(args, ctx) return 1 end,
      card = function(call, result, ctx)
        while true do end
      end,
    },
  },
  turn_end = function(turn, ctx)
    ctx.state.seen = true
    error("no luck")
  end,
}
"#;
    let loaded = load("broken", files(plugin)).await.unwrap();
    let mut ctx = context("Monday");
    ctx.state = json!({ "kept": 1 });
    let failed = loaded
        .call(
            &Hook::TurnEnd,
            json!({ "turn": 1 }),
            &ctx,
            no_reach(),
            CancellationToken::new(),
        )
        .await;
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|e| e.contains("no luck")),
        "{failed:?}"
    );
    assert_eq!(failed.state, json!({ "kept": 1 }));
    let looped = loaded
        .call(
            &Hook::Card("slow".into()),
            json!({ "call": {}, "result": 1 }),
            &ctx,
            no_reach(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(
        looped.error.as_deref(),
        Some("it ran past its 200 ms"),
        "{looped:?}"
    );
}

/// The text a hook's input is passed as, in a Luau long string, reads
/// back as that text in Luau itself, whatever brackets and `=` it holds.
/// The text is JSON, which never begins with a line break: the one
/// character a long string drops after its opening bracket.
#[hegel::test(test_cases = 100)]
fn a_long_string_reads_back_as_its_text(tc: TestCase) {
    let drawn: String = tc.draw(gs::text().alphabet("[]=x\n").max_size(24));
    let text = if drawn.starts_with('\n') {
        format!("x{drawn}")
    } else {
        drawn
    };
    let modules = Arc::new(files("return {}").modules().unwrap());
    let read = tau_testing::block_on_io(evaluate(
        "strings",
        modules,
        // A table, as hooks return: a bare string comes back as text.
        format!("return {{ text = {} }}", long_string(&text)),
        Duration::from_secs(2),
        CancellationToken::new(),
    ));
    assert_eq!(read, Ok(json!({ "text": text })), "{}", long_string(&text));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_that_cannot_load_says_why() {
    let named_wrong = files(&FRIDAY.replace("no-friday-deploys", "other"));
    let error = load("no-friday-deploys", named_wrong).await.unwrap_err();
    assert!(error.contains("folder is `no-friday-deploys`"), "{error}");

    let broken = load("x", files("return tau.plugin {")).await.unwrap_err();
    assert!(!broken.is_empty());

    let bad_tool = r#"
local tau = require("tau")
return tau.plugin { name = "x", tools = { ["no good"] = { call = function() end } } }
"#;
    let error = load("x", files(bad_tool)).await.unwrap_err();
    assert!(error.contains("not a tool name"), "{error}");

    let taken = Files {
        source: "return {}".into(),
        libs: BTreeMap::from([("tau".into(), "return {}".into())]),
        ..Files::default()
    };
    let error = load("x", taken).await.unwrap_err();
    assert!(error.contains("taken"), "{error}");
}

const FRIDAY_TESTS: &str = r#"
local tau = require("tau")
local ui = tau.ui
local t = tau.test

t.case("blocks deploys on Friday", function()
  local run = t.run { now = "2026-10-09T10:00:00Z" }
  t.equal(run:before_tool { name = "exec_command", args = { cmd = "make deploy" } },
    tau.block("No deploys on Friday."))
end)

t.case("lets them through on Monday", function()
  local run = t.run { now = "2026-10-05T10:00:00Z" }
  t.equal(run:before_tool { name = "exec_command", args = { cmd = "make deploy" } }, nil)
end)

t.case("the card says when to wait", function()
  local run = t.run { now = "2026-10-09T10:00:00Z" }
  t.equal(run:card("deploy_window", {}, { ok = false }), ui.badge("wait", "warn"))
end)

t.case("turns are counted, and the view shows them", function()
  local run = t.run {}
  run:turn_end { turn = 1 }
  run:turn_end { turn = 2 }
  t.equal(run.state.turns, 2)
  t.equal(run.logs, { "turn 1", "turn 2" })
  t.equal(run:view().status.detail, "2 turns")
  run:action("reset")
  t.equal(run.state.turns, 0)
end)

t.case("a tool calls the fake tools it is given", function()
  local run = t.run { tools = { exec_command = function(args) return "ran " .. args.cmd end } }
  t.equal(run:tool("run_it", { cmd = "ls" }), "ran ls")
end)

t.case("this one is wrong on purpose", function()
  local run = t.run { now = "2026-10-09T10:00:00Z" }
  t.equal(run:tool("deploy_window"), { ok = true }, "the window")
end)
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_plugins_tests_run_against_fake_runs() {
    let mut with_tests = files(FRIDAY);
    with_tests.tests = BTreeMap::from([
        ("friday".to_owned(), FRIDAY_TESTS.to_owned()),
        ("broken".to_owned(), "error('not even a case')".to_owned()),
    ]);
    let loaded = load("no-friday-deploys", with_tests).await.unwrap();
    let results = loaded.test(CancellationToken::new()).await;
    let summary: Vec<(&str, &str, bool)> = results
        .iter()
        .map(|r| (r.file.as_str(), r.name.as_str(), r.passed))
        .collect();
    assert_eq!(
        summary,
        [
            ("broken", "tests/broken.luau", false),
            ("friday", "blocks deploys on Friday", true),
            ("friday", "lets them through on Monday", true),
            ("friday", "the card says when to wait", true),
            ("friday", "turns are counted, and the view shows them", true),
            ("friday", "a tool calls the fake tools it is given", true),
            ("friday", "this one is wrong on purpose", false),
        ],
        "{results:#?}"
    );
    let wrong = results.last().unwrap().error.clone().unwrap();
    assert!(
        wrong.contains(r#"the window: expected {"ok":true}, got {"ok":false}"#),
        "{wrong}"
    );
    assert!(
        results[0]
            .error
            .as_deref()
            .unwrap()
            .contains("not even a case")
    );
}
