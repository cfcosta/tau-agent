//! A plugin's actor (ADR 0056): one model, one message a step. A hook
//! awaits a command and goes on in a later step with its answer; what
//! another message changes meanwhile is the model the hook sees; a step
//! that fails keeps nothing of what it changed; a view draws from a
//! frozen copy and waits on nothing.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::collections::BTreeMap;

use tau_luau_plugins::TestResult;
use tau_luau_plugins_host::runtime::{Files, load};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

/// A goal-like plugin: a check that awaits Jev, a pause, and a view.
const PLUGIN: &str = r#"
local tau = require("tau")
local cmd = tau.cmd
return tau.plugin({
  uses = { jev = true, store = true },
  before_stop = function(stop, model, env)
    model.checks = (model.checks or 0) + 1
    tau.log("checking at " .. tostring(env.at))
    local answers, cost = tau.await(cmd.jev.ask({ questions = { met = "?" } }))
    if model.paused then
      return "paused"
    end
    model.met = answers.met
    tau.give(cmd.store.put("cost", cost))
    return "checked"
  end,
  actions = {
    pause = function(_, model) model.paused = true end,
    broken = function(_, model)
      model.paused = "half"
      error("it broke")
    end,
    own = function()
      coroutine.yield(1)
    end,
    many = function()
      return tau.all({ cmd.store.get("a"), cmd.store.get("b") })
    end,
  },
  view = function(model)
    return { status = { text = tostring(model.checks) } }
  end,
})
"#;

fn only(tests: &str) -> TestResult {
    let files = Files {
        source: PLUGIN.into(),
        tests: BTreeMap::from([("actors".to_owned(), tests.to_owned())]),
        ..Files::named("actors")
    };
    let loaded = block_on_io(load("actors", files)).unwrap();
    let mut results = block_on_io(loaded.test(CancellationToken::new()));
    assert_eq!(results.len(), 1, "{results:?}");
    results.remove(0)
}

fn passes(tests: &str) {
    let result = only(tests);
    assert!(result.passed, "{:?}", result.error);
}

const HEAD: &str = r#"
local tau = require("tau")
local t = tau.test
local plugin = require("plugin")
"#;

/// A hook awaits its command, and ends with the answer in a later step,
/// its log, its given command and its model changed.
#[test]
fn a_hook_awaits_a_command_and_ends_with_its_answer() {
    passes(&format!(
        r#"{HEAD}
t.case("awaits", function()
  local actor = tau._actor(plugin, {{}})
  local step = actor.deliver({{ kind = "call", id = "h1", path = {{ "before_stop" }}, input = {{}}, env = {{ at = 7 }} }})
  t.equal(step.waits.h1.op, "jev")
  t.equal(step.changed, true)
  t.equal(step.logs, {{ "checking at 7" }})
  t.equal(step.ended.h1, nil)
  step = actor.deliver({{ kind = "answer", id = "h1", ok = true, values = {{ {{ met = 0.9 }}, 0.25 }} }})
  t.equal(step.ended.h1, {{ value = "checked" }})
  t.equal(step.given, {{ {{ op = "store_put", args = {{ key = "cost", value = 0.25 }} }} }})
  t.equal(actor.model, {{ checks = 1, met = 0.9 }})
  t.equal(actor.waiting(), {{}})
end)
"#
    ));
}

/// An action that comes while a hook awaits changes the one model, and
/// the hook sees it once it goes on: nothing is written over.
#[test]
fn an_action_meanwhile_is_the_model_the_hook_goes_on_with() {
    passes(&format!(
        r#"{HEAD}
t.case("pause meanwhile", function()
  local actor = tau._actor(plugin, {{}})
  actor.deliver({{ kind = "call", id = "h1", path = {{ "before_stop" }}, input = {{}} }})
  local paused = actor.deliver({{ kind = "call", id = "a1", path = {{ "actions", "pause" }}, input = {{}} }})
  t.equal(paused.ended.a1, {{ value = json.null }})
  local step = actor.deliver({{ kind = "answer", id = "h1", ok = true, values = {{ {{ met = 0.9 }}, 0 }} }})
  t.equal(step.ended.h1, {{ value = "paused" }})
  t.equal(actor.model, {{ checks = 1, paused = true }})
end)
"#
    ));
}

/// A step that fails keeps nothing of what it changed, in the same table
/// the hooks under way hold, and gives none of its commands.
#[test]
fn a_failed_step_keeps_nothing() {
    passes(&format!(
        r#"{HEAD}
t.case("broken", function()
  local actor = tau._actor(plugin, {{}})
  local held = actor.model
  actor.deliver({{ kind = "call", id = "h1", path = {{ "before_stop" }}, input = {{}} }})
  local step = actor.deliver({{ kind = "call", id = "a1", path = {{ "actions", "broken" }}, input = {{}} }})
  t.equal(string.find(step.ended.a1.error, "it broke") ~= nil, true, step.ended.a1.error)
  t.equal(step.changed, false)
  t.equal(actor.model == held, true, "the same table")
  t.equal(held.paused, nil)
  step = actor.deliver({{ kind = "answer", id = "h1", ok = true, values = {{ {{ met = 1 }}, 0 }} }})
  t.equal(step.ended.h1, {{ value = "checked" }})
end)
"#
    ));
}

/// A failed answer raises where the hook awaits, and a cancel too; a
/// hook that yields on its own is an error, not an await.
#[test]
fn failures_reach_the_hook_where_it_awaits() {
    passes(&format!(
        r#"{HEAD}
t.case("failures", function()
  local actor = tau._actor(plugin, {{}})
  actor.deliver({{ kind = "call", id = "h1", path = {{ "before_stop" }}, input = {{}} }})
  local step = actor.deliver({{ kind = "answer", id = "h1", ok = false, error = "no key" }})
  t.equal(string.find(step.ended.h1.error, "no key") ~= nil, true, step.ended.h1.error)
  t.equal(actor.model.checks, 1, "what the earlier step changed stays")
  actor.deliver({{ kind = "call", id = "h2", path = {{ "before_stop" }}, input = {{}} }})
  step = actor.deliver({{ kind = "cancel", id = "h2", why = "the run ended" }})
  t.equal(string.find(step.ended.h2.error, "the run ended") ~= nil, true)
  step = actor.deliver({{ kind = "call", id = "o", path = {{ "actions", "own" }}, input = {{}} }})
  t.equal(step.ended.o, {{ error = "a hook yields only through tau.await" }})
  step = actor.deliver({{ kind = "call", id = "m", path = {{ "actions", "many" }}, input = {{}} }})
  t.equal(step.waits.m.op, "all")
  t.equal(#step.waits.m.args, 2)
  step = actor.deliver({{ kind = "answer", id = "m", ok = true, values = {{ {{ ok = true, values = {{ 1 }} }}, {{ ok = true, values = {{ 2 }} }} }} }})
  t.equal(step.ended.m.value[2].values, {{ 2 }})
  step = actor.deliver({{ kind = "call", id = "x", path = {{ "nothing" }}, input = {{}} }})
  t.equal(step.ended.x, {{ value = json.null }})
end)
"#
    ));
}

/// A view draws from a frozen copy: it can neither change the model nor
/// await.
#[test]
fn a_view_draws_from_a_frozen_copy() {
    passes(&format!(
        r#"{HEAD}
t.case("view", function()
  local actor = tau._actor(plugin, {{ checks = 3 }})
  t.equal(actor.draw({{ "view" }}), {{ status = {{ text = "3" }} }})
  local sneaky = tau._actor({{ view = function(model) model.checks = 9 end }}, {{ checks = 3 }})
  t.equal(pcall(sneaky.draw, {{ "view" }}), false)
  t.equal(sneaky.model.checks, 3)
  local waits = tau._actor({{ view = function() return tau.await(tau.cmd.transcript()) end }}, {{}})
  local ok, why = pcall(waits.draw, {{ "view" }})
  t.equal(ok, false)
  t.equal(string.find(why, "not a view") ~= nil, true, why)
end)
"#
    ));
}

mod host {
    //! The host's actor: the same steps, in a VM of its own that reaches
    //! nothing, has no clock, and fails a step past its limit.

    use std::time::Duration;

    use serde_json::{Value, json};
    use tau_luau_plugins_host::{
        actor::{Actor, Ended, Message},
        runtime::Files,
    };

    fn actor(source: &str) -> Actor {
        let files = Files {
            source: source.into(),
            ..Files::named("actors")
        };
        Actor::new(&files.modules().unwrap(), "plugin", &json!({})).unwrap()
    }

    fn call(id: &str, path: &[&str]) -> Message {
        Message::Call {
            id: id.into(),
            path: path.iter().map(|key| (*key).to_owned()).collect(),
            input: json!({}),
            env: json!({ "at": 1 }),
        }
    }

    const LIMIT: Duration = Duration::from_secs(2);

    /// A hook awaits Jev in one step and ends in the next with its
    /// answer; a Pause between them is what it goes on with.
    #[test]
    fn a_hook_awaits_and_goes_on_with_the_model_as_it_is() {
        let actor = actor(super::PLUGIN);
        let step = actor.deliver(&call("h1", &["before_stop"]), LIMIT).unwrap();
        assert_eq!(step.waits["h1"].op, "jev");
        assert_eq!(step.logs, ["checking at 1"]);
        assert!(step.changed);
        actor
            .deliver(&call("a1", &["actions", "pause"]), LIMIT)
            .unwrap();
        let step = actor
            .deliver(
                &Message::Answer {
                    id: "h1".into(),
                    ok: true,
                    values: vec![json!({ "met": 0.9 }), json!(0)],
                    error: None,
                },
                LIMIT,
            )
            .unwrap();
        assert_eq!(
            step.ended["h1"],
            Ended::Value {
                value: json!("paused")
            }
        );
        assert_eq!(
            actor.model().unwrap(),
            json!({ "checks": 1, "paused": true })
        );
        assert!(actor.waiting().unwrap().is_empty());
        assert_eq!(
            actor
                .draw(&["view".to_owned()], None, &Value::Null, LIMIT)
                .unwrap(),
            json!({ "status": { "text": "1" } })
        );
    }

    /// A step has no clock nor randomness, and one that runs past its
    /// limit fails, keeping nothing.
    #[test]
    fn a_step_is_deterministic_and_bounded() {
        let actor = actor(
            r#"
local tau = require("tau")
return tau.plugin({
  actions = {
    clock = function() return os.time() end,
    dice = function() return math.random() end,
    spin = function(_, model) model.spun = true while true do end end,
  },
})
"#,
        );
        for name in ["clock", "dice"] {
            let step = actor
                .deliver(&call(name, &["actions", name]), LIMIT)
                .unwrap();
            assert!(
                matches!(&step.ended[name], Ended::Error { .. }),
                "{name}: {:?}",
                step.ended
            );
        }
        let step = actor
            .deliver(
                &call("spin", &["actions", "spin"]),
                Duration::from_millis(50),
            )
            .unwrap();
        let Ended::Error { error } = &step.ended["spin"] else {
            panic!("{:?}", step.ended);
        };
        assert!(error.contains("it ran past its 50 ms"), "{error}");
        assert!(!step.changed);
        assert_eq!(actor.model().unwrap(), json!({}));
    }
}
