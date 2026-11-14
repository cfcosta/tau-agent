//! State machines in Luau plugins (ADR 0054): `tau.machine` keeps its
//! states in shape and steps by its table; `t.machine_of` drives one on
//! hegel's engine, every event from every state, and finds a stray
//! field, a missing edge and an impure transition, printing the case to
//! keep.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::collections::BTreeMap;

use tau_luau_plugins::TestResult;
use tau_luau_plugins_host::runtime::{Files, load};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

const PLUGIN: &str = r#"
local tau = require("tau")
return tau.plugin { }
"#;

/// The sign-in of tau-github, as ADR 0054 writes it: `failed` has its
/// `token_saved` arm when `FIXED` says so.
const SIGN_IN: &str = r#"
local tau = require("tau")
local t = tau.test
local g = t.gen

local function sign_in(fixed)
  local token_saved = {
    signed_out = function() return { state = "checking" }, { "check_token" } end,
  }
  if fixed then
    token_saved.failed = function() return { state = "checking" }, { "check_token" } end
  end
  return tau.machine({
    states = {
      signed_out = {},
      checking = {},
      failed = { "error" },
      signed_in = { "user" },
    },
    initial = { state = "signed_out" },
    effects = { "check_token", "forget_secret" },
    on = {
      token_saved = token_saved,
      checked = {
        checking = function(_, e)
          if e.ok then return { state = "signed_in", user = e.user } end
          return { state = "failed", error = e.error }
        end,
      },
      signed_out = {
        signed_in = function() return { state = "signed_out" }, { "forget_secret" } end,
      },
    },
  })
end
"#;

/// What the plugin's tests come to, with `tests` as its one test file.
fn results(tests: &str) -> Vec<TestResult> {
    let files = Files {
        source: PLUGIN.into(),
        tests: BTreeMap::from([("machines".to_owned(), tests.to_owned())]),
        ..Files::named("machines")
    };
    let loaded = block_on_io(load("machines", files)).unwrap();
    block_on_io(loaded.test(CancellationToken::new()))
}

fn only(tests: &str) -> TestResult {
    let mut results = results(tests);
    assert_eq!(results.len(), 1, "{results:?}");
    results.remove(0)
}

fn error(result: &TestResult) -> &str {
    result.error.as_deref().unwrap_or_default()
}

/// A machine steps by its table: an arm's state and effects, the state
/// as it was for an event with no arm, `initial` for no state; strict,
/// an event with no arm is an error; a state out of shape is an error
/// whichever way it comes.
#[test]
fn a_machine_steps_by_its_table_and_keeps_its_states_in_shape() {
    let result = only(&format!(
        r#"{SIGN_IN}
t.case("steps", function()
  local m = sign_in(true)
  local s, effects = m:step(nil, {{ event = "token_saved" }})
  t.equal(s, {{ state = "checking" }})
  t.equal(effects, {{ "check_token" }})
  s = m:step(s, {{ event = "checked", ok = false, error = "no" }})
  t.equal(s, {{ state = "failed", error = "no" }})
  local same, none = m:step(s, {{ event = "signed_out" }})
  t.equal(same, s, "no arm: as it was")
  t.equal(#none, 0)
  t.equal(m:tags(), {{ "checking", "failed", "signed_in", "signed_out" }})
  t.equal(m:handles(s, "token_saved"), true)

  local ok, why = pcall(m.step, m, {{ state = "failed" }}, {{ event = "token_saved" }})
  t.equal(ok, false)
  t.equal(string.find(why, "lacks `error`") ~= nil, true, why)
  ok, why = pcall(m.step, m, {{ state = "checking", error = "old" }}, {{ event = "token_saved" }})
  t.equal(string.find(why, "does not carry") ~= nil, true, why)

  local stray = tau.machine({{
    states = {{ a = {{}}, b = {{ "n" }} }},
    initial = {{ state = "a" }},
    on = {{ go = {{ a = function() return {{ state = "b", n = 1, left = true }} end }} }},
  }})
  ok, why = pcall(stray.step, stray, nil, {{ event = "go" }})
  t.equal(string.find(why, "gave a state that of `b` has `left`") ~= nil, true, why)

  local strict = tau.machine({{
    states = {{ a = {{}} }}, initial = {{ state = "a" }}, strict = true, on = {{ go = {{}} }},
  }})
  ok, why = pcall(strict.step, strict, nil, {{ event = "go" }})
  t.equal(string.find(why, "has no arm from `a`") ~= nil, true, why)

  ok, why = pcall(tau.machine, {{
    states = {{ a = {{}} }}, initial = {{ state = "a" }}, on = {{ go = {{ nowhere = function() end }} }},
  }})
  t.equal(string.find(why, "which is no state") ~= nil, true, why)
end)
"#
    ));
    assert!(result.passed, "{}", error(&result));
}

/// A machine can name its tag field, for data kept before it: its states
/// read and step by that field, which is no field of theirs.
#[test]
fn a_machine_names_its_tag_field() {
    let result = only(
        r#"
local tau = require("tau")
local t = tau.test
t.case("by status", function()
  local m = tau.machine({
    tag = "status",
    states = { active = { "n" }, paused = { "n" } },
    initial = { status = "active", n = 0 },
    on = { pause = { active = function(s) return { status = "paused", n = s.n } end } },
  })
  local s = m:step(nil, { event = "pause" })
  t.equal(s, { status = "paused", n = 0 })
  t.equal(m:is(s, "paused"), true)
  local ok, why = pcall(m.step, m, { state = "active", n = 0 }, { event = "pause" })
  t.equal(ok, false)
  t.equal(string.find(why, "names no state") ~= nil, true, why)
  ok, why = pcall(tau.machine, {
    tag = "status", states = { a = { "status" } }, initial = { status = "a" }, on = {},
  })
  t.equal(string.find(why, "names the tag") ~= nil, true, why)
end)
"#,
    );
    assert!(result.passed, "{}", error(&result));
}

/// A machine with an arm no path from `initial` takes: nothing leads to
/// `archived`, as records an older version kept may still hold.
const ARCHIVE: &str = r#"
local tau = require("tau")
local t = tau.test
local g = t.gen
local machine = tau.machine({
  states = { open = { "n" }, closed = { "n" }, archived = { "n", "why" } },
  initial = { state = "open", n = 0 },
  on = {
    close = { open = function(s) return { state = "closed", n = s.n + 1 } end },
    reopen = { closed = function(s) return { state = "open", n = s.n } end },
    restore = { archived = function(s) return { state = "open", n = s.n } end },
  },
})
"#;

/// An arm no case took fails the machine, naming it; one the plugin
/// lists in `untaken` may stay so, and one `spec.states` starts cases
/// from is taken.
#[test]
fn an_arm_no_case_took_is_named() {
    let untested = only(&format!(
        r#"{ARCHIVE}
t.machine_of("every arm", machine, {{}})
"#
    ));
    assert!(!untested.passed);
    assert!(
        error(&untested).contains("no case took `restore` from `archived`"),
        "{}",
        error(&untested)
    );

    let listed = only(&format!(
        r#"{ARCHIVE}
t.machine_of("every arm", machine, {{ untaken = {{ "restore from archived" }} }})
"#
    ));
    assert!(listed.passed, "{}", error(&listed));

    let drawn = only(&format!(
        r#"{ARCHIVE}
t.machine_of("every arm", machine, {{
  states = {{ archived = g.records({{ n = g.integers({{ min = 0, max = 9 }}), why = g.text({{ max = 3 }}) }}) }},
}})
"#
    ));
    assert!(drawn.passed, "{}", error(&drawn));
}

/// A case that starts from a drawn state, and fails, is noted from that
/// state: the case to paste begins there.
#[test]
fn a_case_from_a_drawn_state_is_noted_from_it() {
    let result = only(&format!(
        r#"{ARCHIVE}
t.machine_of("counts stay small", machine, {{
  states = {{ archived = g.records({{ n = g.integers({{ min = 5, max = 9 }}), why = g.just("old") }}) }},
  invariant = function(state) t.equal(state.n < 5, true, "a count stays under 5") end,
}})
"#
    ));
    assert!(!result.passed);
    let error = error(&result);
    assert!(error.contains("a count stays under 5"), "{error}");
    assert!(
        error.contains(
            r#"local state = { n = 5, state = "archived", why = "old" }"#
        ),
        "{error}"
    );
}

/// A start state drawn out of shape is the spec's error, said as such.
#[test]
fn a_start_state_out_of_shape_is_said() {
    let result = only(&format!(
        r#"{ARCHIVE}
t.machine_of("shapes", machine, {{
  states = {{ archived = g.records({{ n = g.just(1) }}) }},
}})
"#
    ));
    assert!(!result.passed);
    assert!(
        error(&result)
            .contains("spec.states.archived drew a state out of shape"),
        "{}",
        error(&result)
    );
}

/// The fixed sign-in holds under every event from every state: a token
/// saved is checked, whatever came before.
#[test]
fn a_machine_that_keeps_its_rule_passes() {
    let result = only(&format!(
        r#"{SIGN_IN}
local machine = sign_in(true)
t.machine_of("the sign-in checks every saved token", machine, {{
  events = {{
    checked = g.records({{ ok = g.booleans(), user = g.text({{ max = 4 }}), error = g.text({{ max = 4 }}) }}),
  }},
  init = function() return {{ saved = false }} end,
  event = function(model, e, state)
    if e.event == "token_saved" then model.saved = true end
    if state.state ~= "checking" then model.saved = false end
  end,
  invariant = function(state, model)
    t.equal(not model.saved or state.state == "checking", true, "a saved token waits to be checked")
  end,
}})
"#
    ));
    assert!(result.passed, "{}", error(&result));
}

/// tau-github's bug: `failed` has no `token_saved` arm, so a token saved
/// after a failure is never checked. The model finds it in three events
/// and notes the case to keep.
#[test]
fn a_missing_edge_is_found_and_its_case_noted() {
    let result = only(&format!(
        r#"{SIGN_IN}
local machine = sign_in(false)
t.machine_of("the sign-in checks every saved token", machine, {{
  events = {{
    checked = g.records({{ ok = g.booleans(), user = g.text({{ max = 4 }}), error = g.text({{ max = 4 }}) }}),
  }},
  init = function() return {{ saved = false }} end,
  event = function(model, e, state)
    if e.event == "token_saved" then model.saved = true end
    if state.state == "checking" or state.state == "signed_in" then model.saved = false end
  end,
  invariant = function(state, model)
    t.equal(not model.saved, true, "a saved token waits to be checked")
  end,
}})
"#
    ));
    assert!(!result.passed);
    let error = error(&result);
    assert!(
        error.contains("a saved token waits to be checked"),
        "{error}"
    );
    let steps: Vec<&str> = error
        .lines()
        .filter(|line| line.trim_start().starts_with("step "))
        .collect();
    assert_eq!(
        steps,
        [
            "  step 1: token_saved",
            "  step 2: checked",
            "  step 3: token_saved"
        ],
        "{error}"
    );
    assert!(
        error
            .contains("t.case(\"the sign-in checks every saved token: found\""),
        "{error}"
    );
    assert!(error.contains("machine:step(state, event)"), "{error}");
    assert!(error.contains("event = \"token_saved\""), "{error}");
}

/// A transition that reads more than its state and event gives another
/// answer when taken again, and is found without a line of its own.
#[test]
fn an_impure_transition_is_found() {
    let result = only(
        r#"
local tau = require("tau")
local t = tau.test
local calls = 0
local machine = tau.machine({
  states = { a = { "n" } },
  initial = { state = "a", n = 0 },
  on = { tick = { a = function() calls += 1 return { state = "a", n = calls } end } },
})
t.machine_of("ticks", machine, {})
"#,
    );
    assert!(!result.passed);
    let error = error(&result);
    assert!(
        error.contains("gave another answer when taken again"),
        "{error}"
    );
}

/// What the tests of a plugin of `source` come to.
fn results_of(source: &str, tests: &str) -> Vec<TestResult> {
    let files = Files {
        source: source.into(),
        tests: BTreeMap::from([("hooks".to_owned(), tests.to_owned())]),
        ..Files::named("hooks")
    };
    let loaded = block_on_io(load("hooks", files)).unwrap();
    block_on_io(loaded.test(CancellationToken::new()))
}

fn only_of(source: &str, tests: &str) -> TestResult {
    let mut results = results_of(source, tests);
    assert_eq!(results.len(), 1, "{results:?}");
    results.remove(0)
}

/// A check that waits on Jev, as tau-goal's does, and a Pause.
const CHECKING: &str = r#"
local tau = require("tau")
local cmd = tau.cmd
return tau.plugin({
  uses = { jev = true },
  actions = {
    pause = function(_, model) model.paused = true end,
  },
  before_stop = function(_, model)
    model.checks = (model.checks or 0) + 1
    tau.await(cmd.jev.ask({ questions = {} }))
    return nil
  end,
})
"#;

/// A Pause pressed while a check waits on Jev is taken at once, on the
/// one model, as on the host (ADR 0056): every order `t.hooks` tries
/// keeps it.
#[test]
fn an_action_while_a_hook_awaits_is_kept() {
    let result = only_of(
        CHECKING,
        r#"
local t = require("tau").test
t.hooks("a pause is kept", {
  run = { jev = { ask = function() return {}, 0 end } },
  hooks = {
    stop = function(run) run:before_stop({ text = "done" }) end,
    pause = function(run, _, world) run:action("pause") world.paused = true end,
  },
  -- A check whose Jev fails raises.
  may_fail = function(hook) return hook == "stop" end,
  invariant = function(world)
    if world.paused then
      t.equal(world.run.state.paused, true, "the pause is kept")
    end
  end,
})
"#,
    );
    assert!(result.passed, "{:?}", result.error);
}

/// A count read from the store, then a wait, then the count written: two
/// checks, `n` read by both before either writes.
fn counting(read_again: bool) -> String {
    let after_wait = if read_again {
        "n = tau.await(cmd.store.get(\"n\")) or 0"
    } else {
        ""
    };
    format!(
        r#"
local tau = require("tau")
local cmd = tau.cmd
return tau.plugin({{
  uses = {{ jev = true, store = true }},
  before_stop = function()
    local n = tau.await(cmd.store.get("n")) or 0
    tau.await(cmd.jev.ask({{ questions = {{}} }}))
    {after_wait}
    tau.give(cmd.store.put("n", n + 1))
    return nil
  end,
}})
"#
    )
}

const EVERY_CHECK_COUNTS: &str = r#"
local t = require("tau").test
t.hooks("every check counts", {
  run = { jev = { ask = function() return {}, 0 end } },
  hooks = {
    stop = function(run, _, world)
      run:before_stop({ text = "done" })
      world.done = (world.done or 0) + 1
    end,
  },
  may_fail = function(hook) return hook == "stop" end,
  invariant = function(world)
    t.equal(world.run.stores.test.n or 0, world.done or 0, "every check that ended counts")
  end,
})
"#;

/// A hook that keeps what it read from the store across an await, and
/// writes from it, loses a count when another check runs meanwhile: the
/// store is taken a step at a time (ADR 0056), and `t.hooks` finds the
/// order. Read again after the await, every check counts.
#[test]
fn a_read_kept_across_an_await_is_found() {
    let stale = only_of(&counting(false), EVERY_CHECK_COUNTS);
    assert!(!stale.passed);
    assert!(
        error(&stale).contains("every check that ended counts"),
        "{}",
        error(&stale)
    );
    let fresh = only_of(&counting(true), EVERY_CHECK_COUNTS);
    assert!(fresh.passed, "{}", error(&fresh));
}

/// A wait that fails raises in the hook that waited: that fails the test
/// unless the plugin may fail there.
#[test]
fn a_failed_wait_raises_unless_the_hook_may_fail() {
    let tests = |may_fail: &str| {
        format!(
            r#"
local t = require("tau").test
t.hooks("checks", {{
  run = {{ jev = {{ ask = function() return {{}}, 0 end }} }},
  hooks = {{ stop = function(run) run:before_stop({{ text = "done" }}) end }},
  {may_fail}
}})
"#
        )
    };
    let strict = only_of(CHECKING, &tests(""));
    assert!(!strict.passed);
    assert!(error(&strict).contains("stop raised"), "{}", error(&strict));
    let lenient = only_of(
        CHECKING,
        &tests("may_fail = function(hook, why) return hook == \"stop\" end,"),
    );
    assert!(lenient.passed, "{}", error(&lenient));
}
