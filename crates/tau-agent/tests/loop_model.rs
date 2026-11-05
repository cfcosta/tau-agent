//! The agent loop (`docs/reference/agent-loop.md`) against a model of
//! it ([`tau_agent::testing::loop_model`]), driven by tau-agent's own
//! loop: what any filler of `tau.loop` is held to as well.

use std::sync::Arc;

use hegel::TestCase;
use tau_agent::{
    runner::driver::Kernel,
    testing::{Stepwise, loop_model},
};

#[hegel::test(test_cases = 300)]
fn the_loop_keeps_to_its_model(tc: TestCase) {
    loop_model::check(tc, Arc::new(Kernel), 30)
}

/// [`the_loop_keeps_to_its_model`] with more cases and longer runs, for
/// the nightly tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn the_loop_keeps_to_its_model_nightly(tc: TestCase) {
    loop_model::check(tc, Arc::new(Kernel), 60)
}

/// The loop put together from the run's steps keeps to the same model.
#[hegel::test(test_cases = 100)]
fn a_loop_of_the_runs_steps_keeps_to_its_model(tc: TestCase) {
    loop_model::check(tc, Arc::new(Stepwise), 30)
}

/// The histories the model found once, each checked again.
#[test]
fn the_loop_keeps_to_its_regressions() {
    loop_model::regressions(Arc::new(Kernel));
    loop_model::regressions(Arc::new(Stepwise));
}
