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

fn files(name: &str, plugin: &str) -> Files {
    Files {
        source: plugin.into(),
        libs: BTreeMap::from([("days".into(), DAYS.into())]),
        ..Files::named(name)
    }
}

fn context(weekday: &str) -> Context {
    Context {
        run: json!({ "id": "r1", "kind": "main", "turn": 1 }),
        now: json!({ "unix": 0, "iso": "2026-10-09T10:00:00Z", "weekday": weekday }),
        settings: json!({ "days": ["Friday"] }),
        state: json!({}),
        secrets: None,
        store: None,
    }
}

fn no_reach() -> Arc<dyn Reach> {
    Arc::new(NoReach)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_declares_what_it_is() {
    let mut described = files("no-friday-deploys", FRIDAY);
    described.manifest.description =
        "Blocks deploy commands on Fridays.".into();
    let loaded = load("no-friday-deploys", described).await.unwrap();
    let declared = &loaded.declaration;
    assert_eq!(declared.name, "no-friday-deploys");
    assert_eq!(declared.description, "Blocks deploy commands on Fridays.");
    assert_eq!(
        declared.uses,
        Uses {
            tools: vec!["exec_command".into()],
            jev: false,
            infer: false,
            change_args: false,
            network: Vec::new(),
            secrets: Vec::new(),
            model: None,
            store: false,
            transcript: false,
            archive: false,
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
            start: false,
            before_tool: true,
            before_request: false,
            after_tool_result: false,
            rewrite_context: false,
            rewritten: false,
            before_stop: true,
            turn_end: true,
            on_event: false,
            title: false,
            notify: false,
            repo_data: false,
            recover: false,
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
    let loaded = load("no-friday-deploys", files("no-friday-deploys", FRIDAY))
        .await
        .unwrap();
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
    let loaded = load("no-friday-deploys", files("no-friday-deploys", FRIDAY))
        .await
        .unwrap();
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
    let loaded = load("no-friday-deploys", files("no-friday-deploys", FRIDAY))
        .await
        .unwrap();
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
return tau.plugin { tools = {
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
    let loaded = load("broken", files("broken", plugin)).await.unwrap();
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
    let modules = Arc::new(files("x", "return {}").modules().unwrap());
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
    let named_wrong = files("other", FRIDAY);
    let error = load("no-friday-deploys", named_wrong).await.unwrap_err();
    assert!(error.contains("folder is `no-friday-deploys`"), "{error}");

    let broken = load("x", files("x", "return tau.plugin {"))
        .await
        .unwrap_err();
    assert!(!broken.is_empty());

    let bad_tool = r#"
local tau = require("tau")
return tau.plugin { tools = { ["no good"] = { call = function() end } } }
"#;
    let error = load("x", files("x", bad_tool)).await.unwrap_err();
    assert!(error.contains("not a tool name"), "{error}");

    let taken = Files {
        source: "return {}".into(),
        libs: BTreeMap::from([("tau".into(), "return {}".into())]),
        ..Files::named("x")
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
    let mut with_tests = files("no-friday-deploys", FRIDAY);
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

const EVERY_PIECE: &str = r##"
local tau = require("tau")
local ui = tau.ui

return tau.plugin {
  view = function(state, ctx)
    return { page = ui.stack {
      "plain",
      ui.text("text"), ui.rich("**rich**"), ui.mono("mono"),
      ui.heading("Heading"), ui.markdown("# md\n\n- a"),
      ui.icon("memory", "info"), ui.badge("b", "warn"), ui.pill("p", "good"),
      ui.status("on", "detail", "good"), ui.status("on"),
      ui.rows { a = 1, b = 2 }, ui.rows { { "k", "v" } },
      ui.table({ "name", "n" }, { { "a", ui.badge("1") }, { "b", "2" } }),
      ui.list { "one", ui.mono("two") },
      ui.progress(0.4, "40%"), ui.progress(2),
      ui.code("lua", "x = 1"), ui.code(nil, "plain", { first_line = 10 }), ui.toned("PASS", "good", true),
      ui.row { ui.button("Go", "go"), ui.button("Drop", "drop", { id = 1 }, "danger") },
      ui.scroll({ "a", "b" }, 10), ui.scroll {},
      ui.rule(), ui.empty("Nothing yet."),
      ui.link("Notes", "notes"),
      ui.link("Theirs", "page", { plugin = "tau-memory", params = { repo = "r" } }),
      ui.section("more", "More", { "inside" }, { open = true }), ui.section("less", "Less", {}),
      ui.tabs("views", { { "One", { "first" } }, { "Two", { ui.mono("second") } } }),
      ui.input("note", { placeholder = "A note", multiline = true, submit = "save" }), ui.input("q"),
      ui.diff("@@ -1 +1 @@\n-a\n+b\n", "src/lib.rs"), ui.diff("-x\n"),
      ui.terminal("out", "ok\r\n", { cols = 40, rows = 4 }), ui.terminal("live", "", { live = true }),
      ui.ticker("load", 1000), ui.ticker("done", 1000, 61000),
      ui.input("limit", { keep = true }), ui.modal("Sure?", { "It cannot be undone." }, { dismiss = "cancel" }),
      ui.graph({ { id = "a", label = "A", focus = true }, { id = "b", x = 0.2, y = 0.2, tone = "info", page = "note" } }, { { "a", "b" } }),
    } }
  end,
}
"##;

/// Every piece `tau.ui` builds reads as the interface's view tree, with
/// nothing invalid: the builders and the tree agree on the wire shape.
#[tokio::test(flavor = "multi_thread")]
async fn every_ui_piece_reads_as_a_tree() {
    let loaded = load("every-piece", files("every-piece", EVERY_PIECE))
        .await
        .unwrap();
    let drawn = loaded
        .call(
            &Hook::View,
            json!({}),
            &context("Monday"),
            no_reach(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(drawn.error, None);
    let page = &drawn.value["page"];
    let node = tau_ui_plugin::tree::Node::read(page);
    let tau_ui_plugin::tree::Node::Stack { children } = &node else {
        panic!("a stack: {node:?}")
    };
    assert_eq!(children.len(), 41);
    let written = node.value().to_string();
    assert!(!written.contains(r#""piece":"invalid""#), "{written}");
}

const ECHO: &str = r#"
local tau = require("tau")
return tau.plugin { tools = {
    echo = {
      description = "Gives its arguments back.",
      call = function(args, ctx) return args end,
    },
  },
}
"#;

/// What a hook is given reaches it whole, whatever it holds: brackets
/// that would close a long string, and text up to near the megabyte
/// `json.decode` reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_hook_is_given_its_input_whole() {
    let echo = Files {
        source: ECHO.into(),
        ..Files::named("echo")
    };
    let loaded = load("echo", echo).await.unwrap();
    let long = "x]]".repeat(100_000);
    for text in ["]]", "]=]", "a]==]b[[c", long.as_str()] {
        let input = json!({ "text": text, "nested": [text, { "k": text }] });
        let out = loaded
            .call(
                &Hook::Tool("echo".into()),
                input.clone(),
                &context("Monday"),
                no_reach(),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(out.error, None);
        assert_eq!(out.value, input);
    }
}

const PINGS: &str = r#"
local tau = require("tau")
return tau.plugin { uses = { network = { "127.0.0.1" }, secrets = { "token" } },
  tools = {
    ping = {
      description = "Pings the server.",
      call = function(args, ctx)
        local answer = http.request({
          url = args.url,
          headers = { authorization = "Bearer " .. http.secret(args.secret) },
        })
        return answer.body
      end,
    },
  },
}
"#;

/// A plugin's HTTP requests reach the hosts it declares, with the
/// secrets it declares put in as they leave, from its own store; any
/// other host or secret is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_reaches_the_hosts_and_secrets_it_declares() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let heard = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let server = heard.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buffer = vec![0u8; 4096];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            *server.lock().unwrap() =
                String::from_utf8_lossy(&buffer[..read]).into_owned();
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\npong")
                .await;
        }
    });
    let config = tempfile::tempdir().unwrap();
    let secrets =
        tau_luau_plugins_host::runtime::secrets_of(config.path(), "pings");
    tau_tools_host::secrets::SecretStore::new(&secrets)
        .set("token", "s3cr3t-value")
        .await
        .unwrap();
    let pings = Files {
        source: PINGS.into(),
        ..Files::named("pings")
    };
    let loaded = load("pings", pings).await.unwrap();
    let context = Context {
        secrets: Some(secrets),
        ..context("Monday")
    };
    let ping = |url: String, secret: &str| {
        let (loaded, context) = (loaded.clone(), context.clone());
        let args = json!({ "url": url, "secret": secret });
        async move {
            loaded
                .call(
                    &Hook::Tool("ping".into()),
                    args,
                    &context,
                    no_reach(),
                    CancellationToken::new(),
                )
                .await
        }
    };
    let answered = ping(format!("http://127.0.0.1:{port}/"), "token").await;
    assert_eq!(answered.error, None);
    assert_eq!(answered.value, json!("pong"));
    assert!(
        heard.lock().unwrap().contains("Bearer s3cr3t-value"),
        "{}",
        heard.lock().unwrap()
    );

    let elsewhere = ping("http://localhost:1/".into(), "token").await;
    assert!(
        elsewhere
            .error
            .as_deref()
            .is_some_and(|e| e.contains("`localhost`")),
        "{elsewhere:?}"
    );
    let other = ping(format!("http://127.0.0.1:{port}/"), "github").await;
    assert!(
        other
            .error
            .as_deref()
            .is_some_and(|e| e.contains("`github`")),
        "{other:?}"
    );
}

const SHIPS: &str = r#"
local tau = require("tau")
return tau.plugin { commands = {
    ship = {
      description = "Marks the run shipped",
      args = "<version>",
      icon = "rocket",
      popover = { jev = tau.ui.input("channel", { submit = "tau.submit" }) },
      run = function(text, ctx)
        ctx.state.shipped = text
        ctx.state.channel = ctx.drafts.channel
      end,
    },
  },
}
"#;

/// A plugin's command is declared with what it does and takes, its icon
/// and its popover, the same without a Jev key when it gives no other;
/// and it runs as its action `/name`, given the text after it and what
/// the popover's inputs say.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_runs_as_its_action() {
    let ships = Files {
        source: SHIPS.into(),
        ..Files::named("ships")
    };
    let loaded = load("ships", ships).await.unwrap();
    let [command] = loaded.declaration.commands.as_slice() else {
        panic!("{:?}", loaded.declaration.commands);
    };
    assert_eq!(
        (
            command.name.as_str(),
            command.description.as_str(),
            command.args.as_str(),
            command.icon.as_str(),
        ),
        ("ship", "Marks the run shipped", "<version>", "rocket")
    );
    let popover = command.popover.clone().expect("its popover");
    assert_eq!(popover.jev["piece"], "input");
    assert_eq!(popover.no_jev, popover.jev);
    let out = loaded
        .call(
            &Hook::Action("/ship".into()),
            json!({ "text": "1.2", "drafts": { "channel": "beta" } }),
            &context("Monday"),
            no_reach(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(out.error, None);
    assert_eq!(out.state, json!({ "shipped": "1.2", "channel": "beta" }));
}

/// A run hook of a plugin whose `uses` names the model or Jev has a
/// tool's time to call them (ADR 0050); one that names neither keeps its
/// short limit, and `on_event`, which runs for every event, always does.
#[test]
fn a_hook_that_may_call_the_model_has_the_time_for_it() {
    use tau_luau_plugins::Declaration;
    let plain = Declaration::default();
    let mut reaching = Declaration::default();
    reaching.uses.jev = true;
    let tool = Hook::Tool("t".into()).limit();
    for hook in [
        Hook::Start,
        Hook::BeforeTool,
        Hook::BeforeRequest,
        Hook::AfterToolResult,
        Hook::Rewritten,
        Hook::BeforeStop,
        Hook::TurnEnd,
        Hook::RunEnd,
    ] {
        assert_eq!(hook.limit_for(&reaching), tool, "{hook:?}");
        assert_eq!(hook.limit_for(&plain), hook.limit(), "{hook:?}");
    }
    // A compaction may ask for many summaries, or a long one.
    assert_eq!(
        Hook::RewriteContext.limit_for(&reaching),
        tau_luau_plugins_host::runtime::REWRITE_LIMIT
    );
    assert_eq!(
        Hook::RewriteContext.limit_for(&plain),
        Hook::RewriteContext.limit()
    );
    assert_eq!(Hook::BeforeRequest.limit(), Duration::from_millis(200));
    assert_eq!(
        Hook::OnEvent.limit_for(&reaching),
        Duration::from_millis(200)
    );
}

/// An action of a plugin that uses Jev asks the host's, through what an
/// action reaches; one that does not use it has none.
#[tokio::test(flavor = "multi_thread")]
async fn an_action_asks_jev_when_its_plugin_uses_it() {
    const TRIES: &str = r#"
local tau = require("tau")
return tau.plugin { uses = { jev = USES },
  actions = {
    try = function(args, ctx)
      if ctx.jev == nil then return "no jev" end
      local answer = ctx.jev.noul({ state = { x = 1 }, question = "Is it?" })
      return answer.probability
    end,
  },
}
"#;
    let jev: std::sync::Arc<dyn tau_jev::Jev> =
        std::sync::Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.25));
    for (uses, want) in [("true", json!(0.25)), ("false", json!("no jev"))] {
        let files = Files {
            source: TRIES.replace("USES", uses),
            ..Files::named("tries")
        };
        let loaded = load("tries", files).await.unwrap();
        let out = loaded
            .call(
                &Hook::Action("try".into()),
                json!({}),
                &context("Monday"),
                std::sync::Arc::new(
                    tau_luau_plugins_host::runtime::ActionReach(Some(
                        jev.clone(),
                    )),
                ),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(out.error, None);
        assert_eq!(out.value, want, "uses.jev = {uses}");
    }
}
