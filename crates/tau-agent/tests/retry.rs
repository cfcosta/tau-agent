//! Retries of failed responses inside the loop
//! (`docs/reference/agent-loop.md`, "Retries"), driven through `Agent`
//! with `ScriptedModel`, `Store::memory()` and paused time.

use std::time::Duration;

use futures_util::StreamExt;
use hegel::{TestCase, generators as gs};
use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
};
use tau_ai::{
    message::{Message, StopReason as MessageStop},
    retry::RetryPolicy,
};
use tau_store::Store;
use tau_testing::{
    block_on,
    generators,
    scripted::{ScriptedModel, TurnBuilder},
};
use tokio::time::Instant;

mod common;
use common::stored;

/// A retryable failure, as the scripts draw it.
#[derive(
    Debug, Clone, Copy, hegel::PrettyPrintable, hegel::DefaultGenerator,
)]
enum Retryable {
    ServerError,
    RateLimited,
    NeverStarted,
}

fn fail(t: TurnBuilder, kind: Retryable) -> TurnBuilder {
    match kind {
        Retryable::ServerError => t.error("server_error", "try again"),
        Retryable::RateLimited => t.error("rate_limit_exceeded", "slow down"),
        Retryable::NeverStarted => t.fails_before_start(),
    }
}

/// The most a backoff before attempt `attempt` (the second attempt is
/// 2) may wait: `base * 2^(attempt - 2)`, capped at `max_delay`
/// (`docs/reference/agent-loop.md`, "Retries"), computed here rather
/// than asked of the policy.
fn backoff_cap(policy: &RetryPolicy, attempt: u32) -> Duration {
    policy
        .base
        .saturating_mul(1 << (attempt - 2))
        .min(policy.max_delay)
}

/// Retries over drawn failure runs, against a model: a turn fails `k`
/// times in a row in retryable ways, then succeeds, under a drawn policy
/// of `max` attempts. A `Retry` event announces each new attempt with a
/// delay within the backoff cap, and the run waits at least that long.
/// If `k < max` the run recovers: the model is asked `k + 1` times, and
/// none of the failed responses is stored. Otherwise the run fails after
/// exactly `max` requests, and only the last failure is stored.
#[hegel::test(test_cases = 60)]
#[hegel::explicit_test_case(
    failures = vec![Retryable::ServerError, Retryable::NeverStarted],
    policy = RetryPolicy {
        max_attempts: 0,
        base: Duration::from_millis(1500),
        max_delay: Duration::from_secs(4),
    },
    max_attempts = 2u32,
)]
#[hegel::explicit_test_case(
    failures = vec![Retryable::RateLimited, Retryable::ServerError],
    policy = RetryPolicy {
        max_attempts: 0,
        base: Duration::from_millis(1500),
        max_delay: Duration::from_secs(4),
    },
    max_attempts = 3u32,
)]
fn retryable_failures_are_retried_within_the_policy(tc: TestCase) {
    let failures: Vec<Retryable> =
        tc.draw(gs::vecs(gs::default::<Retryable>()).max_size(4));
    let policy = tc.draw(generators::retry_policy());
    let max_attempts = tc.draw(gs::integers::<u32>().min_value(1).max_value(5));
    let policy = RetryPolicy {
        max_attempts,
        ..policy
    };
    let mut llm = ScriptedModel::new();
    for &kind in &failures {
        llm = llm.turn(move |t| fail(t, kind));
    }
    llm = llm.turn(|t| t.text("done"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let started = Instant::now();
        let mut run = Agent::new(llm.clone()).retry(policy).start("go", &store);
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();
        let elapsed = started.elapsed();

        let retries: Vec<(u32, Duration)> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::Retry {
                    turn: 1,
                    attempt,
                    delay,
                    ..
                } => Some((*attempt, *delay)),
                RunEvent::Retry { .. } => panic!("retry outside turn 1"),
                _ => None,
            })
            .collect();
        for &(attempt, delay) in &retries {
            assert!(delay <= backoff_cap(&policy, attempt), "{retries:?}");
        }
        let waited: Duration = retries.iter().map(|(_, delay)| *delay).sum();
        assert!(elapsed >= waited, "waited {elapsed:?} of {retries:?}");
        let attempts: Vec<u32> = retries.iter().map(|(a, _)| *a).collect();
        let transcript = stored(&store, &outcome.run.0).await;
        let k = failures.len() as u32;
        if k < policy.max_attempts {
            assert_eq!(outcome.stop, StopReason::Stop);
            assert_eq!(outcome.text, "done");
            assert_eq!(llm.requests().len() as u32, k + 1);
            assert_eq!(attempts, (2..=k + 1).collect::<Vec<_>>());
            assert_eq!(transcript.len(), 2, "{transcript:?}");
            assert!(matches!(transcript[1], Message::Assistant(_)));
        } else {
            let StopReason::Error(error) = &outcome.stop else {
                panic!("{:?}", outcome.stop)
            };
            assert_eq!(llm.requests().len() as u32, policy.max_attempts);
            assert_eq!(attempts, (2..=policy.max_attempts).collect::<Vec<_>>());
            // The input, then the last failure; the failures retried
            // before it are never stored.
            assert_eq!(transcript.len(), 2, "{transcript:?}");
            assert!(matches!(transcript[0], Message::User(_)));
            let Message::Assistant(last) = &transcript[1] else {
                panic!("{transcript:?}")
            };
            assert_eq!(last.stop_reason, MessageStop::Error);
            assert_eq!(last.error_message.as_ref(), Some(error));
        }
    });
}

/// Failures that retrying cannot fix are not retried: a quota error, an
/// invalid request, and a connection dropped after output began.
#[test]
fn fatal_failures_are_not_retried() {
    let cases: [fn(TurnBuilder) -> TurnBuilder; 3] = [
        |t| t.error("insufficient_quota", "pay up"),
        |t| t.error("invalid_api_key", "who are you"),
        |t| t.dropped(),
    ];
    for case in cases {
        let llm = ScriptedModel::new().turn(case).turn(|t| t.text("never"));
        block_on(async {
            let store = Store::memory().await.unwrap();
            let outcome =
                Agent::new(llm.clone()).run("go", &store).await.unwrap();
            assert!(matches!(outcome.stop, StopReason::Error(_)));
            assert_eq!(llm.requests().len(), 1);
        });
    }
}

/// A cancel during the backoff ends the run as cancelled at once,
/// without another request.
#[test]
fn a_cancel_during_backoff_ends_the_run() {
    let llm = ScriptedModel::new()
        .turn(|t| t.error("server_error", "try again"))
        .turn(|t| t.text("never"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let policy = RetryPolicy {
            max_attempts: 3,
            base: Duration::from_secs(3600),
            max_delay: Duration::from_secs(3600),
        };
        let mut run = Agent::new(llm.clone()).retry(policy).start("go", &store);
        {
            let mut events = run.events();
            loop {
                match events.next().await {
                    Some(RunEvent::Retry { .. }) => break,
                    Some(_) => {}
                    None => panic!("the run never retried"),
                }
            }
        }
        run.cancel();
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.stop, StopReason::Cancelled);
        assert_eq!(llm.requests().len(), 1);
    });
}
