//! tau-goal's UI: what its fold makes of its records, what it says as a
//! run starts, and that it keeps to the design language.

use std::{collections::BTreeSet, sync::Arc};

use hegel::generators::{self as gs, Generator as _};
use serde_json::{Value, json};
use tau_goal::{
    Check,
    Exhausted,
    Goal,
    NAME,
    Record,
    ui::{GoalUi, State},
};
use tau_ui_plugin::{
    CardInfo,
    RepoCtx,
    RunCtx,
    RunCx,
    RunKind,
    Services,
    UiPlugin,
};

/// The anchors a fold places, in order.
#[derive(Default)]
struct Anchors(Vec<String>);

impl RunCx for Anchors {
    fn transcript(&mut self, key: &str) {
        self.0.push(key.to_owned());
    }

    fn attach(&mut self, _: &str, _: &str) -> bool {
        false
    }

    fn dropped(&mut self, _: &str, _: tau_ui_plugin::Dropped) -> bool {
        false
    }

    fn cut(&mut self, _: &str, _: tau_ui_plugin::OutputCut) -> bool {
        false
    }

    fn mark(&mut self, _: &str, _: tau_ui_plugin::CardMark) -> bool {
        false
    }

    fn rewrite(&mut self, _: &str) {}

    fn cards(&self) -> Vec<CardInfo> {
        Vec::new()
    }

    fn last_text(&self) -> Option<String> {
        None
    }

    fn turn(&self) -> u32 {
        0
    }
}

/// One of tau-goal's records, or what it says as a run starts.
#[hegel::composite]
fn body(tc: &hegel::TestCase) -> Value {
    let n: u32 = tc.draw(gs::integers().min_value(1_u32).max_value(9));
    match tc.draw(gs::integers().min_value(0_u8).max_value(8)) {
        0 => Record::Set {
            goal: tc
                .draw(gs::sampled_from(vec!["it builds", "tests pass"]))
                .into(),
            continuations: n,
            budget: 1.0,
        }
        .to_value(),
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
        })
        .to_value(),
        2 => Record::Stopped {
            why: if tc.draw(gs::booleans()) {
                Exhausted::Continuations
            } else {
                Exhausted::Budget
            },
        }
        .to_value(),
        3 => Record::Extended { by: n }.to_value(),
        4 => Record::Paused.to_value(),
        5 => Record::Resumed.to_value(),
        6 => Record::Cleared.to_value(),
        7 => Record::Error {
            message: "offline".into(),
        }
        .to_value(),
        _ => json!({ "kind": "starting", "checks": tc.draw(gs::booleans()) }),
    }
}

/// Over any records, the fold's goal is the goal the records leave; a
/// note shows for each check, stop and error, and only for those; the
/// continuations a check sent are held; what it says as a run starts
/// sets whether it checks, and nothing else.
#[hegel::test(test_cases = 300)]
fn the_fold_follows_the_records(tc: hegel::TestCase) {
    let bodies: Vec<Value> = tc.draw(gs::vecs(body()).max_size(12));
    let mut state = State::default();
    let mut anchors = Anchors::default();
    for body in &bodies {
        state.apply(body, &mut anchors);
    }
    assert_eq!(state.goal, Goal::fold(&bodies));
    let records: Vec<Record> =
        bodies.iter().filter_map(Record::parse).collect();
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
    assert_eq!(anchors.0.len(), noted);
    let keys: BTreeSet<&String> = anchors.0.iter().collect();
    assert_eq!(keys.len(), noted, "each note has a key of its own");
    assert!(anchors.0.iter().all(|key| state.notes.contains_key(key)));
    let held: BTreeSet<u32> = records
        .iter()
        .filter_map(|record| match record {
            Record::Check(check) => check.continuation,
            _ => None,
        })
        .collect();
    assert_eq!(state.held, held);
    let checks = bodies
        .iter()
        .rev()
        .find(|body| body["kind"] == "starting")
        .is_some_and(|body| body["checks"] == true);
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
        kind,
        repo: RepoCtx {
            name: "repo".into(),
            checkout: "/tmp/repo".into(),
            dir: "/tmp/tau/repo".into(),
        },
        model: "gpt-5.5".into(),
        effort: None,
        services,
    }
}

/// tau-goal checks a run, and builds its agent plugin, only with a key
/// and not in a sub-agent; as a run starts it says which.
#[hegel::test(test_cases = 50)]
fn it_says_whether_it_checks(tc: hegel::TestCase) {
    let jev = tc.draw(gs::booleans());
    let kind = tc.draw(
        gs::sampled_from(vec![RunKind::Main, RunKind::Chat, RunKind::SubAgent])
            .print_as_debug(),
    );
    let run = run(kind, jev);
    let on = jev && kind != RunKind::SubAgent;
    let mut state = State::default();
    for body in GoalUi.starting(&(), &run, &()) {
        state.apply(&body, &mut Anchors::default());
    }
    assert_eq!(state.checks, on);
    let plugins = GoalUi.agent_plugins(&(), &run, &()).unwrap();
    assert_eq!(plugins.len(), usize::from(on));
    assert!(plugins.iter().all(|plugin| plugin.name() == NAME));
}

/// The UI takes its look from the kit.
#[test]
fn only_the_kit_holds_design_values() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = tau_ui_kit::design::check(&src, &[]);
    assert!(found.is_empty(), "{}", found.join("\n"));
}
