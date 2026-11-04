//! tau-constitution's UI: what its fold makes of the checks, the editor
//! that writes and tries a rule, the review queue, the page's totals,
//! and that it keeps to the design language.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use hegel::generators as gs;
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_constitution::ui::{
    Act,
    ConstitutionUi,
    Data,
    RuleInfo,
    Rules,
    State,
    Stats,
    page::{self, Do, Draft, HandledKind, Mark, Preset, Snapshot, Trying},
    stats::{FlaggedAnswer, RulesStats},
};
use tau_constitution_host::{NAME, Record};
use tau_ui_plugin::{
    CardInfo,
    CardMark,
    Handle,
    PluginValue,
    Request,
    RunInfo,
    UiPlugin,
    ViewCx,
    testing::{FakeRun, fold},
    tree,
};

fn card(call_id: &str, tool: &str, summary: &str) -> CardInfo {
    CardInfo {
        call_id: call_id.into(),
        tool: tool.into(),
        args: json!({ "cmd": summary }),
        summary: summary.into(),
        size: 40,
        turn: 1,
    }
}

/// One of tau-constitution's records: a check of a call or the answer,
/// a verdict on either, or a failure.
#[hegel::composite]
fn record(tc: &hegel::TestCase) -> Value {
    let rule = tc.draw(gs::sampled_from(vec!["R1", "R2", "R3"]));
    let call = tc.draw(gs::optional(gs::sampled_from(vec!["c1", "c2"])));
    let score = f64::from(tc.draw(gs::integers::<u8>().max_value(100))) / 100.0;
    match tc.draw(gs::integers::<u8>().max_value(4)) {
        0 => {
            let rules = tc.draw(gs::integers::<usize>().max_value(3));
            json!({
                "kind": "checked",
                "call_id": call,
                "scores": (1..=rules)
                    .map(|n| json!({"rule": format!("R{n}"), "score": score}))
                    .collect::<Vec<_>>(),
                // Multiples of 1/1024, so sums are exact.
                "cost": f64::from(tc.draw(gs::integers::<u32>().max_value(64)))
                    / 1024.0,
            })
        }
        1 => json!({
            "kind": "blocked", "rule": rule, "text": format!("text of {rule}"),
            "score": score, "call_id": call,
            "reason": tc.draw(gs::optional(gs::sampled_from(vec!["Rule says no"]))),
        }),
        2 => json!({
            "kind": "flagged", "rule": rule, "text": format!("text of {rule}"),
            "score": score, "call_id": call,
        }),
        3 => json!({
            "kind": "held", "rule": rule, "text": format!("text of {rule}"),
            "score": score, "hold": 1,
            "max_holds": tc.draw(gs::integers::<u32>().min_value(1).max_value(5)),
        }),
        _ => json!({
            "kind": "error", "message": "offline", "call_id": call,
            "on_error": tc.draw(gs::sampled_from(vec!["allow", "block"])),
            "held": tc.draw(gs::booleans()),
        }),
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
                stats.add(&Record::parse(&body).unwrap(), None);
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

/// Over any records, the fold counts what the records hold: checks of
/// calls and of answers, questions, failures, cost, and the rules behind
/// each block, flag and hold, an answer's flag with the run's last
/// text; each call a verdict is about is marked on its card as its last
/// verdict says, and shows its last scores; each failure and each
/// verdict on the answer gets a note of its own.
#[hegel::test(test_cases = 200)]
fn the_fold_marks_what_the_checks_decided(tc: hegel::TestCase) {
    let bodies: Vec<Value> = tc.draw(gs::vecs(record()).max_size(10));
    let mut run = FakeRun {
        cards: vec![
            card("c1", "exec_command", "rm -rf /"),
            card("c2", "write", "a.rs"),
        ],
        last_text: Some("Done.".into()),
        turn: 1,
        ..FakeRun::default()
    };
    let mut state = State::default();
    for body in &bodies {
        fold(ConstitutionUi, &mut state, body, &mut run);
    }

    let kind = |body: &Value| body["kind"].as_str().unwrap().to_owned();
    let rules_of = |what: &str| -> Vec<String> {
        bodies
            .iter()
            .filter(|body| kind(body) == what)
            .map(|body| body["rule"].as_str().unwrap().to_owned())
            .collect()
    };
    let checks = |on_call: bool| {
        bodies
            .iter()
            .filter(|body| {
                kind(body) == "checked"
                    && body["call_id"].is_string() == on_call
            })
            .count() as u32
    };
    let expected = Stats {
        calls: checks(true),
        answers: checks(false),
        questions: bodies
            .iter()
            .filter(|body| kind(body) == "checked")
            .map(|body| body["scores"].as_array().unwrap().len() as u32)
            .sum(),
        failed: bodies.iter().filter(|body| kind(body) == "error").count()
            as u32,
        cost: bodies
            .iter()
            .filter(|body| kind(body) == "checked")
            .map(|body| body["cost"].as_f64().unwrap())
            .sum(),
        blocked: rules_of("blocked"),
        flagged: rules_of("flagged"),
        held: rules_of("held"),
        max_holds: bodies
            .iter()
            .rev()
            .find(|body| kind(body) == "held")
            .map(|body| body["max_holds"].as_u64().unwrap() as u32),
        flagged_answers: bodies
            .iter()
            .filter(|body| kind(body) == "flagged" && body["call_id"].is_null())
            .map(|body| FlaggedAnswer {
                rule: body["rule"].as_str().unwrap().to_owned(),
                text: body["text"].as_str().unwrap().to_owned(),
                score: body["score"].as_f64().unwrap(),
                answer: "Done.".into(),
            })
            .collect(),
    };
    assert_eq!(state.stats, expected);

    for call in ["c1", "c2"] {
        let about = |body: &&Value| body["call_id"] == call;
        let verdict = bodies.iter().filter(about).rfind(|body| {
            ["blocked", "flagged"].contains(&kind(body).as_str())
        });
        let mark = verdict.map(|body| match kind(body).as_str() {
            "blocked" => CardMark::Blocked {
                reason: body["reason"]
                    .as_str()
                    .or(body["text"].as_str())
                    .unwrap()
                    .to_owned(),
            },
            _ => CardMark::Flagged,
        });
        assert_eq!(run.marks.get(call), mark.as_ref(), "{call}");
        let scores: Option<Vec<(String, f64)>> = bodies
            .iter()
            .filter(about)
            .rfind(|body| kind(body) == "checked")
            .map(|body| {
                body["scores"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|score| {
                        (
                            score["rule"].as_str().unwrap().to_owned(),
                            score["score"].as_f64().unwrap(),
                        )
                    })
                    .collect()
            });
        let shown = state.calls.get(call);
        if verdict.is_none() && scores.is_none() {
            assert!(shown.is_none(), "{call}");
            continue;
        }
        let shown = shown.unwrap();
        assert_eq!(shown.scores, scores.unwrap_or_default(), "{call}");
        assert_eq!(
            shown.verdict.as_ref().map(|v| (v.rule.as_str(), v.score)),
            verdict.map(|body| (
                body["rule"].as_str().unwrap(),
                body["score"].as_f64().unwrap()
            )),
            "{call}"
        );
    }

    // A note for each failure and each verdict on the answer.
    let noted = bodies
        .iter()
        .filter(|body| {
            kind(body) == "error"
                || (kind(body) != "checked" && body["call_id"].is_null())
        })
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
            card("c1", "exec_command", "rm -rf /"),
            card("c2", "write", "a.rs"),
        ],
        turn: 1,
        ..FakeRun::default()
    };
    let mut state = State::default();
    for body in [
        json!({"kind": "blocked", "rule": "R1", "text": "No deletes.", "score": 0.95,
               "call_id": "c1", "tool": "exec_command", "reason": "Rule R1 forbids it"}),
        json!({"kind": "flagged", "rule": "R2", "text": "No unwrap.", "score": 0.55,
               "call_id": "c2", "tool": "write"}),
        json!({"kind": "checked", "call_id": "c2", "tool": "write",
               "scores": [{"rule": "R2", "score": 0.55}], "cost": 0.0}),
    ] {
        fold(ConstitutionUi, &mut state, &body, &mut run);
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

/// The page as one window holds it: its UI state, as the interface
/// keeps it, and what it asked the host half to do, in order.
#[derive(Default)]
struct Page {
    ui: BTreeMap<String, Value>,
    acts: Vec<Act>,
    /// Anything else it asked of the interface.
    other: Vec<Request>,
}

impl Page {
    /// Carries out `request` as the interface would.
    fn take(&mut self, request: Request) {
        match request {
            Request::Ui {
                key,
                value: Some(value),
            } => {
                self.ui.insert(key, value);
            }
            Request::Ui { key, value: None } => {
                self.ui.remove(&key);
            }
            Request::Act(action) => {
                self.acts.push(serde_json::from_value(action).unwrap());
            }
            other => self.other.push(other),
        }
    }

    fn text(&self, id: &str) -> String {
        self.ui
            .get(&tree::draft_key(id))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    /// The page's UI state as a click reads it.
    fn now(&self) -> Snapshot {
        Snapshot {
            draft: self.draft(),
            text: self.text(page::RULE_TEXT),
            on: self.text(page::RULE_ON),
            dismissed: self
                .ui
                .get(page::DISMISSED)
                .map(|value| serde_json::from_value(value.clone()).unwrap())
                .unwrap_or_default(),
        }
    }

    /// A click that asks `doing`: the page's `do` action, carried out
    /// by the plugin on a view of the page as it is now.
    fn click(&mut self, doing: Do) {
        let args = serde_json::to_value(&doing).unwrap();
        let (handle, asked) = ui();
        let data = Data::default();
        with_handle(&[], &data, &self.ui, handle, |view| {
            ConstitutionUi.on_action("do", &args, view);
        });
        for request in asked.take() {
            self.take(request);
        }
    }

    /// Types `text` in the input `id`, as its editor keeps it.
    fn type_in(&mut self, id: &str, text: &str) {
        self.ui.insert(tree::draft_key(id), json!(text));
    }

    fn draft(&self) -> Option<Draft> {
        self.ui
            .get(page::EDITOR)
            .map(|value| serde_json::from_value(value.clone()).unwrap())
    }

    fn trying(&self) -> Option<Trying> {
        self.ui
            .get(page::TRYING)
            .map(|value| serde_json::from_value(value.clone()).unwrap())
    }
}

fn open(repo: &str, rule: Option<RuleInfo>) -> Do {
    Do::Open {
        repo: repo.into(),
        rule,
    }
}

fn place(place: &str) -> Do {
    Do::Place {
        place: place.into(),
    }
}

const SAVE: Do = Do::Save { typed: None };

#[test]
fn rules_are_written_and_edited_in_the_editor() {
    let mut page = Page::default();
    page.click(open("docbert", None));
    page.type_in(page::RULE_TEXT, "Never delete an index.");
    // Nothing picked: nothing is sent, and the editor says why.
    page.click(SAVE);
    assert_eq!(
        page.now().problem(),
        Some("Pick at least one place, or Jev has nothing to check.")
    );
    assert!(page.draft().unwrap().tried_to_save);
    assert!(page.acts.is_empty());
    // Places are picked, and any tool's field can be added by name.
    page.click(place("exec_command.cmd"));
    page.type_in(page::RULE_ON, "not a field");
    page.click(Do::AddPlace { typed: None });
    let draft = page.draft().unwrap();
    assert_eq!(draft.bad_place.as_deref(), Some("not a field"));
    assert_eq!(draft.places, ["exec_command.cmd"]);
    assert_eq!(page.text(page::RULE_ON), "not a field", "kept to fix");
    page.type_in(page::RULE_ON, "grep.pattern");
    page.click(Do::AddPlace { typed: None });
    let draft = page.draft().unwrap();
    assert_eq!(draft.bad_place, None);
    assert_eq!(draft.places, ["exec_command.cmd", "grep.pattern"]);
    assert_eq!(page.text(page::RULE_ON), "", "the field empties");
    // Presets, then a step: review never passes block.
    page.click(Do::Preset {
        preset: Preset::Strict,
    });
    assert_eq!(page.draft().unwrap().preset(), Some(Preset::Strict));
    let nudge = |mark, delta| Do::Nudge { mark, delta };
    page.click(nudge(Mark::Block, 0.05));
    assert_eq!(page.draft().unwrap().preset(), None);
    for _ in 0..20 {
        page.click(nudge(Mark::Review, 0.05));
    }
    let draft = page.draft().unwrap();
    assert_eq!((draft.review, draft.block), (0.65, 0.65));
    page.click(nudge(Mark::Review, -0.35));
    page.click(SAVE);
    assert!(page.draft().is_none());
    assert_eq!(page.text(page::RULE_TEXT), "", "the editor closes empty");
    assert_eq!(
        page.acts,
        [Act::Add {
            repo: "docbert".into(),
            text: "Never delete an index.".into(),
            on: vec!["exec_command.cmd".into(), "grep.pattern".into()],
            review: 0.3,
            block: 0.65,
        }]
    );
    // Editing opens with the rule as it is, and saves over it.
    let rule = RuleInfo {
        id: "D4".into(),
        text: "Never delete an index.".into(),
        applies_to: vec!["exec_command.cmd".into(), "grep.pattern".into()],
        review: 0.3,
        block: 0.65,
    };
    page.click(open("docbert", Some(rule)));
    let draft = page.draft().unwrap();
    assert_eq!(draft.editing.as_deref(), Some("D4"));
    assert_eq!(draft.places, ["exec_command.cmd", "grep.pattern"]);
    assert_eq!(page.text(page::RULE_TEXT), "Never delete an index.");
    page.click(place("grep.pattern"));
    page.click(SAVE);
    page.click(Do::Host {
        act: Act::Remove {
            repo: "docbert".into(),
            id: "D4".into(),
        },
    });
    assert_eq!(
        page.acts[1..],
        [
            Act::Update {
                repo: "docbert".into(),
                id: "D4".into(),
                text: "Never delete an index.".into(),
                on: vec!["exec_command.cmd".into()],
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

/// Enter in the rule's field saves it, and empties the field: a rule
/// that cannot be saved yet is kept, and saved once it can.
#[test]
fn enter_saves_what_was_typed() {
    let mut page = Page::default();
    page.click(open("docbert", None));
    page.click(Do::Save {
        typed: Some("Never delete an index.".into()),
    });
    assert!(page.acts.is_empty());
    let draft = page.draft().unwrap();
    assert_eq!(draft.kept.as_deref(), Some("Never delete an index."));
    assert_eq!(page.now().rule_text(), "Never delete an index.");
    page.click(place("exec_command.cmd"));
    page.click(SAVE);
    assert_eq!(
        page.acts,
        [Act::Add {
            repo: "docbert".into(),
            text: "Never delete an index.".into(),
            on: vec!["exec_command.cmd".into()],
            review: 0.3,
            block: 0.8,
        }]
    );
}

/// The plugin's way back to a window, with what it asks kept.
fn ui() -> (Handle, Rc<RefCell<Vec<Request>>>) {
    let asked: Rc<RefCell<Vec<Request>>> = Rc::default();
    let sink = asked.clone();
    let handle = Handle::new(
        NAME,
        Rc::new(move |_, request| sink.borrow_mut().push(request)),
    );
    (handle, asked)
}

#[test]
fn a_rule_is_tried_and_says_what_it_would_do() {
    let mut page = Page::default();
    let calls =
        vec![("exec_command".to_owned(), json!({ "cmd": "psql prod" }))];
    page.click(open("tau-agent", None));
    page.type_in(page::RULE_TEXT, "Never touch the production database.");
    // Nothing to check yet: nothing is tried.
    page.click(Do::Try {
        calls: calls.clone(),
        answers: Vec::new(),
    });
    assert!(page.acts.is_empty());
    assert!(page.draft().unwrap().tried_to_save);
    page.click(place("exec_command.cmd"));
    page.click(Do::Try {
        calls: calls.clone(),
        answers: Vec::new(),
    });
    assert_eq!(page.trying(), Some(Trying::Asking));
    let Some(Act::Try {
        text,
        calls: sent,
        answers,
        on,
        ..
    }) = page.acts.pop()
    else {
        panic!("no trial asked");
    };
    assert_eq!(
        (text.as_str(), sent, answers, on),
        (
            "Never touch the production database.",
            calls,
            Vec::new(),
            vec!["exec_command.cmd".into()]
        )
    );
    // What the host answers goes back to the editor.
    let (ui, asked) = ui();
    let reply = json!({ "Ok": [[{ "tool": "exec_command", "shown": "psql prod", "score": 0.9 }], 0.001] });
    ConstitutionUi.reply(reply, &ui);
    for request in asked.borrow_mut().drain(..) {
        page.take(request);
    }
    let Some(Trying::Done { trials, .. }) = page.trying() else {
        panic!("not tried");
    };
    assert_eq!(trials.len(), 1);
    // Changing the rule drops what trying it gave.
    page.click(place("exec_command.cmd"));
    assert_eq!(page.trying(), None);
}

/// Unreadable rules are removed only once confirmed; settings are sent
/// as set.
#[test]
fn rules_are_reset_only_once_confirmed() {
    let mut page = Page::default();
    let ask = |repo: Option<&str>| Do::AskReset {
        repo: repo.map(str::to_owned),
    };
    page.click(ask(Some("tau-agent")));
    assert_eq!(page.ui.get(page::RESETTING), Some(&json!("tau-agent")));
    page.click(ask(None));
    assert_eq!(page.ui.get(page::RESETTING), None);
    page.click(ask(Some("tau-agent")));
    assert!(page.acts.is_empty(), "asking is not removing");
    page.click(Do::Reset {
        repo: "tau-agent".into(),
    });
    assert_eq!(page.ui.get(page::RESETTING), None);
    page.click(Do::Host {
        act: Act::Settings {
            repo: "tau-agent".into(),
            blocks_unchecked: true,
            max_holds: 4,
        },
    });
    assert_eq!(
        page.acts,
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

/// The page over a repository's runs, each with what the checks did,
/// and the UI state `ui_state`.
fn with_view<R>(
    runs: &[(RunInfo, State)],
    data: &Data,
    ui_state: &BTreeMap<String, Value>,
    f: impl FnOnce(&mut ViewCx<'_, ConstitutionUi>) -> R,
) -> R {
    let handle = Handle::new(NAME, Rc::new(|_, _| {}));
    with_handle(runs, data, ui_state, handle, f)
}

/// The same, with what the view asks going to `handle`.
fn with_handle<R>(
    runs: &[(RunInfo, State)],
    data: &Data,
    ui_state: &BTreeMap<String, Value>,
    handle: Handle,
    f: impl FnOnce(&mut ViewCx<'_, ConstitutionUi>) -> R,
) -> R {
    let runs: Vec<(RunInfo, PluginValue)> = runs
        .iter()
        .map(|(run, state)| (run.clone(), PluginValue::typed(state.clone())))
        .collect();
    let list = move || runs.clone();
    let cards = |_: &RunId| Vec::new();
    let params = BTreeMap::new();
    let repos: BTreeMap<String, Rules> = BTreeMap::new();
    let repos = tau_ui_plugin::testing::RepoValues::new(&repos);
    let repos = repos.refs();
    let mut view = ViewCx::new(
        &ConstitutionUi,
        None,
        data,
        &(),
        &repos,
        None,
        &params,
        false,
        true,
        1400.,
        handle,
        &list,
        &cards,
    )
    .with_ui_state(ui_state);
    f(&mut view)
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
#[test]
fn what_waits_for_a_person_and_what_was_handled() {
    let mut run = FakeRun {
        cards: vec![
            card("c1", "exec_command", "rm -rf /"),
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
        fold(ConstitutionUi, &mut state, &body, &mut run);
    }
    let runs = vec![
        (info("a", "tau-agent"), state.clone()),
        // Another repository's run is not this one's to count.
        (info("b", "docbert"), state),
    ];
    let data = Data::default();
    let mut page = Page::default();
    let (keys, stats) = with_view(&runs, &data, &page.ui, |view| {
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
    // Looks fine: off the queue, and onto what was handled; the host
    // is told.
    page.click(Do::Reviewed {
        run: "a".into(),
        key: "answer-0".into(),
    });
    assert_eq!(
        page.acts,
        [Act::Reviewed {
            run: "a".into(),
            key: "answer-0".into()
        }]
    );
    let (keys, fine) = with_view(&runs, &data, &page.ui, |view| {
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
    let waiting = with_view(&runs, &data, &page.ui, |view| {
        page::review_items(view, "tau-agent").len()
    });
    assert_eq!(waiting, 0);
}
