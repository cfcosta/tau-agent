//! Property inventory: `map_matches_arithmetic_oracle` checks exact ordered
//! settled records against independent indexed arithmetic, including empty
//! arrays and every valid small concurrency. The generator draws small JSON
//! safe integers and a valid concurrency by construction; vector shrinking
//! removes items and integer shrinking simplifies values. Hegel uses the
//! workspace hegel.toml development profile locally and its shipped CI
//! profile on CI, so this test needs no per-test case count.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use common::{FakeHost, error, script, script_with, texts};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_codemode::{CallStatus, CancellationToken, Failure, store::Snapshot};
use tau_testing::block_on;

fn host() -> Arc<FakeHost> {
    Arc::new(FakeHost::default())
}

#[tokio::test]
async fn preserves_item_order_despite_staggered_completion() {
    let host = host();
    let outcome = script(
        &host,
        r#"return map(array({ 80, 10, 40 }), function(ms, index)
            tools.sleep({ ms = ms })
            return { item = ms, index = index }
        end, 3)"#,
    )
    .await;
    assert_eq!(outcome.failure, None);
    assert_eq!(
        texts(&outcome),
        [
            r#"[{"ok":true,"value":{"index":1,"item":80}},{"ok":true,"value":{"index":2,"item":10}},{"ok":true,"value":{"index":3,"item":40}}]"#
        ]
    );
    assert_eq!(host.peak.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn settles_errors_without_stopping_siblings() {
    let host = host();
    let outcome = script(
        &host,
        r#"return map(array({ 1, 2, 3 }), function(item)
            if item == 2 then tools.fail({}) end
            return tools.echo({ item = item }).item
        end)"#,
    )
    .await;
    assert_eq!(outcome.failure, None);
    let records: Value = serde_json::from_str(&texts(&outcome)[0]).unwrap();
    assert_eq!(records[0], json!({"ok":true,"value":1}));
    assert_eq!(records[1]["ok"], false);
    assert!(records[1]["error"].as_str().unwrap().contains("tool broke"));
    assert_eq!(records[2], json!({"ok":true,"value":3}));
    assert_eq!(host.names().len(), 3);
}

#[tokio::test]
async fn bounds_running_callbacks_with_nested_parallel_tools() {
    let host = host();
    let outcome = script(
        &host,
        r#"local running, peak = 0, 0
           local results = map(array({ 1, 2, 3, 4, 5, 6 }), function(item)
               running += 1
               peak = math.max(peak, running)
               local a, b = parallel(
                   function() return tools.sleep({ ms = 40 }) end,
                   function() return tools.sleep({ ms = 40 }) end
               )
               running -= 1
               return item + a + b
           end, 2)
           return { peak = peak, results = results }"#,
    )
    .await;
    assert_eq!(outcome.failure, None);
    let value: Value = serde_json::from_str(&texts(&outcome)[0]).unwrap();
    assert_eq!(value["peak"], 2);
    assert_eq!(host.peak.load(Ordering::SeqCst), 4);
    assert_eq!(value["results"][5], json!({"ok":true,"value":86}));
}

#[tokio::test]
async fn empty_array_round_trips_and_callback_values_are_explicit() {
    let outcome = script(
        &host(),
        r#"local empty = map(array({}), function() error('not called') end)
           local values = map(array({ json.null, 1, 2, 3 }), function(item, index)
               if index == 1 then return item end
               if index == 2 then return nil end
               if index == 3 then return 7, 8 end
           end)
           return { empty = empty, values = values }"#,
    )
    .await;
    assert_eq!(outcome.failure, None);
    let value: Value = serde_json::from_str(&texts(&outcome)[0]).unwrap();
    assert_eq!(value["empty"], json!([]));
    assert_eq!(
        value["values"],
        json!([
            {"ok":true,"value":null},
            {"ok":true,"value":null},
            {"ok":true,"value":7},
            {"ok":true,"value":null}
        ])
    );
}

#[tokio::test]
async fn rejects_invalid_collection_and_concurrency_before_calls() {
    let host = host();
    for (code, expected) in [
        ("map({}, function() end)", "marked array"),
        ("map(3, function() end)", "must be an array"),
        ("map(array({1, [3] = 3}), function() end)", "dense array"),
        ("map(array({1, named = 2}), function() end)", "dense array"),
        ("map(array({1}), 3)", "fn must be a function"),
        ("map(array({1}), function() end, 0)", "integer from 1 to 32"),
        (
            "map(array({1}), function() end, 33)",
            "integer from 1 to 32",
        ),
        (
            "map(array({1}), function() end, 1.5)",
            "integer from 1 to 32",
        ),
    ] {
        let outcome = script(&host, code).await;
        assert!(error(&outcome).contains(expected), "{code}: {outcome:?}");
    }
    assert!(host.names().is_empty());
}

#[tokio::test]
async fn limits_collection_to_ten_thousand_items() {
    let outcome = script(
        &host(),
        r#"local items = array({})
           for i = 1, 10001 do items[i] = i end
           map(items, function() end)"#,
    )
    .await;
    assert!(error(&outcome).contains("10000 elements"));
}

#[tokio::test]
async fn timeout_drops_running_callbacks_and_never_starts_queued_items() {
    let host = host();
    let outcome = script(
        &host,
        r#"-- @options: {"timeout_ms": 100}
           return map(array({ 1, 2, 3, 4 }), function(item)
               return tools.sleep({ ms = 10000, item = item })
           end, 2)"#,
    )
    .await;
    assert_eq!(outcome.failure, Some(Failure::TimedOut { timeout_ms: 100 }));
    assert_eq!(host.names().len(), 2);
    assert_eq!(host.dropped.load(Ordering::SeqCst), 2);
    assert!(
        outcome
            .calls
            .iter()
            .all(|row| row.status == CallStatus::Cancelled)
    );
}

#[tokio::test]
async fn cancellation_drops_running_callbacks_and_never_starts_queued_items() {
    let host = host();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let outcome = script_with(
        &host,
        r#"return map(array({ 1, 2, 3, 4 }), function(item)
               return tools.sleep({ ms = 10000, item = item })
           end, 2)"#,
        Snapshot::new(),
        cancel,
    )
    .await;
    assert_eq!(outcome.failure, Some(Failure::Cancelled));
    assert_eq!(host.names().len(), 2);
    assert_eq!(host.dropped.load(Ordering::SeqCst), 2);
    assert!(
        outcome
            .calls
            .iter()
            .all(|row| row.status == CallStatus::Cancelled)
    );
}

#[tokio::test]
async fn map_starts_fresh_in_each_vm() {
    let host = host();
    let first = script(
        &host,
        "counter = 9; return map(array({1}), function(x) return x end)",
    )
    .await;
    assert_eq!(first.failure, None);
    let second = script(&host, "return counter == nil, type(map)").await;
    assert_eq!(texts(&second), ["true", "function"]);
}

#[hegel::test]
fn map_matches_arithmetic_oracle(tc: TestCase) {
    let inputs: Vec<i16> = tc.draw(
        gs::vecs(gs::integers::<i16>().min_value(-100).max_value(100))
            .max_size(12),
    );
    let concurrency: u8 =
        tc.draw(gs::integers::<u8>().min_value(1).max_value(8));
    let code = format!(
        "return map(json.decode('{}'), function(item, index) return item * 3 + index end, {concurrency})",
        json!(inputs)
    );
    let outcome = block_on(script(&host(), &code));
    assert_eq!(outcome.failure, None);
    let actual: Value = serde_json::from_str(&texts(&outcome)[0]).unwrap();
    let expected: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(index, item)| json!({"ok":true,"value":i32::from(*item) * 3 + index as i32 + 1}))
        .collect();
    assert_eq!(actual, json!(expected));
}
