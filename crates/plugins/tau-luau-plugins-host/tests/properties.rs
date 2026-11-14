//! Property tests in Luau (ADR 0049): a plugin's `tests/*.luau` draw
//! from hegel's engine, which shrinks a failure to its least case and
//! gives what reproduces it; a state machine's failure shrinks to its
//! fewest steps; the same files give the same results.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::collections::BTreeMap;

use tau_luau_plugins::{TestKind, TestResult};
use tau_luau_plugins_host::runtime::{Files, load};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

const PLUGIN: &str = r#"
local tau = require("tau")
return tau.plugin { }
"#;

/// What the plugin's tests come to, with `tests` as its one test file.
fn results(tests: &str) -> Vec<TestResult> {
    let files = Files {
        source: PLUGIN.into(),
        tests: BTreeMap::from([("props".to_owned(), tests.to_owned())]),
        ..Files::named("props")
    };
    let loaded = block_on_io(load("props", files)).unwrap();
    block_on_io(loaded.test(CancellationToken::new()))
}

/// The one result `tests` gives.
fn only(tests: &str) -> TestResult {
    let mut results = results(tests);
    assert_eq!(results.len(), 1, "{results:?}");
    results.remove(0)
}

/// `result` but for how long it took, which no two runs share.
fn timeless(result: TestResult) -> TestResult {
    TestResult {
        millis: None,
        ..result
    }
}

fn error(result: &TestResult) -> &str {
    result.error.as_deref().unwrap_or_default()
}

#[test]
fn a_property_that_holds_passes() {
    let result = only(
        r#"
local t = require("tau").test
local g = t.gen
t.property("adding is commutative", function(draw)
  local a = draw(g.integers { min = -1000, max = 1000 })
  local b = draw(g.integers { min = -1000, max = 1000 })
  t.equal(a + b, b + a)
end)
"#,
    );
    assert!(result.passed, "{result:?}");
    assert_eq!(result.name, "adding is commutative");
    assert_eq!(result.kind, TestKind::Property);
    assert_eq!((result.cases, result.shrinks), (Some(100), None));
    assert!(result.millis.is_some(), "{result:?}");
}

/// An integer at least 5 shrinks to 5, and the report says what was
/// drawn, why it failed, and the blob that replays it.
#[test]
fn a_failure_shrinks_to_its_least_case() {
    let result = only(
        r#"
local t = require("tau").test
local g = t.gen
t.property("small", function(draw)
  local n = draw(g.integers { min = 0, max = 100 })
  t.equal(n < 5, true, "n is small")
end)
"#,
    );
    assert!(!result.passed);
    // It failed at some case, and every case after was hegel shrinking.
    assert!(result.cases.is_some_and(|at| at >= 1), "{result:?}");
    assert!(result.shrinks.is_some(), "{result:?}");
    let error = error(&result);
    assert!(error.contains("n is small"), "{error}");
    assert!(error.contains("draw 1 = 5"), "{error}");
    assert!(error.contains("reproduce: { reproduce = \""), "{error}");
}

/// A list whose sum passes 10 shrinks to one element, 11; a record to
/// the least of each field.
#[test]
fn lists_and_records_shrink_as_wholes() {
    let results = results(
        r#"
local t = require("tau").test
local g = t.gen
t.property("sums stay small", function(draw)
  local list = draw(g.lists(g.integers { min = 0, max = 100 }, { max = 10 }))
  local sum = 0
  for _, n in ipairs(list) do sum += n end
  t.equal(sum <= 10, true, "the sum")
end)
t.property("a record", function(draw)
  local r = draw(g.records { a = g.integers { min = 0, max = 50 }, b = g.integers { min = 0, max = 50 } })
  t.equal(r.a >= 3 and r.b >= 4, false, "both large")
end)
"#,
    );
    let by: BTreeMap<&str, &TestResult> =
        results.iter().map(|r| (r.name.as_str(), r)).collect();
    let sums = error(by["sums stay small"]);
    assert!(sums.contains("draw 1 = [11]"), "{sums}");
    let record = error(by["a record"]);
    assert!(record.contains(r#"draw 1 = {"a":3,"b":4}"#), "{record}");
}

/// Text, a regex, sampled values, `one_of`, `map`, `filter`, `optional`
/// and `dicts` give what they say.
#[test]
fn every_generator_gives_what_it_says() {
    let result = only(
        r#"
local t = require("tau").test
local g = t.gen
t.property("shapes", function(draw)
  local id = draw(g.from_regex("td-[0-9a-f]{4}"))
  t.equal(string.match(id, "^td%-%x%x%x%x$") ~= nil, true, id)
  local word = draw(g.text { min = 1, max = 5, codec = "ascii" })
  t.equal(#word >= 1 and #word <= 5, true, word)
  local status = draw(g.sampled_from { "open", "closed" })
  t.equal(status == "open" or status == "closed", true, status)
  local even = draw(g.integers { min = 0, max = 100 }:filter(function(n) return n % 2 == 0 end))
  t.equal(even % 2, 0)
  local priority = draw(g.integers { min = 0, max = 4 }:map(function(n) return "P" .. n end))
  t.equal(string.match(priority, "^P[0-4]$") ~= nil, true, priority)
  local either = draw(g.one_of { g.just("a"), g.booleans() })
  t.equal(either == "a" or type(either) == "boolean", true)
  local maybe = draw(g.optional(g.integers { min = 1, max = 1 }))
  t.equal(maybe == nil or maybe == 1, true)
  local x = draw(g.floats { min = 0, max = 1 })
  t.equal(x >= 0 and x <= 1, true, tostring(x))
  local d = draw(g.dicts(g.sampled_from { "a", "b", "c" }, g.booleans(), { max = 3 }))
  for key, value in pairs(d) do
    t.equal(type(key) == "string" and type(value) == "boolean", true)
  end
  local unique = draw(g.lists(g.integers { min = 0, max = 3 }, { unique = true }))
  local seen = {}
  for _, n in ipairs(unique) do
    t.equal(seen[n], nil, "twice: " .. n)
    seen[n] = true
  end
end)
"#,
    );
    assert!(result.passed, "{result:?}");
}

/// `assume` drops a case rather than failing it; a property that drops
/// every case fails, saying so.
#[test]
fn assume_drops_a_case() {
    let results = results(
        r#"
local t = require("tau").test
local g = t.gen
t.property("halves", function(draw)
  local n = draw(g.integers { min = 0, max = 1000 })
  t.assume(n % 2 == 0)
  t.equal((n / 2) * 2, n)
end)
t.property("never", function(draw)
  draw(g.integers {})
  t.assume(false)
end)
"#,
    );
    let by: BTreeMap<&str, &TestResult> =
        results.iter().map(|r| (r.name.as_str(), r)).collect();
    assert!(by["halves"].passed, "{:?}", by["halves"]);
    assert!(!by["never"].passed, "{:?}", by["never"]);
}

/// A counter that skips 3 is caught by its invariant in the fewest
/// steps: three increments.
#[test]
fn a_machine_shrinks_to_its_fewest_steps() {
    let result = only(
        r#"
local t = require("tau").test
t.machine("a counter", {
  init = function() return { n = 0 } end,
  rules = {
    inc = function(m) m.n += 1 end,
    reset = { weight = 0.5, run = function(m) m.n = 0 end },
    skip = function(m, draw) t.assume(m.n > 10) end,
  },
  invariant = function(m) t.equal(m.n ~= 3, true, "the counter is never 3") end,
  steps = 20,
})
"#,
    );
    assert!(!result.passed);
    assert_eq!(result.kind, TestKind::Machine);
    let error = error(&result);
    assert!(error.contains("the counter is never 3"), "{error}");
    let steps: Vec<&str> = error
        .lines()
        .filter(|line| line.contains("step "))
        .collect();
    assert_eq!(
        steps,
        ["  step 1: inc", "  step 2: inc", "  step 3: inc"],
        "{error}"
    );
}

/// The same files give the same results, report and blob alike, and the
/// blob replays that failure.
#[test]
fn a_failure_is_the_same_each_time_and_its_blob_replays_it() {
    let tests = r#"
local t = require("tau").test
local g = t.gen
t.property("short", function(draw)
  local list = draw(g.lists(g.text { max = 3, codec = "ascii" }, { max = 5 }))
  t.equal(#list < 3, true, "three or more")
end)
"#;
    let first = only(tests);
    assert_eq!(timeless(first.clone()), timeless(only(tests)));
    let error = error(&first);
    let blob = error
        .split("reproduce = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    let replayed =
        only(&tests.replace(
            "end)\n",
            &format!("end, {{ reproduce = \"{blob}\" }})\n"),
        ));
    assert!(!replayed.passed);
    assert!(error_head(&replayed).contains("three or more"));
    assert_eq!(
        error_head(&replayed),
        error_head(&first),
        "the same failure"
    );
}

/// A report up to its blob.
fn error_head(result: &TestResult) -> String {
    error(result)
        .lines()
        .take_while(|line| !line.starts_with("reproduce:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A draw outside a property says it cannot be; cases and properties
/// sit in one file, each its own result.
#[test]
fn cases_and_properties_live_together() {
    let results = results(
        r#"
local t = require("tau").test
local g = t.gen
t.case("an example", function() t.equal(1 + 1, 2) end)
t.case("a draw here", function() local d = g.integers {} ; d.produce() end)
t.property("a property", function(draw) draw(g.booleans()) end)
"#,
    );
    let summary: Vec<(&str, bool)> = results
        .iter()
        .map(|r| (r.name.as_str(), r.passed))
        .collect();
    assert_eq!(
        summary,
        [
            ("an example", true),
            ("a draw here", false),
            ("a property", true)
        ]
    );
    let kinds: Vec<TestKind> = results.iter().map(|r| r.kind).collect();
    assert_eq!(kinds, [TestKind::Case, TestKind::Case, TestKind::Property]);
    assert!(results[0].millis.is_some(), "{:?}", results[0]);
    assert_eq!(results[0].cases, None);
    assert!(
        error(&results[1]).contains("a draw happens only inside a property"),
        "{:?}",
        results[1]
    );
}

/// A case that draws without end runs out of the engine's choices: an
/// overrun, not a failure of its own, and a property whose every case
/// overruns fails a health check rather than hanging.
#[test]
fn a_property_that_draws_without_end_overruns() {
    let results = results(
        r#"
local t = require("tau").test
local g = t.gen
t.property("sometimes long", function(draw)
  if draw(g.integers { min = 0, max = 9 }) == 9 then
    while true do draw(g.booleans()) end
  end
end)
t.property("always long", function(draw)
  while true do draw(g.booleans()) end
end)
"#,
    );
    let by: BTreeMap<&str, &TestResult> =
        results.iter().map(|r| (r.name.as_str(), r)).collect();
    assert!(by["sometimes long"].passed, "{:?}", by["sometimes long"]);
    let always = error(by["always long"]);
    assert!(always.contains("HealthCheck"), "{always}");
}
