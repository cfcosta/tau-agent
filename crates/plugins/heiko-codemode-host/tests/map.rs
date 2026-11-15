//! Property inventory: `map_matches_arithmetic_oracle` checks exact ordered
//! settled records against independent indexed arithmetic, including empty
//! arrays and every valid small concurrency, while each callback's tool
//! call takes a drawn time, and checks how many callbacks run at once. The
//! generator draws small JSON safe integers, durations and a valid
//! concurrency by construction; vector shrinking removes items and integer
//! shrinking simplifies values. Hegel uses the
//! workspace hegel.toml development profile locally and its shipped CI
//! profile on CI, so this test needs no per-test case count.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use common::{FakeHost, Held, beside, error, script, script_with, texts};
use hegel::{TestCase, generators as gs};
use heiko_codemode::store::Snapshot;
use heiko_codemode_host::{CallStatus, CancellationToken, Failure};
use heiko_testing::{block_on, block_on_io};
use serde_json::{Value, json};

use crate::common;

fn host() -> Arc<FakeHost> {
    Arc::new(FakeHost::default())
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

/// Whatever the items, the concurrency, how many tools each callback
/// runs in `parallel` and the order the test lets those calls go, `map`
/// runs as many callbacks at once as the concurrency allows and the
/// items left to start can fill: a callback starts only when one ends,
/// in item order, and never more run at once. Each call holds until the
/// test lets it go, so what runs beside what is set by what the test
/// let go, not by timing.
#[hegel::test(test_cases = 200)]
fn map_runs_callbacks_up_to_its_concurrency(tc: TestCase) {
    let items: usize = tc.draw(gs::integers::<usize>().max_value(8));
    let concurrency: usize =
        tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let width: usize =
        tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    let code = format!(
        r#"local running, peak = 0, 0
           local results = map(json.decode('{}'), function(item)
               running += 1
               peak = math.max(peak, running)
               local calls = {{}}
               for k = 1, {width} do
                   calls[k] = function()
                       return tools.hold({{ item = item, k = k }}).item
                   end
               end
               local values = {{ parallel(table.unpack(calls)) }}
               running -= 1
               local sum = 0
               for _, value in values do sum += value end
               return sum
           end, {concurrency})
           return {{ peak = peak, results = results }}"#,
        json!((1..=items).collect::<Vec<_>>())
    );
    let host = host();
    let mut held = host.held();
    let test = async {
        // The callbacks running, by item, with how many of their calls
        // are still held; the next item to start.
        let mut running: BTreeMap<u64, usize> = BTreeMap::new();
        let mut next = 1;
        let mut holding: Vec<Held> = Vec::new();
        let mut ended = 0;
        while next <= items && running.len() < concurrency {
            running.insert(next as u64, width);
            next += 1;
        }
        while ended < items {
            let live: usize = running.values().sum();
            while holding.len() < live {
                let call = held.recv().await.expect("a held call");
                let item = call.args["item"].as_u64().unwrap();
                assert!(
                    running.contains_key(&item),
                    "item {item} started before a callback ended: {running:?}"
                );
                assert!(call.running <= concurrency * width);
                holding.push(call);
            }
            let at =
                tc.draw(gs::integers::<usize>().max_value(holding.len() - 1));
            let call = holding.swap_remove(at);
            let item = call.args["item"].as_u64().unwrap();
            call.open();
            let left = running.get_mut(&item).unwrap();
            *left -= 1;
            if *left == 0 {
                running.remove(&item);
                ended += 1;
                if next <= items {
                    running.insert(next as u64, width);
                    next += 1;
                }
            }
        }
    };
    let outcome = block_on_io(beside(script(&host, &code), test));
    assert_eq!(outcome.failure, None);
    let value: Value = serde_json::from_str(&texts(&outcome)[0]).unwrap();
    let most = items.min(concurrency);
    assert_eq!(value["peak"], most, "callbacks running at once");
    assert_eq!(host.peak.load(Ordering::SeqCst), most * width);
    let expected: Vec<Value> = (1..=items)
        .map(|item| json!({"ok":true,"value":item * width}))
        .collect();
    assert_eq!(value["results"], json!(expected));
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

/// A timeout or a cancel stops a map alike: the callbacks running are
/// dropped, their calls marked cancelled, and queued items never start.
#[tokio::test]
async fn timeout_and_cancel_drop_running_callbacks_and_never_start_queued_items()
 {
    const MAP: &str = r#"return map(array({ 1, 2, 3, 4 }), function(item)
               return tools.sleep({ ms = 10000, item = item })
           end, 2)"#;
    for cancelled in [false, true] {
        let host = host();
        let outcome = if cancelled {
            let cancel = CancellationToken::new();
            let trigger = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                trigger.cancel();
            });
            script_with(&host, MAP, Snapshot::new(), cancel).await
        } else {
            script(
                &host,
                &format!("-- @options: {{\"timeout_ms\": 100}}\n{MAP}"),
            )
            .await
        };
        let stopped = if cancelled {
            Failure::Cancelled
        } else {
            Failure::TimedOut { timeout_ms: 100 }
        };
        assert_eq!(outcome.failure, Some(stopped));
        assert_eq!(host.names().len(), 2);
        assert_eq!(host.dropped.load(Ordering::SeqCst), 2);
        assert!(
            outcome
                .calls
                .iter()
                .all(|row| row.status == CallStatus::Cancelled)
        );
    }
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

/// Whatever the items, the concurrency and how long each callback's tool
/// call takes, the settled records come back in item order with the
/// values independent arithmetic gives, and as many callbacks run at
/// once as the concurrency allows: no more, and, since every call takes
/// some time, no fewer. Time is tokio's paused clock, so the drawn
/// durations reorder completions without real waiting.
#[hegel::test]
fn map_matches_arithmetic_oracle(tc: TestCase) {
    let inputs: Vec<(i16, u8)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::integers::<i16>().min_value(-100).max_value(100),
            gs::integers::<u8>().min_value(1).max_value(50)
        ))
        .max_size(12),
    );
    let concurrency: u8 =
        tc.draw(gs::integers::<u8>().min_value(1).max_value(8));
    let pairs: Vec<[i64; 2]> = inputs
        .iter()
        .map(|(item, ms)| [i64::from(*item), i64::from(*ms)])
        .collect();
    let code = format!(
        "return map(json.decode('{}'), function(pair, index) \
         tools.sleep({{ ms = pair[2] }}) return pair[1] * 3 + index end, {concurrency})",
        json!(pairs)
    );
    let host = host();
    let outcome = block_on(script(&host, &code));
    assert_eq!(outcome.failure, None);
    let actual: Value = serde_json::from_str(&texts(&outcome)[0]).unwrap();
    let expected: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(index, (item, _))| json!({"ok":true,"value":i32::from(*item) * 3 + index as i32 + 1}))
        .collect();
    assert_eq!(actual, json!(expected));
    assert_eq!(
        host.peak.load(Ordering::SeqCst),
        inputs.len().min(usize::from(concurrency)),
        "callbacks running at once"
    );
}
