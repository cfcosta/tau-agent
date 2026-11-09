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
    store::{Step, Store, draw_step, explain},
};
use tau_luau_plugins_host::store::{Fuse, Kv};
use tau_testing::block_on_io;

use super::support;

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
        let answers: Vec<Result<Value, String>> = block_on_io(futures_join(
            batch
                .iter()
                .map(|step| {
                    let (hook, input) = step.call();
                    let (plugin, store) = (&plugin, &store);
                    async move {
                        plugin
                            .call(store, &step.run, step.now, hook, input)
                            .await
                    }
                })
                .collect(),
        ));
        let kept_now = block_on_io(kept(&store));
        model = explain(&model, &batch, &answers, &kept_now);
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
