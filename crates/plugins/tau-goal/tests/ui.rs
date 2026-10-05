//! tau-goal's UI: what its fold makes of its records, what it says as a
//! run starts, and that it keeps to the design language.

use std::{collections::BTreeSet, sync::Arc};

use hegel::generators::{self as gs, Generator as _};
use serde_json::Value;
use tau_goal::{
    Check,
    Exhausted,
    Goal,
    NAME,
    Record,
    ui::{GoalUi, State},
};
use tau_ui_plugin::{
    Fold as _,
    RunCtx,
    RunKind,
    Services,
    UiPlugin,
    testing::{FakeRun, run_ctx},
};

/// One of tau-goal's records, or what it says as a run starts.
#[hegel::composite]
fn record(tc: &hegel::TestCase) -> Record {
    let n: u32 = tc.draw(gs::integers().min_value(1_u32).max_value(9));
    match tc.draw(gs::integers().min_value(0_u8).max_value(8)) {
        0 => Record::Set {
            goal: tc
                .draw(gs::sampled_from(vec!["it builds", "tests pass"]))
                .into(),
            continuations: n,
            budget: 1.0,
        },
        1 => Record::Check(Check {
            n,
            met: tc.draw(gs::booleans()),
            p: 0.5,
            turn: n,
            continuation: tc.draw(gs::optional(
                gs::integers().min_value(1_u32).max_value(9),
            )),
            cost: 0.0,
            spent: 0.0,
        }),
        2 => Record::Stopped {
            why: if tc.draw(gs::booleans()) {
                Exhausted::Continuations
            } else {
                Exhausted::Budget
            },
        },
        3 => Record::Extended { by: n },
        4 => Record::Paused,
        5 => Record::Resumed,
        6 => Record::Cleared,
        7 => Record::Error {
            message: "offline".into(),
        },
        _ => Record::Starting {
            checks: tc.draw(gs::booleans()),
        },
    }
}

/// Over any records, the fold's goal is the goal the records leave; a
/// note shows for each check, stop and error, and only for those; the
/// continuations a check sent are held; what it says as a run starts
/// sets whether it checks, and nothing else.
#[hegel::test(test_cases = 300)]
fn the_fold_follows_the_records(tc: hegel::TestCase) {
    let records: Vec<Record> =
        tc.draw(gs::vecs(record().print_as_debug()).max_size(12));
    let mut state = State::default();
    let mut anchors = FakeRun::default();
    for record in &records {
        state.apply(record.clone(), &mut anchors);
    }
    let bodies: Vec<Value> = records
        .iter()
        .map(|record| serde_json::to_value(record).unwrap())
        .collect();
    assert_eq!(state.goal, Goal::fold(&bodies));
    let noted = records
        .iter()
        .filter(|record| {
            matches!(
                record,
                Record::Check(_)
                    | Record::Stopped { .. }
                    | Record::Error { .. }
            )
        })
        .count();
    assert_eq!(anchors.anchors.len(), noted);
    let keys: BTreeSet<&String> = anchors.anchors.iter().collect();
    assert_eq!(keys.len(), noted, "each note has a key of its own");
    assert!(
        anchors
            .anchors
            .iter()
            .all(|key| state.notes.contains_key(key))
    );
    let held: BTreeSet<u32> = records
        .iter()
        .filter_map(|record| match record {
            Record::Check(check) => check.continuation,
            _ => None,
        })
        .collect();
    assert_eq!(state.held, held);
    let checks = records.iter().rev().find_map(|record| match record {
        Record::Starting { checks } => Some(*checks),
        _ => None,
    }) == Some(true);
    assert_eq!(state.checks, checks);
}

fn run(kind: RunKind, jev: bool) -> RunCtx {
    let mut services = Services::default();
    if jev {
        let jev: Arc<dyn tau_jev::Jev> =
            Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.5));
        services = services.with(jev);
    }
    RunCtx {
        services,
        ..run_ctx(kind)
    }
}

/// Property inventory: Every key/run-kind pair follows the starting-check and
/// agent-plugin contract. The oracle is key presence outside SubAgent plus
/// the expected plugin count/name; inputs are the fixed 2 × 3 Cartesian table,
/// with no rejection or shrinking so every contract case always runs.
#[test]
fn starting_checks_and_plugin_presence_follow_key_and_run_kind() {
    for jev in [false, true] {
        for kind in [RunKind::Main, RunKind::Chat, RunKind::SubAgent] {
            let run = run(kind, jev);
            let should_check = jev && kind != RunKind::SubAgent;
            let mut state = State::default();
            let mut anchors = FakeRun::default();
            for record in tau_testing::block_on(GoalUi.starting(&(), &run, &()))
            {
                state.apply(record, &mut anchors);
            }
            assert_eq!(state.checks, should_check);

            let plugins =
                tau_testing::block_on(GoalUi.agent_plugins(&(), &run, &()))
                    .unwrap();
            assert_eq!(plugins.len(), usize::from(should_check));
            assert!(plugins.iter().all(|plugin| plugin.name() == NAME));
        }
    }
}
