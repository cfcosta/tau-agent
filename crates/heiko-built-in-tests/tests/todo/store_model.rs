//! The store (ADR 0048, "2. The store"): the plugin's tools on the real
//! SQLite store, one to three at once, with a clock moved forward and
//! back, and after every step the store reads back exactly what the
//! reference model, which makes each change the slow, plain way, says.
//! Calls at once are each one step, in some order: one of the orders
//! explains every answer and what the store keeps.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::Arc;

use hegel::{TestCase, generators as gs};
use serde_json::Value;
use support::{
    Plugin,
    kept,
    store::{Op, Step, Store, draw_step, explain},
};
use heiko_luau_plugins_host::store::{Fuse, Kv};
use heiko_testing::block_on_io;

use super::support;

/// Makes `batch`'s calls at once, and answers the model after them:
/// some order of them explains every answer and what the store keeps.
fn at_once(
    plugin: &Plugin,
    store: &Arc<Kv>,
    model: &Store,
    batch: &[Step],
) -> (Store, Vec<Result<Value, String>>) {
    let answers: Vec<Result<Value, String>> = block_on_io(futures_join(
        batch
            .iter()
            .map(|step| {
                let (hook, input) = step.call();
                async move {
                    plugin.call(store, &step.run, step.now, hook, input).await
                }
            })
            .collect(),
    ));
    let kept_now = block_on_io(kept(store));
    (explain(model, batch, &answers, &kept_now), answers)
}

fn run_model(tc: TestCase, steps: usize) {
    let plugin = Plugin::new();
    let store = Arc::new(block_on_io(Kv::memory(Fuse::never())).unwrap());
    let mut model = Store::default();
    let mut now: i64 = 1_793_700_000;
    for _ in 0..steps {
        let writers: usize = tc.draw(gs::integers().min_value(1).max_value(3));
        let batch: Vec<Step> = (0..writers)
            .map(|_| draw_step(&tc, &model, &mut now))
            .collect();
        model = at_once(&plugin, &store, &model, &batch).0;
    }
}

/// Runs `futures` at once, answering in their order.
async fn futures_join<F: std::future::Future>(
    futures: Vec<F>,
) -> Vec<F::Output> {
    futures_util::future::join_all(futures).await
}

#[hegel::test(test_cases = 60)]
fn the_store_keeps_what_the_model_says(tc: TestCase) {
    run_model(tc, 20);
}

#[hegel::test(test_cases = 1000)]
#[ignore = "nightly"]
fn the_store_keeps_what_the_model_says_nightly(tc: TestCase) {
    run_model(tc, 60);
}

/// Found by the model: a run files an issue while another deletes the
/// one it holds. The new issue is found while the held one, read before
/// the delete and looked up after it: the call answered "there is no
/// issue", of one it never named, where it files the issue with no such
/// link, as after the delete.
#[test]
fn an_issue_is_filed_as_the_one_its_run_holds_is_deleted() {
    let plugin = Plugin::new();
    let store = Arc::new(block_on_io(Kv::memory(Fuse::never())).unwrap());
    let now: i64 = 1_793_700_000;
    let at = |run: &str, op: Op| Step {
        run: run.into(),
        now,
        op,
    };
    let create = |kind: &str, parent: Option<String>| Op::Create {
        title: "issue 0".into(),
        kind: kind.into(),
        priority: 0,
        criteria: 0,
        parent,
    };
    let mut model = Store::default();
    let mut filed = Vec::new();
    for kind in ["epic", "task"] {
        let (next, answers) =
            at_once(&plugin, &store, &model, &[at("r1", create(kind, None))]);
        model = next;
        let id = answers[0].as_ref().unwrap()["issue"]["id"].as_str().unwrap();
        filed.push(id.to_owned());
    }
    let (epic, held) = (filed[0].clone(), filed[1].clone());
    let claim = at("r2", Op::Claim { id: held.clone() });
    model = at_once(&plugin, &store, &model, &[claim]).0;
    assert_eq!(model.held_by("r2"), std::slice::from_ref(&held));

    let batch = [
        at("r1", Op::Delete { id: held.clone() }),
        at("r1", Op::RunEnd),
        at("r2", create("task", Some(epic.clone()))),
    ];
    let (model, answers) = at_once(&plugin, &store, &model, &batch);
    assert!(answers.iter().all(Result::is_ok), "{answers:#?}");
    assert!(!model.issues.contains_key(&held));
    assert_eq!(
        model.issues[&format!("{epic}.1")]["links"],
        serde_json::json!([{ "id": epic, "kind": "parent" }])
    );
}
