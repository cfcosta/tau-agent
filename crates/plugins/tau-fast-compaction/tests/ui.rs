//! fast-compaction's UI: what its fold makes of its passes and cut
//! outputs, what it says as a run starts, and that it keeps to the
//! design language.

use std::sync::Arc;

use hegel::generators as gs;
use serde_json::{Value, json};
use tau_fast_compaction::{
    NAME,
    ui::{Decision, FastCompactionUi, State},
};
use tau_ui_plugin::{
    CardInfo,
    Dropped,
    Fold as _,
    REWRITE,
    RunCtx,
    RunKind,
    Services,
    UiPlugin,
    testing::{FakeRun, fold, run_ctx},
};

/// A run showing `n` reads, `c0` and on, one a turn.
fn with_calls(n: usize) -> FakeRun {
    FakeRun {
        cards: (0..n)
            .map(|i| CardInfo {
                call_id: format!("c{i}"),
                tool: "read".into(),
                args: json!({ "path": format!("f{i}.rs") }),
                summary: format!("f{i}.rs"),
                size: 400,
                turn: i as u32 + 1,
            })
            .collect(),
        turn: n as u32,
        ..FakeRun::default()
    }
}

/// A pass's details: an action for some of the calls, `c0` and on, the
/// rest too recent to judge.
fn details(actions: &[&str]) -> Value {
    let decisions: Vec<Value> = actions
        .iter()
        .enumerate()
        .map(|(i, action)| {
            json!({
                "call_id": format!("c{i}"), "tool": "read", "action": action,
                "keep_call": 0.5, "keep_result": 0.5,
            })
        })
        .collect();
    json!({
        "decisions": decisions,
        "stats": {
            "calls": actions.len(), "pinned": 0, "kept": 0,
            "results_dropped": 0, "calls_dropped": 0, "requests": 1,
            "state_tokens": 100, "state_stage": "whole",
            "chars_before": 4000, "chars_after": 2000,
            "reduction_ratio": 0.5,
        },
    })
}

/// Over any passes, live or from history, each names its rewrite once;
/// the ledger is the last pass's, a line for each card and for each
/// judged call no card is left for; a call it drops is marked on its
/// card, and every card it judged carries its mark.
#[hegel::test(test_cases = 200)]
fn passes_mark_what_they_drop(tc: hegel::TestCase) {
    let cards: usize = tc.draw(gs::integers().max_value(6));
    let passes: Vec<(Vec<&str>, bool)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::vecs(gs::sampled_from(vec![
                "keep",
                "drop_result",
                "drop_call"
            ]))
            .max_size(8),
            gs::booleans(),
        ))
        .max_size(4),
    );
    let mut run = with_calls(cards);
    let mut state = State::default();
    for (actions, stored) in &passes {
        let body = if *stored {
            json!({ REWRITE: details(actions) })
        } else {
            let mut body = details(actions);
            body["kind"] = "ledger".into();
            body
        };
        fold(FastCompactionUi, &mut state, &body, &mut run);
    }
    assert_eq!(run.rewrites.len(), passes.len());
    assert_eq!(state.passes.len(), passes.len());
    assert_eq!(state.last.as_ref(), run.rewrites.last());
    let Some((actions, _)) = passes.last() else {
        assert!(state.ledger.is_empty());
        return;
    };
    let gone = actions.len().saturating_sub(cards);
    assert_eq!(state.ledger.len(), cards + gone);
    for (i, action) in actions.iter().enumerate() {
        let call = format!("c{i}");
        let entry = state.entry(&call).unwrap();
        let (decision, dropped) = match *action {
            "keep" => (Decision::Keep, None),
            "drop_result" => (Decision::DropResult, Some(Dropped::Result)),
            _ => (Decision::DropCall, Some(Dropped::Call)),
        };
        assert_eq!(entry.decision, decision);
        if dropped.is_some() {
            assert_eq!(run.dropped.get(&call).copied(), dropped);
        }
    }
    for i in actions.len()..cards {
        let entry = state.entry(&format!("c{i}")).unwrap();
        assert_eq!(entry.decision, Decision::Pinned);
    }
    assert!(
        state.ledger.iter().all(|entry| run
            .attached
            .iter()
            .any(|(id, _)| *id == entry.call_id))
    );
}

/// An output cut as it arrived goes on its card and counts toward what
/// was saved; one Jev left whole changes nothing.
#[test]
fn a_cut_output_goes_on_its_card() {
    let mut run = with_calls(1);
    let mut state = State::default();
    let output = |pruned: bool| {
        json!({
            "kind": "output", "call_id": "c0", "lines": 4810, "chunks": 200,
            "kept": 212, "dropped_lines": 4598, "segments": 1, "requests": 2,
            "tokens_before": 12_000, "tokens_after": 900, "pruned": pruned,
            "archive": "/tmp/archive/out.txt",
        })
    };
    fold(FastCompactionUi, &mut state, &output(false), &mut run);
    assert!(run.cut.is_empty());
    fold(FastCompactionUi, &mut state, &output(true), &mut run);
    let cut = &run.cut["c0"];
    assert_eq!(cut.label(), "kept 212 of 4,810 lines · 12k → 900 tokens");
    assert_eq!((state.outputs, state.outputs_saved), (1, 11_100));
}

fn run(jev: bool) -> RunCtx {
    let mut services = Services::default();
    if jev {
        let jev: Arc<dyn tau_jev::Jev> =
            Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.5));
        services = services.with(jev);
    }
    let _dir = std::env::temp_dir().join("tau-fast-compaction-ui");
    RunCtx {
        services,
        ..run_ctx(RunKind::Chat)
    }
}

/// It prunes only with a key; as a run starts it says which, and where
/// it steps in on the context meter.
/// Finite inventory: absent and present Jev each exercise every run-start
/// assertion.
#[test]
fn it_says_whether_it_is_on() {
    for jev in [false, true] {
        let run = run(jev);
        let mut state = State::default();
        for body in
            tau_testing::block_on(FastCompactionUi.starting(&(), &run, &()))
        {
            state.apply(body, &mut FakeRun::default());
        }
        assert_eq!(state.on, Some(jev));
        let status = state.status().unwrap();
        assert_eq!(status == tau_ui_plugin::NO_KEY, !jev, "{status}");
        let plugins = tau_testing::block_on(FastCompactionUi.agent_plugins(
            &(),
            &run,
            &(),
        ))
        .unwrap();
        assert_eq!(plugins.len(), usize::from(jev));
        assert!(plugins.iter().all(|plugin| plugin.name() == NAME));
        assert!(FastCompactionUi.rewrites_keep_transcript());
    }
}

/// Finite inventory: all four decisions must match their exact card label.
/// The literal table is the independent oracle; enumeration needs no
/// generated inputs or shrinking.
#[test]
fn each_decision_has_its_exact_card_label() {
    let cases = [
        (Decision::Pinned, "kept"),
        (Decision::Keep, "kept"),
        (Decision::DropResult, "result dropped"),
        (Decision::DropCall, "call dropped"),
    ];
    for (decision, expected) in cases {
        assert_eq!(decision.card_label(), expected, "{decision:?}");
    }
}
