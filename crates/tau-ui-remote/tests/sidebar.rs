//! The sidebar says what each chat needs of the person: a chat that
//! asks, one ready to land, one that would conflict, one tau's closing
//! cut off, one that works, and, still listed, one that landed or was
//! dropped. The repository counts those that need the person, the rows
//! keep their order, and a phone that takes the computer's snapshot says
//! the same.

use gpui::{TestAppContext, VisualTestContext};
use tau_agent::{event::StopReason, tool::RunId};
use tau_ui_remote::{
    Workspace,
    attention::{Attention, Forecast},
    catalog::{Catalog, Repo},
    repos::TreeRow,
    update::HostUpdate,
    view::{Ending, Origin, RunView},
};

const REPO: &str = "tau-agent";

fn id(name: &str) -> RunId {
    RunId(name.into())
}

/// A chat forked from main, live until `stop` says otherwise.
fn chat(name: &str, stop: Option<StopReason>) -> RunView {
    let mut view = RunView::new(id(name), name, "coder", "gpt-5.5")
        .in_repo(REPO)
        .with_origin(Origin::Fork {
            from: id("main"),
            turn: 0,
        });
    view.turn = 7;
    if let Some(stop) = stop {
        view.finish_stored(stop, 0.0, 0.0);
    }
    view
}

/// A chat tau's closing cut off, as history rebuilds it.
fn interrupted() -> RunView {
    let mut view = chat("interrupted", None);
    view.interrupted_stored(0.0, 0.0);
    view
}

fn catalog() -> Catalog {
    Catalog {
        repos: vec![Repo {
            name: REPO.into(),
            main: Some(id("main")),
            ..Default::default()
        }],
        open_repos: vec![REPO.into()],
        ..Default::default()
    }
}

fn runs() -> Vec<RunView> {
    let mut main =
        RunView::new(id("main"), "main", "coder", "gpt-5.5").in_repo(REPO);
    main.finish_stored(StopReason::Stop, 0.0, 0.0);
    vec![
        main,
        chat("working", None),
        chat("asks", None),
        chat("ready", Some(StopReason::Stop)),
        chat("conflict", Some(StopReason::Stop)),
        interrupted(),
    ]
}

/// What each listed row of the repository says, in the sidebar's order.
fn rows(ws: &Workspace, cx: &mut gpui::App) -> Vec<(String, Attention)> {
    let all = ws.repo_rows("");
    let tree: Vec<RunView> = all
        .iter()
        .flat_map(|rows| ws.repo_tree(rows))
        .filter_map(|row| match row {
            TreeRow::Run { run, .. } => Some(run.clone()),
            TreeRow::Child { .. } => None,
        })
        .collect();
    tree.iter()
        .map(|run| (run.title.clone(), ws.attention(run, cx)))
        .collect()
}

#[gpui::test]
fn the_sidebar_says_what_each_chat_needs(cx: &mut TestAppContext) {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs(), catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    // A phone, with nothing until the computer's snapshot.
    let phone = cx.add_window(|window, cx| {
        Workspace::new("phone", Vec::new(), Catalog::default(), window, cx)
    });
    let phone = phone.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    let order_before = workspace.update(&mut cx, |ws, cx| {
        rows(ws, cx)
            .into_iter()
            .map(|(title, _)| title)
            .collect::<Vec<_>>()
    });

    let question = "Delete the 3 workspaces with no run, or keep them?";
    let ask: tau_ask::Ask = serde_json::from_value(serde_json::json!({
        "questions": [{
            "question": question,
            "header": "Workspaces",
            "options": [
                { "label": "Delete", "description": "They have no run." },
                { "label": "Keep", "description": "Look at them first." }
            ]
        }]
    }))
    .unwrap();
    let forecast =
        |run: &str, changes, conflicts: &[&str]| HostUpdate::Forecast {
            run: id(run),
            forecast: Some(Forecast {
                changes,
                conflicts: conflicts
                    .iter()
                    .map(|path| path.to_string())
                    .collect(),
            }),
        };
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            HostUpdate::PluginFold {
                run: id("asks"),
                plugin: tau_ask::NAME.into(),
                body: tau_ask::Record::Asked {
                    call: "call_1".into(),
                    ask,
                }
                .to_value(),
            },
            cx,
        );
        ws.apply(forecast("ready", 2, &[]), cx);
        ws.apply(forecast("conflict", 1, &["a.rs", "b.rs"]), cx);
    });
    cx.run_until_parked();

    let said = workspace.update(&mut cx, |ws, cx| rows(ws, cx));
    let said_of = |title: &str| {
        said.iter()
            .find(|(row, _)| row == title)
            .map(|(_, attention)| attention.clone())
            .unwrap_or_else(|| panic!("no row {title}: {said:?}"))
    };
    assert_eq!(said_of("working"), Attention::Working { turn: 7 });
    assert_eq!(
        said_of("asks"),
        Attention::Asks {
            question: question.into()
        }
    );
    assert_eq!(said_of("ready"), Attention::ReadyToLand { changes: 2 });
    assert_eq!(
        said_of("conflict"),
        Attention::WouldConflict {
            files: vec!["a.rs".into(), "b.rs".into()]
        }
    );
    assert_eq!(said_of("interrupted"), Attention::Interrupted);
    assert_eq!(said_of("main"), Attention::Idle);
    // The rows keep their order: a state never moves one.
    let order: Vec<String> =
        said.iter().map(|(title, _)| title.clone()).collect();
    assert_eq!(order, order_before);
    // Asking, ready to land and would conflict need the person.
    let need_you = workspace.update(&mut cx, |ws, cx| ws.need_you(REPO, cx));
    assert_eq!(need_you, 3);

    // A phone takes the computer's snapshot and says the same.
    let snapshot = workspace.read_with(&cx, |ws, _| ws.snapshot());
    phone.update(&mut cx, |ws, cx| ws.apply(snapshot, cx));
    cx.run_until_parked();
    let on_phone = phone.update(&mut cx, |ws, cx| {
        let need_you = ws.need_you(REPO, cx);
        (rows(ws, cx), need_you)
    });
    assert_eq!(on_phone, (said, 3));

    // Answered, the chat works again; landed, the chat that was ready
    // says so, and no longer needs the person; dropped, the one that
    // would conflict says that. Both stay listed where they were.
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            HostUpdate::PluginFold {
                run: id("asks"),
                plugin: tau_ask::NAME.into(),
                body: tau_ask::Record::Closed {
                    call: "call_1".into(),
                }
                .to_value(),
            },
            cx,
        );
        let view = ws.run(&id("asks")).unwrap().clone();
        assert_eq!(ws.attention(&view, cx), Attention::Working { turn: 7 });
        let landing = tau_vcs::Landing {
            changes: Vec::new(),
            conflicts: Vec::new(),
            head: "0".repeat(40),
        };
        ws.apply(
            HostUpdate::Landed {
                run: id("ready"),
                landing: Ok(landing),
            },
            cx,
        );
        ws.apply(
            HostUpdate::Dropped {
                run: id("conflict"),
                result: Ok(()),
            },
            cx,
        );
        assert!(ws.is_closed(&id("ready")) && ws.is_closed(&id("conflict")));
        assert_eq!(ws.need_you(REPO, cx), 0);
        let said = rows(ws, cx);
        let order: Vec<&str> =
            said.iter().map(|(title, _)| title.as_str()).collect();
        assert_eq!(order, order_before);
        let said_of = |title: &str| {
            said.iter()
                .find(|(row, _)| row == title)
                .map(|(_, attention)| attention.clone())
        };
        assert_eq!(said_of("ready"), Some(Attention::Landed));
        assert_eq!(said_of("conflict"), Some(Attention::Dropped));
    });
}

/// A chat closed by hand, without landing or dropping it, leaves the
/// sidebar; one that ended for good stays, as history loads it.
#[gpui::test]
fn closed_chats_leave_the_sidebar_unless_they_ended(cx: &mut TestAppContext) {
    cx.update(tau_ui_remote::init);
    let mut landed = chat("landed", Some(StopReason::Stop));
    landed.ending = Some(Ending::Landed {
        on: id("main"),
        changes: 2,
    });
    let mut dropped = chat("dropped", Some(StopReason::Stop));
    dropped.ending = Some(Ending::Dropped);
    let main = runs().remove(0);
    let runs = vec![
        main,
        chat("closed", Some(StopReason::Stop)),
        landed,
        dropped,
    ];
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs, catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        let closed = Catalog {
            closed_runs: vec![id("landed"), id("dropped"), id("closed")],
            ..catalog()
        };
        ws.set_catalog(closed, cx);
        let said = rows(ws, cx);
        let said_of = |title: &str| {
            said.iter()
                .find(|(row, _)| row == title)
                .map(|(_, attention)| attention.clone())
        };
        assert_eq!(said_of("closed"), None, "{said:?}");
        assert_eq!(said_of("landed"), Some(Attention::Landed));
        assert_eq!(said_of("dropped"), Some(Attention::Dropped));
    });
}

/// Chats waiting in main's landing queue say where they wait, and main
/// says what a turn left in conflict on it, while marked, even once the
/// person put its card away. A phone says the same.
#[gpui::test]
fn queued_chats_and_conflicts_on_main_say_so(cx: &mut TestAppContext) {
    use tau_ui_remote::{
        attention::Queued,
        queue::{MainConflicts, Waiting},
    };
    cx.update(tau_ui_remote::init);
    let main = runs().remove(0);
    let runs = vec![
        main,
        chat("clean", Some(StopReason::Stop)),
        chat("confirm", Some(StopReason::Stop)),
        chat("idle", Some(StopReason::Stop)),
    ];
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs, catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let phone = cx.add_window(|window, cx| {
        Workspace::new("phone", Vec::new(), Catalog::default(), window, cx)
    });
    let phone = phone.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    let waiting = |run: &str, conflicts: &[&str], confirmed: &[&str]| Waiting {
        run: run.into(),
        title: run.into(),
        changes: 2,
        conflicts: conflicts.iter().map(|f| f.to_string()).collect(),
        confirmed: confirmed.iter().map(|f| f.to_string()).collect(),
        sub_agent: None,
    };
    let files = vec!["a.rs".to_owned(), "b.rs".to_owned()];
    let marked = |dismissed| MainConflicts {
        files: files.clone(),
        from: Some("clean".into()),
        prompt: "Resolve them.".into(),
        dismissed,
    };
    let said = workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            HostUpdate::LandingQueue {
                main: id("main"),
                queue: vec![
                    waiting("clean", &[], &[]),
                    waiting("confirm", &["c.rs"], &[]),
                ],
                conflicts: Some(marked(false)),
            },
            cx,
        );
        rows(ws, cx)
    });
    let said_of = |title: &str| {
        said.iter()
            .find(|(row, _)| row == title)
            .map(|(_, attention)| attention.clone())
            .unwrap_or_else(|| panic!("no row {title}: {said:?}"))
    };
    assert_eq!(
        said_of("main"),
        Attention::ConflictsOnMain {
            files: files.clone()
        }
    );
    assert_eq!(
        said_of("clean"),
        Attention::Queued(Queued {
            position: 1,
            needs_confirmation: false
        })
    );
    assert_eq!(
        said_of("confirm"),
        Attention::Queued(Queued {
            position: 2,
            needs_confirmation: true
        })
    );
    assert_eq!(said_of("idle"), Attention::Idle);
    // Main's conflicts and the chat waiting for a confirmation need the
    // person; the clean one waits on main alone.
    let need_you = workspace.update(&mut cx, |ws, cx| ws.need_you(REPO, cx));
    assert_eq!(need_you, 2);

    let snapshot = workspace.read_with(&cx, |ws, _| ws.snapshot());
    phone.update(&mut cx, |ws, cx| ws.apply(snapshot, cx));
    cx.run_until_parked();
    let on_phone = phone.update(&mut cx, |ws, cx| rows(ws, cx));
    assert_eq!(on_phone, said);

    // Put away, the card goes and the mark stays; resolved, main is
    // idle again, and the queue's chats say so as it drains.
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(
            HostUpdate::LandingQueue {
                main: id("main"),
                queue: vec![waiting("confirm", &["c.rs"], &["c.rs"])],
                conflicts: Some(marked(true)),
            },
            cx,
        );
        let view = ws.run(&id("main")).unwrap().clone();
        assert_eq!(
            ws.attention(&view, cx),
            Attention::ConflictsOnMain {
                files: files.clone()
            }
        );
        ws.apply(
            HostUpdate::LandingQueue {
                main: id("main"),
                queue: vec![waiting("confirm", &["c.rs"], &["c.rs"])],
                conflicts: None,
            },
            cx,
        );
        let said = rows(ws, cx);
        let said_of = |title: &str| {
            said.iter()
                .find(|(row, _)| row == title)
                .map(|(_, attention)| attention.clone())
        };
        assert_eq!(said_of("main"), Some(Attention::Idle));
        assert_eq!(said_of("clean"), Some(Attention::Idle));
        assert_eq!(
            said_of("confirm"),
            Some(Attention::Queued(Queued {
                position: 1,
                needs_confirmation: false
            }))
        );
        assert_eq!(ws.need_you(REPO, cx), 0);
    });
}
