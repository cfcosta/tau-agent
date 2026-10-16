//! Run limits (`tau_agent::limits`), against a plain reference
//! (`common::reference`).

use std::time::Duration;

use hegel::{TestCase, generators as gs};
use tau_agent::limits::Limits;
use tau_ai::message::{Usage, UsageCost};

mod common;
use common::reference;

#[hegel::test(test_cases = 500)]
fn reached_matches_reference(tc: TestCase) {
    let small = || gs::integers::<u64>().max_value(100);
    let turns_limit =
        tc.draw(gs::optional(gs::integers::<u32>().max_value(10)));
    let tokens_limit =
        tc.draw(gs::optional(gs::integers::<u64>().max_value(400)));
    let usd_limit = tc.draw(gs::optional(small())).map(|n| n as f64 / 8.0);
    let timeout = tc.draw(gs::optional(small())).map(Duration::from_secs);
    let mut limits = Limits::default();
    if let Some(n) = turns_limit {
        limits = limits.max_turns(n);
    }
    if let Some(n) = tokens_limit {
        limits = limits.max_tokens(n);
    }
    if let Some(usd) = usd_limit {
        limits = limits.max_usd(usd);
    }
    if let Some(after) = timeout {
        limits = limits.timeout(after);
    }
    // Each builder sets its own limit and no other.
    assert_eq!(
        (
            limits.max_turns,
            limits.max_tokens,
            limits.max_usd,
            limits.timeout
        ),
        (turns_limit, tokens_limit, usd_limit, timeout)
    );
    let usage = Usage {
        input: tc.draw(small()),
        output: tc.draw(small()),
        cache_read: tc.draw(small()),
        cache_write: tc.draw(small()),
        cost: UsageCost {
            total: tc.draw(small()) as f64 / 8.0,
            ..UsageCost::default()
        },
        ..Usage::default()
    };
    let turns = tc.draw(gs::integers::<u32>().max_value(10));
    let elapsed = Duration::from_secs(tc.draw(small()));
    assert_eq!(
        limits.reached(turns, &usage, elapsed),
        reference(&limits, turns, &usage, elapsed)
    );
}
