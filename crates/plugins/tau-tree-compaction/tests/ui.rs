//! Tree compaction's UI: off until switched on, what it says as a run
//! starts, and what its fold makes of its compactions.

use tau_tree_compaction::{
    NAME,
    Record,
    ui::{Settings, State, TreeCompactionHost},
};
use tau_ui_plugin::{
    Fold as _,
    HostHalf as _,
    RunKind,
    testing::{FakeRun, run_ctx},
};

fn compacted(entries: usize, lines: usize) -> Record {
    Record::Compacted {
        messages: 3,
        entries,
        lines,
        bytes: 900,
        asked: 1,
        tokens_before: 5_000,
    }
}

/// Finite inventory: off and on. Off, a run gets no plugin and the list
/// says so; on, it gets the plugin, watching the window.
#[test]
fn it_runs_only_when_switched_on() {
    let run = run_ctx(RunKind::Chat);
    for on in [false, true] {
        let settings = Settings { on };
        let mut state = State::default();
        for record in tau_testing::block_on(TreeCompactionHost.starting(
            &(),
            &run,
            &settings,
        )) {
            state.apply(record, &mut FakeRun::default());
        }
        assert_eq!(state.on, Some(on));
        assert_eq!(
            state.status(),
            if on { "watching the window" } else { "off" }
        );
        let plugins = tau_testing::block_on(TreeCompactionHost.agent_plugins(
            &(),
            &run,
            &settings,
        ))
        .unwrap();
        assert_eq!(plugins.len(), usize::from(on));
        assert!(plugins.iter().all(|plugin| plugin.name() == NAME));
    }
    // Off unless the user said otherwise.
    assert_eq!(Settings::default(), Settings { on: false });
    assert_eq!(
        serde_json::from_str::<Settings>("{}").unwrap(),
        Settings { on: false }
    );
}

/// Each compaction counts, and the list says how much the history holds
/// as of the last; what was folded is not the fold's business.
#[test]
fn the_list_says_what_the_last_compaction_left() {
    let mut state = State::default();
    let mut run = FakeRun::default();
    state.apply(Record::Starting { on: true }, &mut run);
    state.apply(compacted(40, 12), &mut run);
    state.apply(
        Record::Folded {
            first: 0,
            entries: Vec::new(),
            nodes: Vec::new(),
        },
        &mut run,
    );
    state.apply(compacted(90, 20), &mut run);
    assert_eq!(state.compactions, 2);
    assert_eq!(state.status(), "90 messages in 20 lines");
    // Records round-trip as the store keeps them.
    let record = compacted(1, 1);
    let body = serde_json::to_value(&record).unwrap();
    assert_eq!(body["kind"], "compacted");
    assert_eq!(serde_json::from_value::<Record>(body).unwrap(), record);
}
