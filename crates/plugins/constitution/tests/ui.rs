//! tau-constitution's UI: what its fold makes of the checks, the editor
//! that writes and tries a rule, the review queue, the page's totals,
//! and that it keeps to the design language.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use gpui::{App, AppContext as _, Entity, TestAppContext};
use hegel::generators as gs;
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_constitution::{
    NAME,
    ui::{
        Act,
        ConstitutionUi,
        Data,
        RuleInfo,
        Rules,
        State,
        Stats,
        page::{self, HandledKind, Mark, Preset, Trying, Ui},
        stats::RulesStats,
    },
};
use tau_ui_plugin::{
    CardInfo,
    CardMark,
    Handle,
    Request,
    RunInfo,
    UiPlugin,
    ViewCx,
    testing::FakeRun,
};

fn card(call_id: &str, tool: &str, summary: &str) -> CardInfo {
    CardInfo {
        call_id: call_id.into(),
        tool: tool.into(),
        args: json!({ "command": summary }),
        summary: summary.into(),
        size: 40,
        turn: 1,
    }
}

/// One of tau-constitution's records: a check of a call or the answer,
/// a verdict or a failure.
#[hegel::composite]
fn record(tc: &hegel::TestCase) -> Value {
    let rule = tc.draw(gs::sampled_from(vec!["R1", "R2", "R3"]));
    let call = tc.draw(gs::booleans());
    match tc.draw(gs::integers::<u8>().max_value(4)) {
        0 => json!({
            "kind": "checked",
            "call_id": call.then_some("c1"),
            "scores": [{"rule": rule, "score": 0.5}],
            // Multiples of 1/1024, so sums are exact.
            "cost": f64::from(tc.draw(gs::integers::<u32>().max_value(64)))
                / 1024.0,
        }),
        1 => json!({"kind": "blocked", "rule": rule, "text": "", "score": 0.9}),
        2 => json!({"kind": "flagged", "rule": rule, "text": "", "score": 0.5}),
        3 => json!({
            "kind": "held", "rule": rule, "text": "", "score": 0.9,
            "hold": 1, "max_holds": 3,
        }),
        _ => json!({"kind": "error", "message": "offline"}),
    }
}

/// Which runs are loaded does not change what the page counts: a stored
/// run counts the same as when it is loaded with the same records, and
/// once whichever way.
#[hegel::test(test_cases = 200)]
fn loading_a_run_does_not_change_the_totals(tc: hegel::TestCase) {
    let runs: Vec<(String, Stats)> = (0..tc
        .draw(gs::integers::<usize>().max_value(5)))
        .map(|n| {
            let mut stats = Stats::default();
            for body in tc.draw(gs::vecs(record()).max_size(6)) {
                stats.add(&body, None);
            }
            (format!("run-{n}"), stats)
        })
        .collect();
    let loaded: Vec<bool> =
        runs.iter().map(|_| tc.draw(gs::booleans())).collect();
    let some: Vec<(&str, &Stats)> = runs
        .iter()
        .zip(&loaded)
        .filter(|(_, loaded)| **loaded)
        .map(|((run, stats), _)| (run.as_str(), stats))
        .collect();
    let all: Vec<(&str, &Stats)> = runs
        .iter()
        .map(|(run, stats)| (run.as_str(), stats))
        .collect();
    let stored = RulesStats::of(&[], &runs);
    assert_eq!(RulesStats::of(&some, &runs), stored);
    assert_eq!(RulesStats::of(&all, &[]), stored);
    assert_eq!(stored.runs, runs.len());
}

/// Over any records, the fold counts them as the run's stats do, marks
/// each call a verdict is about on its card, and notes each failure and
/// each verdict on the answer.
#[hegel::test(test_cases = 200)]
fn the_fold_marks_what_the_checks_decided(tc: hegel::TestCase) {
    let bodies: Vec<Value> = tc.draw(gs::vecs(record()).max_size(8));
    let mut run = FakeRun {
        cards: vec![card("c1", "bash", "rm -rf /")],
        turn: 1,
        ..FakeRun::default()
    };
    let mut state = State::default();
    let mut stats = Stats::default();
    for body in &bodies {
        state.apply(body, &mut run);
        stats.add(body, None);
    }
    assert_eq!(state.stats, stats);
    // Verdicts here are all on the answer; failures and verdicts each
    // get a note.
    let noted = bodies
        .iter()
        .filter(|body| body["kind"] != "checked")
        .count();
    assert_eq!(run.anchors.len(), noted);
    assert_eq!(state.notes.len(), noted);
}

/// A verdict on a call marks its card: refused with the reason the model
/// got, or flagged; the call's scores show on it either way.
#[test]
fn a_verdict_marks_its_call() {
    let mut run = FakeRun {
        cards: vec![
            card("c1", "bash", "rm -rf /"),
            card("c2", "write", "a.rs"),
        ],
        turn: 1,
        ..FakeRun::default()
    };
    let mut state = State::default();
    for body in [
        json!({"kind": "blocked", "rule": "R1", "text": "No deletes.", "score": 0.95,
               "call_id": "c1", "tool": "bash", "reason": "Rule R1 forbids it"}),
        json!({"kind": "flagged", "rule": "R2", "text": "No unwrap.", "score": 0.55,
               "call_id": "c2", "tool": "write"}),
        json!({"kind": "checked", "call_id": "c2", "tool": "write",
               "scores": [{"rule": "R2", "score": 0.55}], "cost": 0.0}),
    ] {
        state.apply(&body, &mut run);
    }
    assert_eq!(
        run.marks["c1"],
        CardMark::Blocked {
            reason: "Rule R1 forbids it".into()
        }
    );
    assert_eq!(run.marks["c2"], CardMark::Flagged);
    assert_eq!(state.calls["c2"].scores, [("R2".to_owned(), 0.55)]);
    assert_eq!(state.calls["c1"].shown, "rm -rf /");
    let flags = state.flags();
    assert_eq!(flags.len(), 1);
    assert_eq!(
        (flags[0].key.as_str(), flags[0].rule.as_str()),
        ("c2", "R2")
    );
}

/// A step never takes the answers sent back below none, or past the
/// most the page allows.
#[hegel::test(test_cases = 200)]
fn holds_stay_in_bounds(tc: hegel::TestCase) {
    let holds = tc.draw(gs::integers::<u32>().max_value(page::MAX_HOLDS));
    let delta = tc.draw(gs::integers::<i32>().min_value(-20).max_value(20));
    let next = page::next_holds(holds, delta);
    assert!(next <= page::MAX_HOLDS);
    let wanted = i64::from(holds) + i64::from(delta);
    assert_eq!(i64::from(next), wanted.clamp(0, i64::from(page::MAX_HOLDS)));
}

/// What the UI asked of the interface, in order.
type Asked = Rc<RefCell<Vec<Request>>>;

/// The page's state in a window, with what it asks kept.
fn ui(cx: &mut TestAppContext) -> (Entity<Ui>, Asked) {
    let asked: Asked = Rc::default();
    let sink = asked.clone();
    let handle = Handle::new(
        NAME,
        Rc::new(move |_, request, _: &mut App| sink.borrow_mut().push(request)),
    );
    let ui = cx.update(|cx| cx.new(|cx| ConstitutionUi.new_ui(handle, cx)));
    (ui, asked)
}

/// What the UI asked its host half to do, in order.
fn acts(asked: &Asked) -> Vec<Act> {
    asked
        .borrow()
        .iter()
        .filter_map(|request| match request {
            Request::Act(action) => serde_json::from_value(action.clone()).ok(),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn rules_are_written_and_edited_in_the_editor(cx: &mut TestAppContext) {
    let (ui, asked) = ui(cx);
    ui.update(cx, |ui, cx| {
        ui.open_editor("docbert", None, cx);
        ui.set_rule_text("Never delete an index.", cx);
        // Nothing picked: nothing is sent, and the editor says why.
        ui.save(cx);
        assert_eq!(
            ui.problem(cx),
            Some("Pick at least one place, or Jev has nothing to check.")
        );
        assert!(ui.draft().unwrap().tried_to_save);
        // Places are picked, and any tool's field can be added by name.
        ui.toggle_place("bash.command", cx);
        ui.set_rule_on("not a field", cx);
        assert!(!ui.add_other_place(cx));
        ui.set_rule_on("grep.pattern", cx);
        assert!(ui.add_other_place(cx));
        // Presets, then a step: review never passes block.
        ui.set_preset(Preset::Strict, cx);
        assert_eq!(ui.draft().unwrap().preset(), Some(Preset::Strict));
        ui.nudge(Mark::Block, 0.05, cx);
        assert_eq!(ui.draft().unwrap().preset(), None);
        for _ in 0..20 {
            ui.nudge(Mark::Review, 0.05, cx);
        }
        let draft = ui.draft().unwrap();
        assert_eq!((draft.review, draft.block), (0.65, 0.65));
        ui.nudge(Mark::Review, -0.35, cx);
        ui.save(cx);
        assert!(ui.draft().is_none());
    });
    assert_eq!(
        acts(&asked),
        [Act::Add {
            repo: "docbert".into(),
            text: "Never delete an index.".into(),
            on: vec!["bash.command".into(), "grep.pattern".into()],
            review: 0.3,
            block: 0.65,
        }]
    );
    // Editing opens with the rule as it is, and saves over it.
    let rule = RuleInfo {
        id: "D4".into(),
        text: "Never delete an index.".into(),
        applies_to: vec!["bash.command".into(), "grep.pattern".into()],
        review: 0.3,
        block: 0.65,
    };
    ui.update(cx, |ui, cx| {
        ui.open_editor("docbert", Some(rule), cx);
        let draft = ui.draft().unwrap();
        assert_eq!(draft.editing.as_deref(), Some("D4"));
        assert_eq!(draft.places, ["bash.command", "grep.pattern"]);
        ui.toggle_place("grep.pattern", cx);
        ui.save(cx);
        ui.remove("docbert", "D4", cx);
    });
    assert_eq!(
        acts(&asked)[1..],
        [
            Act::Update {
                repo: "docbert".into(),
                id: "D4".into(),
                text: "Never delete an index.".into(),
                on: vec!["bash.command".into()],
                review: 0.3,
                block: 0.65,
            },
            Act::Remove {
                repo: "docbert".into(),
                id: "D4".into(),
            },
        ]
    );
}

#[gpui::test]
fn a_rule_is_tried_and_says_what_it_would_do(cx: &mut TestAppContext) {
    let (ui, asked) = ui(cx);
    let calls = vec![("bash".to_owned(), json!({ "command": "psql prod" }))];
    ui.update(cx, |ui, cx| {
        ui.open_editor("tau-agent", None, cx);
        ui.set_rule_text("Never touch the production database.", cx);
        ui.toggle_place("bash.command", cx);
        ui.try_rule(calls.clone(), Vec::new(), cx);
        assert_eq!(ui.draft().unwrap().trying, Trying::Asking);
    });
    let Some(Act::Try {
        calls: sent,
        answers,
        on,
        ..
    }) = acts(&asked).pop()
    else {
        panic!("no trial asked");
    };
    assert_eq!(
        (sent, answers, on),
        (calls, Vec::new(), vec!["bash.command".into()])
    );
    // What the host answers goes back to the editor.
    let reply = json!({ "Ok": [[{ "tool": "bash", "shown": "psql prod", "score": 0.9 }], 0.001] });
    ui.update(cx, |ui, cx| ConstitutionUi.reply(ui, reply, cx));
    ui.read_with(cx, |ui, _| {
        let Trying::Done { trials, .. } = &ui.draft().unwrap().trying else {
            panic!("not tried");
        };
        assert_eq!(trials.len(), 1);
    });
}

/// Unreadable rules are removed only once confirmed; settings are sent
/// as set.
#[gpui::test]
fn rules_are_reset_only_once_confirmed(cx: &mut TestAppContext) {
    let (ui, asked) = ui(cx);
    ui.update(cx, |ui, cx| {
        ui.ask_reset(Some("tau-agent"), cx);
        ui.ask_reset(None, cx);
        ui.ask_reset(Some("tau-agent"), cx);
    });
    assert!(acts(&asked).is_empty(), "asking is not removing");
    ui.update(cx, |ui, cx| {
        ui.reset("tau-agent", cx);
        ui.settings("tau-agent", true, 4, cx);
    });
    assert_eq!(
        acts(&asked),
        [
            Act::Reset {
                repo: "tau-agent".into()
            },
            Act::Settings {
                repo: "tau-agent".into(),
                blocks_unchecked: true,
                max_holds: 4,
            },
        ]
    );
}

/// The page over a repository's runs, each with what the checks did.
fn with_view<R>(
    cx: &mut TestAppContext,
    ui: &Entity<Ui>,
    runs: &[(RunInfo, State)],
    data: &Data,
    repos: &BTreeMap<String, Rules>,
    f: impl FnOnce(&mut ViewCx<'_, ConstitutionUi>) -> R,
) -> R {
    let runs: Vec<(RunInfo, Value)> = runs
        .iter()
        .map(|(run, state)| (run.clone(), serde_json::to_value(state).unwrap()))
        .collect();
    let list = move || runs.clone();
    let cards = |_: &RunId| Vec::new();
    let params = BTreeMap::new();
    let handle = Handle::new(NAME, Rc::new(|_, _, _: &mut App| {}));
    cx.update(|cx| {
        let mut view = ViewCx::new(
            &ConstitutionUi,
            ui.clone(),
            None,
            data,
            &(),
            repos,
            None,
            &params,
            false,
            true,
            1400.,
            handle,
            &list,
            &cards,
            cx,
        );
        f(&mut view)
    })
}

fn info(id: &str, repo: &str) -> RunInfo {
    RunInfo {
        id: RunId(id.into()),
        repo: repo.into(),
        live: false,
        title: format!("run {id}"),
        answer: None,
        context: 0,
        window: None,
    }
}

/// A flagged call waits for a person until marked fine, then shows as
/// handled; so does a flagged answer; a blocked call needs no one.
#[gpui::test]
fn what_waits_for_a_person_and_what_was_handled(cx: &mut TestAppContext) {
    let (ui, _) = ui(cx);
    let mut run = FakeRun {
        cards: vec![
            card("c1", "bash", "rm -rf /"),
            card("c2", "write", "a.rs"),
        ],
        last_text: Some("Done.".into()),
        turn: 1,
        ..FakeRun::default()
    };
    let mut state = State::default();
    for body in [
        json!({"kind": "checked", "call_id": "c1", "scores": [{"rule": "R1", "score": 0.95}], "cost": 0.001}),
        json!({"kind": "blocked", "rule": "R1", "text": "", "score": 0.95, "call_id": "c1"}),
        json!({"kind": "flagged", "rule": "R2", "text": "", "score": 0.55, "call_id": "c2"}),
        json!({"kind": "flagged", "rule": "R6", "text": "", "score": 0.5}),
    ] {
        state.apply(&body, &mut run);
    }
    let runs = vec![
        (info("a", "tau-agent"), state.clone()),
        // Another repository's run is not this one's to count.
        (info("b", "docbert"), state),
    ];
    let data = Data::default();
    let repos = BTreeMap::new();
    let (keys, stats) = with_view(cx, &ui, &runs, &data, &repos, |view| {
        let keys: Vec<String> = page::review_items(view, "tau-agent")
            .into_iter()
            .map(|item| item.flag.key)
            .collect();
        (keys, page::rules_stats(view, "tau-agent"))
    });
    assert_eq!(keys, ["c2", "answer-0"]);
    assert_eq!(
        (stats.runs, stats.blocked, stats.flagged, stats.waiting),
        (1, 1, 2, 2)
    );
    assert_eq!(stats.per_rule.get("R1"), Some(&(1, 0, 0)));
    // Looks fine: off the queue, and onto what was handled.
    ui.update(cx, |ui, cx| ui.reviewed("a", "answer-0", cx));
    let (keys, fine) = with_view(cx, &ui, &runs, &data, &repos, |view| {
        let keys: Vec<String> = page::review_items(view, "tau-agent")
            .into_iter()
            .map(|item| item.flag.key)
            .collect();
        let fine: Vec<String> = page::handled(view, "tau-agent")
            .into_iter()
            .filter(|done| done.what == HandledKind::LookedFine)
            .map(|done| done.shown)
            .collect();
        (keys, fine)
    });
    assert_eq!(keys, ["c2"]);
    assert_eq!(fine, ["final answer: Done."]);
    // What the host says was looked at counts too.
    let data = Data {
        reviewed: vec![("a".into(), "c2".into())],
    };
    let waiting = with_view(cx, &ui, &runs, &data, &repos, |view| {
        page::review_items(view, "tau-agent").len()
    });
    assert_eq!(waiting, 0);
}
