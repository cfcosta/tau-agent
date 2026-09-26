//! Run limits (`tau_agent::limits`), against a plain reference.

use std::time::Duration;

use hegel::{TestCase, generators as gs};
use tau_agent::{event::LimitKind, limits::Limits};
use tau_ai::message::{Usage, UsageCost};

/// The reference: each limit, checked in order turns, tokens, cost, time,
/// reached when the value meets or passes it.
fn reference(
    limits: &Limits,
    turns: u32,
    usage: &Usage,
    elapsed: Duration,
) -> Option<LimitKind> {
    let tokens =
        usage.input + usage.output + usage.cache_read + usage.cache_write;
    [
        (limits.max_turns.map(|m| turns >= m), LimitKind::Turns),
        (limits.max_tokens.map(|m| tokens >= m), LimitKind::Tokens),
        (
            limits.max_usd.map(|m| usage.cost.total >= m),
            LimitKind::Usd,
        ),
        (limits.timeout.map(|m| elapsed >= m), LimitKind::Time),
    ]
    .into_iter()
    .find_map(|(hit, kind)| (hit == Some(true)).then_some(kind))
}

#[hegel::test(test_cases = 500)]
fn reached_matches_reference(tc: TestCase) {
    let small = || gs::integers::<u64>().max_value(100);
    let mut limits = Limits::default();
    if tc.draw(gs::booleans()) {
        limits = limits.max_turns(tc.draw(gs::integers::<u32>().max_value(10)));
    }
    if tc.draw(gs::booleans()) {
        limits =
            limits.max_tokens(tc.draw(gs::integers::<u64>().max_value(400)));
    }
    if tc.draw(gs::booleans()) {
        limits = limits.max_usd(tc.draw(small()) as f64 / 8.0);
    }
    if tc.draw(gs::booleans()) {
        limits = limits.timeout(Duration::from_secs(tc.draw(small())));
    }
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

/// Each builder sets exactly its own limit, and exactly at the limit
/// counts as reached.
#[test]
fn builders_and_boundaries() {
    let usage = Usage {
        input: 1,
        output: 2,
        cache_read: 3,
        cache_write: 4,
        cost: UsageCost {
            total: 0.5,
            ..UsageCost::default()
        },
        ..Usage::default()
    };
    let at = |limits: Limits| limits.reached(3, &usage, Duration::from_secs(9));
    assert_eq!(at(Limits::default()), None);
    assert_eq!(at(Limits::default().max_turns(3)), Some(LimitKind::Turns));
    assert_eq!(at(Limits::default().max_turns(4)), None);
    assert_eq!(
        at(Limits::default().max_tokens(10)),
        Some(LimitKind::Tokens)
    );
    assert_eq!(at(Limits::default().max_tokens(11)), None);
    assert_eq!(at(Limits::default().max_usd(0.5)), Some(LimitKind::Usd));
    assert_eq!(at(Limits::default().max_usd(0.75)), None);
    assert_eq!(
        at(Limits::default().timeout(Duration::from_secs(9))),
        Some(LimitKind::Time)
    );
    assert_eq!(at(Limits::default().timeout(Duration::from_secs(10))), None);
    let all = Limits::default()
        .max_turns(1)
        .max_tokens(2)
        .max_usd(3.0)
        .timeout(Duration::from_secs(4));
    assert_eq!(
        (all.max_turns, all.max_tokens, all.max_usd, all.timeout),
        (Some(1), Some(2), Some(3.0), Some(Duration::from_secs(4)))
    );
}
