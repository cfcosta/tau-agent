//! Helpers shared by the tests that drive whole runs.

#![allow(dead_code)]

use std::time::Duration;

use tau_agent::{
    event::{LimitKind, RunEvent},
    limits::Limits,
};
use tau_ai::message::{Message, Usage};
use tau_store::{Entry, Store};

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
