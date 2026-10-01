//! tau-ask in the workspace: a question a live run waits on takes the
//! composer's place and the keys, and what the person picks, writes and
//! notes goes to the host as the call's reply.

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::json;
use tau_agent::tool::RunId;
use tau_ask::{Answer, Record, Reply, ui::Act};
use tau_ui::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    demo,
    route::Route,
    update::HostUpdate,
};

type Events = std::rc::Rc<std::cell::RefCell<Vec<WorkspaceEvent>>>;

fn open(
    cx: &mut TestAppContext,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    open_with(cx, vec![demo::retry_after()])
}

fn open_with(
    cx: &mut TestAppContext,
    runs: Vec<tau_ui::view::RunView>,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    cx.update(tau_ui::init);
    let window = cx.add_window(|window, cx| {
        let catalog = Catalog {
            repos: vec![tau_ui::catalog::Repo {
                name: "tau-agent".into(),
                main: Some(demo::run_id()),
                ..Default::default()
            }],
            ..Default::default()
        };
        Workspace::new("tau", runs, catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let events = Events::default();
    let seen = events.clone();
    cx.update(|cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    (
        workspace,
        VisualTestContext::from_window(window.into(), cx),
        events,
    )
}

fn asked() -> Record {
    Record::Asked {
        call: "call_ask".into(),
        ask: serde_json::from_value(json!({ "questions": [
            {
                "question": "How should the ask tool wait for your answer?",
                "header": "Waiting",
                "options": [
                    { "label": "Hold the call open (Recommended)", "description": "Block until the answer comes.", "preview": "select! { a = rx => a }" },
                    { "label": "End the turn", "description": "Read the answer as the next message.", "preview": "stop_run()" }
                ]
            },
            {
                "question": "Where else should a waiting question show?",
                "header": "Surfaces",
                "multi_select": true,
                "options": [
                    { "label": "Run list badge", "description": "A dot on the run." },
                    { "label": "Notification", "description": "When the window is away." },
                    { "label": "Parent run", "description": "For child runs." }
                ]
            }
        ]}))
        .unwrap(),
    }
}

fn fold(record: &Record) -> HostUpdate {
    fold_in(&demo::run_id(), record)
}

fn fold_in(run: &RunId, record: &Record) -> HostUpdate {
    HostUpdate::PluginFold {
        run: run.clone(),
        plugin: tau_ask::NAME.into(),
        body: record.to_value(),
    }
}

/// The replies the panel sent the host.
fn sent(events: &Events) -> Vec<Act> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::PluginAct { plugin, action }
                if plugin == tau_ask::NAME =>
            {
                serde_json::from_value(action.clone()).ok()
            }
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn a_waiting_question_takes_the_composers_place_and_its_keys(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx);
        assert!(ws.run(&demo::run_id()).unwrap().status.is_live());
        ws.apply(fold(&asked()), cx);
    });
    cx.run_until_parked();

    // The panel took the keys as it appeared. `2` picks the second
    // choice and goes on; on the checklist `1` and `3` toggle, and `n`
    // opens the note, which takes text and closes on Enter.
    cx.simulate_keystrokes("2");
    cx.run_until_parked();
    cx.simulate_keystrokes("1 3 n");
    cx.run_until_parked();
    cx.simulate_input("only for long waits");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // Back on the panel: Right is the review, and Enter sends.
    cx.simulate_keystrokes("right enter");
    cx.run_until_parked();

    let acts = sent(&events);
    assert_eq!(acts.len(), 1, "{acts:?}");
    assert_eq!(acts[0].run, demo::run_id().0.to_string());
    assert_eq!(acts[0].call, "call_ask");
    assert_eq!(
        acts[0].reply,
        Reply::Answered {
            answers: vec![
                Answer {
                    picked: vec!["End the turn".into()],
                    other: None,
                    note: None,
                },
                Answer {
                    picked: vec!["Run list badge".into(), "Parent run".into()],
                    other: None,
                    note: Some("only for long waits".into()),
                },
            ],
        }
    );

    // Answered, the composer is back with the keys: what the person
    // types goes into it.
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            fold(&Record::Answered {
                call: "call_ask".into(),
                reply: acts[0].reply.clone(),
            }),
            cx,
        );
    });
    cx.run_until_parked();
    cx.simulate_input("thanks");
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.composer_text(cx), "thanks");
    });
}

/// A question that comes while the person writes a message does not
/// take their keys: what they type goes nowhere near an answer.
#[gpui::test]
fn the_panel_does_not_take_the_keys_from_a_message_being_written(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    cx.simulate_input("now also");
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.composer_text(cx), "now also");
        ws.apply(fold(&asked()), cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("2 right enter");
    cx.run_until_parked();
    assert!(sent(&events).is_empty());
}

/// A plugin can cancel a run, as the run's Cancel button does: the
/// panel offers it on a phone, where the header's is not there.
#[gpui::test]
fn a_plugin_cancels_a_run(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.plugin_handle(tau_ask::NAME).cancel(&demo::run_id(), cx);
    });
    cx.run_until_parked();
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        WorkspaceEvent::Cancel { run } if *run == demo::run_id()
    )));
}

#[gpui::test]
fn the_persons_own_answer_counts(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx);
        ws.apply(fold(&asked()), cx);
    });
    cx.run_until_parked();
    // `3` is the person's own answer on a two-choice question: its field
    // takes the text, and Enter goes on.
    cx.simulate_keystrokes("3");
    cx.run_until_parked();
    cx.simulate_input("hold, but time out");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("2 right enter");
    cx.run_until_parked();
    let acts = sent(&events);
    assert_eq!(acts.len(), 1, "{acts:?}");
    let Reply::Answered { answers } = &acts[0].reply else {
        panic!("answered");
    };
    assert_eq!(answers[0].other.as_deref(), Some("hold, but time out"));
    assert!(answers[0].picked.is_empty());
    assert_eq!(answers[1].picked, ["Notification"]);
}

/// Two runs waiting at once, on calls with the same id: each keeps its
/// own answers as the person goes between them, and each reply goes to
/// its own run.
#[gpui::test]
fn two_waiting_runs_keep_their_own_answers(cx: &mut TestAppContext) {
    let second = RunId("second".into());
    let mut other = demo::retry_after();
    other.id = second.clone();
    other.title = "second".into();
    let (workspace, mut cx, events) =
        open_with(cx, vec![demo::retry_after(), other]);
    let three = Record::Asked {
        call: "call_ask".into(),
        ask: serde_json::from_value(json!({ "questions": [{
            "question": "Which lane?",
            "header": "Lane",
            "options": [
                { "label": "Left", "description": "l" },
                { "label": "Middle", "description": "m" },
                { "label": "Right", "description": "r" }
            ]
        }]}))
        .unwrap(),
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(fold(&asked()), cx);
        ws.apply(fold_in(&second, &three), cx);
        ws.navigate(Route::Run(demo::run_id()), cx);
    });
    cx.run_until_parked();
    // The first run's first question, answered; then the second run's.
    cx.simulate_keystrokes("2");
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(second.clone()), cx)
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("3");
    cx.run_until_parked();
    // Back to the first: it is where it was left, on its checklist.
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("1 right enter");
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(second.clone()), cx)
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // A second Enter, once sent, sends nothing more.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let acts = sent(&events);
    assert_eq!(acts.len(), 2, "{acts:?}");
    let first = acts
        .iter()
        .find(|act| act.run == demo::run_id().0.to_string())
        .unwrap();
    let Reply::Answered { answers } = &first.reply else {
        panic!("answered");
    };
    assert_eq!(answers[0].picked, ["End the turn"]);
    assert_eq!(answers[1].picked, ["Run list badge"]);
    let other = acts.iter().find(|act| act.run == "second").unwrap();
    assert_eq!(
        other.reply,
        Reply::Answered {
            answers: vec![Answer {
                picked: vec!["Right".into()],
                ..Answer::default()
            }]
        }
    );
}
