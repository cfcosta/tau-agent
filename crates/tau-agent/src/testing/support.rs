//! Checks shared by the tests that drive whole runs, a loop's
//! ([`crate::runner::driver::Loop`]) among them.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tau_ai::message::{AssistantBlock, Message, StopReason, Usage};
use tau_store::{Entry, Store};

use crate::{
    event::{LimitKind, RunEvent},
    limits::Limits,
};

/// The run's stored transcript, as messages.
pub async fn stored(store: &Store, run: &str) -> Vec<Message> {
    store
        .transcript(run)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| match entry {
            Entry::Message { body, .. } => serde_json::from_str(&body).unwrap(),
            other => panic!("only messages in these runs: {other:?}"),
        })
        .collect()
}

/// Checks `RunStart (TurnStart … TurnEnd)* RunEnd` with increasing turn
/// numbers, that tool events sit inside turns, and that continuations
/// sit between them.
pub fn assert_grammar(events: &[RunEvent]) {
    assert!(
        matches!(events.first(), Some(RunEvent::RunStart { .. })),
        "{events:?}"
    );
    assert!(
        matches!(events.last(), Some(RunEvent::RunEnd { .. })),
        "{events:?}"
    );
    let mut in_turn = None;
    let mut last_turn = 0;
    for event in &events[1..events.len() - 1] {
        match event {
            RunEvent::RunStart { .. } | RunEvent::RunEnd { .. } => {
                panic!("nested run event")
            }
            RunEvent::TurnStart { turn, .. } => {
                assert!(in_turn.is_none(), "turn started inside a turn");
                assert_eq!(*turn, last_turn + 1);
                in_turn = Some(*turn);
            }
            RunEvent::TurnEnd { turn, .. } => {
                assert_eq!(in_turn, Some(*turn), "turn ended out of order");
                last_turn = *turn;
                in_turn = None;
            }
            // A continuation or a steer comes between turns. A rewrite comes between
            // turns, or inside the turn whose context overflowed; a plugin
            // can fail anywhere.
            RunEvent::Continued { .. } | RunEvent::Steered { .. } => {
                assert!(in_turn.is_none(), "{event:?} inside a turn")
            }
            RunEvent::ContextRewritten { .. }
            | RunEvent::PluginCharged { .. }
            | RunEvent::PluginReport { .. }
            | RunEvent::PluginError { .. } => {}
            _ => assert!(in_turn.is_some(), "{event:?} outside a turn"),
        }
    }
    assert!(in_turn.is_none(), "a turn never ended");
}

/// The reference: each limit, checked in order turns, tokens, cost, time,
/// reached when the value meets or passes it.
pub fn reference(
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

/// A counter clock, so timestamps are deterministic.
pub fn counter_clock() -> crate::runner::Clock {
    let next = Arc::new(Mutex::new(0u64));
    Arc::new(move || {
        let mut n = next.lock().unwrap();
        *n += 1;
        *n
    })
}

/// Every tool call in the transcript has exactly one result, right after
/// its assistant message and in source order. A failed or aborted
/// message's calls never ran, and the request leaves that message out
/// (`tau_ai::responses::input`, rule 3): nothing answers them.
pub fn assert_results_follow_calls(transcript: &[Message]) {
    for (i, message) in transcript.iter().enumerate() {
        let Message::Assistant(assistant) = message else {
            if let Message::ToolResult(result) = message {
                let after_call = transcript[..i]
                    .iter()
                    .rev()
                    .find(|m| !matches!(m, Message::ToolResult(_)))
                    .is_some_and(|m| matches!(m, Message::Assistant(_)));
                assert!(
                    after_call,
                    "result {} at {i} follows no assistant message",
                    result.tool_call_id
                );
            }
            continue;
        };
        let failed = matches!(
            assistant.stop_reason,
            StopReason::Error | StopReason::Aborted
        );
        let calls: Vec<&str> = assistant
            .content
            .iter()
            .filter_map(|b| match b {
                AssistantBlock::ToolCall(c) if !failed => Some(c.id.as_str()),
                _ => None,
            })
            .collect();
        let results: Vec<&str> = transcript[i + 1..]
            .iter()
            .take_while(|m| matches!(m, Message::ToolResult(_)))
            .map(|m| match m {
                Message::ToolResult(r) => r.tool_call_id.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(results, calls, "results after message {i}");
    }
}
