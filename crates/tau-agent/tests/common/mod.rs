//! Helpers shared by the tests that drive whole runs.

#![allow(dead_code)]

use tau_agent::event::RunEvent;
use tau_ai::message::Message;
use tau_store::{Entry, Store};

/// The run's stored transcript, as messages.
pub async fn stored(store: &Store, run: &str) -> Vec<Message> {
    store
        .transcript(run)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| match entry {
            Entry::Message { body, .. } => {
                serde_json::from_value(body).unwrap()
            }
            Entry::Compaction { .. } => panic!("no compaction in these runs"),
        })
        .collect()
}

/// Checks `RunStart (TurnStart … TurnEnd)* RunEnd` with increasing turn
/// numbers, and that tool events sit inside turns.
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
            _ => assert!(in_turn.is_some(), "{event:?} outside a turn"),
        }
    }
    assert!(in_turn.is_none(), "a turn never ended");
}
