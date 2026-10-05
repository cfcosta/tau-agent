//! Inference admission and accounting.
//!
//! Property inventory: `sequential_attempts_match_counter_model` compares
//! admission, reporting, dropping, and snapshots with an independent integer
//! counter model. It catches counting queued calls and double charging usage.
//! Generator plan: bounded valid limits and short operation sequences are
//! drawn directly; smaller values and shorter sequences shrink failures.
//! CI: the workspace `hegel.toml` chooses the suite-wide local/CI profiles;
//! CI derandomizes and disables the example database. No per-test count is
//! needed for this small, deterministic, paused-time model.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::time::Duration;

use hegel::{TestCase, generators as gs};
use tau_ai::message::{Usage, UsageCost};
use tau_codemode::{
    CancellationToken,
    inference_budget::{Budget, Limits, Permit},
};

fn limits() -> Limits {
    Limits::default()
}

fn usage(tokens: u64, cost: f64) -> Usage {
    Usage {
        input: tokens,
        total_tokens: tokens,
        cost: UsageCost {
            total: cost,
            ..UsageCost::default()
        },
        ..Usage::default()
    }
}

#[test]
fn defaults_and_invalid_limits() {
    assert_eq!(
        limits(),
        Limits {
            max_calls: 16,
            max_concurrency: 4,
            timeout: Duration::from_secs(60),
            max_tokens: Some(100_000),
            max_cost_usd: Some(1.0),
        }
    );
    let mut candidate = limits();
    candidate.max_concurrency = 0;
    assert!(Budget::new(candidate).is_err());
    candidate.max_concurrency = usize::MAX;
    assert!(Budget::new(candidate).is_err());
    candidate.max_concurrency = 1;
    candidate.timeout = Duration::MAX;
    assert!(Budget::new(candidate).is_err());
    candidate.timeout = Duration::from_secs(1);
    for cost in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        candidate.max_cost_usd = Some(cost);
        assert!(Budget::new(candidate).is_err());
    }
    candidate.max_cost_usd = None;
    assert!(Budget::new(candidate).is_ok());
}

#[tokio::test(start_paused = true)]
async fn zero_exact_and_exceeded_call_limits() {
    let cancel = CancellationToken::new();
    let zero = Budget::new(Limits {
        max_calls: 0,
        ..limits()
    })
    .unwrap();
    assert!(zero.admit(&cancel).await.is_err());
    assert_eq!(zero.snapshot().calls, 0);

    let budget = Budget::new(Limits {
        max_calls: 2,
        ..limits()
    })
    .unwrap();
    drop(budget.admit(&cancel).await.unwrap());
    drop(budget.admit(&cancel).await.unwrap());
    assert!(budget.admit(&cancel).await.is_err());
    assert_eq!(budget.snapshot().calls, 2);
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[tokio::test(start_paused = true)]
async fn concurrent_waiter_is_counted_only_after_a_slot_is_free() {
    let budget = Budget::new(Limits {
        max_concurrency: 2,
        ..limits()
    })
    .unwrap();
    let cancel = CancellationToken::new();
    let first = budget.admit(&cancel).await.unwrap();
    let second = budget.admit(&cancel).await.unwrap();
    let waiting = {
        let budget = budget.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { budget.admit(&cancel).await })
    };
    tokio::task::yield_now().await;
    assert_eq!(budget.snapshot().calls, 2);
    assert_eq!(budget.snapshot().in_flight, 2);
    assert!(!waiting.is_finished());
    drop(first);
    let third = waiting.await.unwrap().unwrap();
    assert_eq!(budget.snapshot().calls, 3);
    assert_eq!(budget.snapshot().in_flight, 2);
    drop((second, third));
    assert_eq!(budget.snapshot().in_flight, 0);
}

#[tokio::test(start_paused = true)]
async fn cancelled_waiter_and_dropped_waiter_do_not_count() {
    let budget = Budget::new(Limits {
        max_concurrency: 1,
        ..limits()
    })
    .unwrap();
    let cancel = CancellationToken::new();
    let held = budget.admit(&cancel).await.unwrap();
    let cancelled = {
        let budget = budget.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { budget.admit(&cancel).await })
    };
    tokio::task::yield_now().await;
    cancel.cancel();
    assert!(cancelled.await.unwrap().is_err());
    let dropped = {
        let budget = budget.clone();
        tokio::spawn(
            async move { budget.admit(&CancellationToken::new()).await },
        )
    };
    tokio::task::yield_now().await;
    dropped.abort();
    let _ = dropped.await;
    assert_eq!(budget.snapshot().calls, 1);
    drop(held);
    assert_eq!(budget.snapshot().in_flight, 0);
    assert!(budget.admit(&cancel).await.is_err());
    assert_eq!(budget.snapshot().calls, 1);
}

#[tokio::test(start_paused = true)]
async fn deadline_stops_a_waiter_and_a_later_admission() {
    let budget = Budget::new(Limits {
        max_concurrency: 1,
        timeout: Duration::from_secs(5),
        ..limits()
    })
    .unwrap();
    let cancel = CancellationToken::new();
    let held = budget.admit(&cancel).await.unwrap();
    let waiting = {
        let budget = budget.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { budget.admit(&cancel).await })
    };
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    assert!(waiting.await.unwrap().is_err());
    assert_eq!(budget.snapshot().calls, 1);
    drop(held);
    assert!(budget.admit(&cancel).await.is_err());
    assert_eq!(budget.snapshot().calls, 1);
}

#[tokio::test(start_paused = true)]
async fn report_once_and_soft_thresholds() {
    let budget = Budget::new(Limits {
        max_tokens: Some(10),
        max_cost_usd: Some(1.0),
        ..limits()
    })
    .unwrap();
    let cancel = CancellationToken::new();
    let mut first = budget.admit(&cancel).await.unwrap();
    let mut second = budget.admit(&cancel).await.unwrap();
    first.report(&usage(6, 0.6));
    first.report(&usage(99, 99.0));
    assert_eq!(budget.snapshot().usage, usage(6, 0.6));
    second.report(&usage(6, 0.6));
    assert_eq!(budget.snapshot().usage.input, 12);
    assert_eq!(budget.snapshot().usage.cost.total, 1.2);
    drop((first, second));
    assert!(budget.admit(&cancel).await.is_err());
    assert_eq!(budget.snapshot().calls, 2);
}

#[tokio::test(start_paused = true)]
async fn token_and_cost_limits_stop_later_calls_independently() {
    let cancel = CancellationToken::new();
    for (tokens, cost) in [(10, 0.0), (0, 1.0), (11, 1.1)] {
        let budget = Budget::new(Limits {
            max_tokens: Some(10),
            max_cost_usd: Some(1.0),
            ..limits()
        })
        .unwrap();
        let mut permit = budget.admit(&cancel).await.unwrap();
        permit.report(&usage(tokens, cost));
        drop(permit);
        assert!(budget.admit(&cancel).await.is_err());
        assert_eq!(budget.snapshot().calls, 1);
    }
}

#[hegel::test]
fn sequential_attempts_match_counter_model(tc: TestCase) {
    let max_calls: u8 = tc.draw(gs::integers::<u8>().max_value(6));
    let max_concurrency: u8 =
        tc.draw(gs::integers::<u8>().min_value(1).max_value(4));
    let max_tokens: u8 = tc.draw(gs::integers::<u8>().max_value(12));
    let max_cost: u8 = tc.draw(gs::integers::<u8>().min_value(1).max_value(6));
    let operations: Vec<(u8, u8, u8)> = tc.draw(
        gs::vecs(gs::tuples3(
            gs::integers::<u8>().max_value(2),
            gs::integers::<u8>().max_value(4),
            gs::integers::<u8>().max_value(3),
        ))
        .max_size(24),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let budget = Budget::new(Limits {
            max_calls: max_calls.into(),
            max_concurrency: max_concurrency.into(),
            timeout: Duration::from_secs(60),
            max_tokens: Some(max_tokens.into()),
            max_cost_usd: Some(f64::from(max_cost)),
        })
        .unwrap();
        let cancel = CancellationToken::new();
        let mut permits: Vec<(Permit, bool)> = Vec::new();
        let mut calls = 0;
        let mut tokens = 0_u64;
        let mut cost = 0_f64;

        for (operation, token_charge, cost_charge) in operations {
            match operation {
                0 if permits.len() < usize::from(max_concurrency) => {
                    let allowed = calls < usize::from(max_calls)
                        && tokens < u64::from(max_tokens)
                        && cost < f64::from(max_cost);
                    let admission = budget.admit(&cancel).await;
                    assert_eq!(admission.is_ok(), allowed);
                    if let Ok(permit) = admission {
                        permits.push((permit, false));
                        calls += 1;
                    }
                }
                1 => {
                    if let Some((permit, reported)) =
                        permits.iter_mut().find(|(_, done)| !*done)
                    {
                        permit.report(&usage(
                            u64::from(token_charge),
                            f64::from(cost_charge),
                        ));
                        permit.report(&usage(100, 100.0));
                        *reported = true;
                        tokens += u64::from(token_charge);
                        cost += f64::from(cost_charge);
                    }
                }
                _ => {
                    if !permits.is_empty() {
                        permits.remove(0);
                    }
                }
            }
            let snapshot = budget.snapshot();
            assert_eq!(snapshot.calls, calls);
            assert_eq!(snapshot.in_flight, permits.len());
            assert_eq!(snapshot.usage.input, tokens);
            assert_eq!(snapshot.usage.cost.total, cost);
        }
    });
}
