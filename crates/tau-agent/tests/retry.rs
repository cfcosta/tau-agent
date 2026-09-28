//! Retries of failed responses inside the loop
//! (`docs/reference/agent-loop.md`, "Retries"), driven through `Agent`
//! with `ScriptedModel`, `Store::memory()` and paused time.

use std::time::Duration;

use futures_util::StreamExt;
use hegel::{TestCase, generators as gs};
use tau_agent::{
    agent::Agent,
    compaction::Compaction,
    event::{RunEvent, StopReason},
};
use tau_ai::{message::Message, retry::RetryPolicy};
use tau_store::Store;
use tau_testing::{
    block_on,
    scripted::{ScriptedModel, TurnBuilder},
};

mod common;
use common::stored;

/// A retryable failure, as the scripts draw it.
#[derive(Debug, Clone, Copy)]
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

/// Retries over drawn failure runs, against a model: a turn fails `k`
/// times in a row in retryable ways, then succeeds, under a policy of
/// `max` attempts. If `k < max` the run recovers: the model is asked
/// `k + 1` times, a `Retry` event announces each new attempt with a
/// delay within the policy's cap, and none of the failed responses is
/// stored. Otherwise the run fails after exactly `max` requests with the
/// last failure.
#[hegel::test(test_cases = 60)]
fn retryable_failures_are_retried_within_the_policy(tc: TestCase) {
    let kinds = gs::sampled_from(vec![
        Retryable::ServerError,
        Retryable::RateLimited,
        Retryable::NeverStarted,
    ]);
    let failures: Vec<Retryable> = tc.draw(gs::vecs(kinds).max_size(4));
    let policy = RetryPolicy {
        max_attempts: tc.draw(gs::integers::<u32>().min_value(1).max_value(4)),
        base: Duration::from_secs(2),
        max_delay: Duration::from_secs(5),
    };
    let mut llm = ScriptedModel::new();
    for &kind in &failures {
        llm = llm.turn(move |t| fail(t, kind));
    }
    llm = llm.turn(|t| t.text("done"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let mut run = Agent::new(llm.clone()).retry(policy).start("go", &store);
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();

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
            assert!(delay <= policy.delay(attempt - 1, 1.0), "{retries:?}");
        }
        let attempts: Vec<u32> = retries.iter().map(|(a, _)| *a).collect();
        let k = failures.len() as u32;
        if k < policy.max_attempts {
            assert_eq!(outcome.stop, StopReason::Stop);
            assert_eq!(outcome.text, "done");
            assert_eq!(llm.requests().len() as u32, k + 1);
            assert_eq!(attempts, (2..=k + 1).collect::<Vec<_>>());
            let transcript = stored(&store, &outcome.run.0).await;
            assert_eq!(transcript.len(), 2, "{transcript:?}");
            assert!(matches!(transcript[1], Message::Assistant(_)));
        } else {
            assert!(
                matches!(outcome.stop, StopReason::Error(_)),
                "{:?}",
                outcome.stop
            );
            assert_eq!(llm.requests().len() as u32, policy.max_attempts);
            assert_eq!(attempts, (2..=policy.max_attempts).collect::<Vec<_>>());
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

/// A context overflow reported by its code (not its wording) compacts,
/// and the summary request goes through the retry policy too.
#[test]
fn an_overflow_code_compacts_and_the_summary_is_retried() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("first"))
        .turn(|t| t.error("context_length_exceeded", "no room"))
        .turn(|t| t.error("server_error", "try again"))
        .turn(|t| t.text("summary"))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm.clone()).compaction(
            Compaction::default()
                .context_window(u64::MAX)
                .keep_recent_tokens(1),
        );
        let mut run = agent.start("go", &store);
        run.steer("more");
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.text, "done");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunEvent::ContextRewritten { .. }))
        );
        llm.assert_exhausted();
    });
}

/// A summary request that keeps failing retryably is tried as many times
/// as the policy allows, and then compaction fails.
#[test]
fn a_failing_summary_is_tried_as_the_policy_allows() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("first"))
        .turn(|t| t.error("context_length_exceeded", "no room"))
        .turn(|t| t.error("server_error", "try again"))
        .turn(|t| t.error("server_error", "try again"))
        .turn(|t| t.text("never"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let policy = RetryPolicy {
            max_attempts: 2,
            ..RetryPolicy::default()
        };
        let agent = Agent::new(llm.clone()).retry(policy).compaction(
            Compaction::default()
                .context_window(u64::MAX)
                .keep_recent_tokens(1),
        );
        let run = agent.start("go", &store);
        run.steer("more");
        let outcome = run.outcome().await.unwrap();
        let StopReason::Error(message) = &outcome.stop else {
            panic!("{:?}", outcome.stop)
        };
        assert!(message.contains("compaction failed"), "{message}");
        assert_eq!(llm.requests().len(), 4);
        assert_eq!(llm.remaining(), 1);
    });
}
