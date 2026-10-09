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

/// A run with forks folds them away, and shows them again; a run with
/// none has nothing to fold. A sub-agent that ended is not listed: it
/// is in main's tray, for the round.
#[gpui::test]
fn a_run_folds_its_children_away(cx: &mut TestAppContext) {
    cx.update(tau_ui_remote::init);
    let mut runs = runs();
    let mut sub = RunView::new(id("sub"), "sub", "coder", "gpt-5.5")
        .in_repo(REPO)
        .with_origin(Origin::SubAgent { parent: id("main") });
    sub.finish_stored(StopReason::Stop, 0.0, 0.0);
    runs.insert(0, sub);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs, catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let tree = |ws: &Workspace| -> Vec<(String, Option<bool>)> {
        let all = ws.repo_rows("");
        all.iter()
            .flat_map(|rows| ws.repo_tree(rows))
            .filter_map(|row| match row {
                TreeRow::Run { run, folded, .. } => {
                    Some((run.title.clone(), folded))
                }
                TreeRow::Child { .. } => None,
            })
            .collect()
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(HostUpdate::Closed(id("sub")), cx);
        let open = tree(ws);
        assert_eq!(open[0], ("main".to_owned(), Some(false)));
        assert!(!open.iter().any(|(title, _)| title == "sub"), "{open:?}");
        ws.toggle_fold(&id("main"), cx);
        assert_eq!(tree(ws), [("main".to_owned(), Some(true))]);
        ws.toggle_fold(&id("main"), cx);
        assert_eq!(tree(ws), open);
    });
}

/// A main chat's crew is every sub-agent still working, and those it
/// spawned since the person's last message; the others went with their
/// round. The sidebar lists those working under main; one that ended
/// is not listed, and its chat marks main.
#[gpui::test]
fn the_crew_is_this_rounds_sub_agents(cx: &mut TestAppContext) {
    use std::sync::Arc;

    use tau_agent::{event::RunEvent, tool::ToolOutput};
    use tau_ui_remote::crew::Standing;

    cx.update(tau_ui_remote::init);
    let mut main =
        RunView::new(id("main"), "main", "coder", "gpt-5.5").in_repo(REPO);
    let spawn = |main: &mut RunView, call: &str, run: &str| {
        main.apply(&RunEvent::ToolStart {
            run: id("main"),
            call_id: call.into(),
            tool: Arc::from("spawn"),
            args: serde_json::json!({ "task": run }),
            parent: None,
        });
        main.apply(&RunEvent::ToolEnd {
            run: id("main"),
            call_id: call.into(),
            output: Arc::new(ToolOutput {
                details: Some(serde_json::json!({ "run": run })),
                ..ToolOutput::text("started")
            }),
            is_error: false,
            parent: None,
        });
    };
    main.push_user("first");
    spawn(&mut main, "c1", "old");
    main.push_user("second");
    spawn(&mut main, "c2", "new");
    main.finish_stored(StopReason::Stop, 0.0, 0.0);
    let sub = |name: &str, finished: bool| {
        let mut view = RunView::new(id(name), name, "coder", "gpt-5.5")
            .in_repo(REPO)
            .with_origin(Origin::SubAgent { parent: id("main") });
        if finished {
            view.finish_stored(StopReason::Stop, 0.0, 0.0);
        }
        view
    };
    let runs = vec![
        sub("working", false),
        sub("new", true),
        sub("old", true),
        main,
    ];
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs, catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    workspace.update(&mut cx, |ws, _| {
        let main = ws.run(&id("main")).unwrap();
        let crew: Vec<(String, Standing)> = ws
            .crew(main)
            .into_iter()
            .map(|member| (member.run.title.clone(), member.standing))
            .collect();
        assert_eq!(crew.len(), 2, "{crew:?}");
        assert_eq!(crew[0].0, "working");
        assert!(crew[0].1.is_working());
        assert_eq!(crew[1], ("new".to_owned(), Standing::WaitingToLand));
        assert_eq!(ws.working_crew(&id("main")), 1);
        let listed: Vec<&str> = ws
            .listed_children(main)
            .map(|run| run.title.as_str())
            .collect();
        assert_eq!(listed, ["working"]);
        // Sub-agents at work do not count toward the forks shown.
        let rows = ws.repo_rows("");
        assert_eq!(rows[0].older, 0);
        assert_eq!(rows[0].main_children, None);
        let working = ws.run(&id("working")).unwrap();
        assert_eq!(ws.listed_as(working), &id("working"));
        let ended = ws.run(&id("new")).unwrap();
        assert_eq!(ws.listed_as(ended), &id("main"));
    });
}

/// A row's icon says where its run stands, git's way, and its counts
/// say how much: main's changes to push, a fork's to land or its
/// conflicting files, its place in the queue, what it landed.
#[test]
fn a_row_says_where_its_run_stands_and_how_much() {
    use tau_ui_kit::{assets::Icon, theme::Theme};
    use tau_ui_remote::{
        attention::Queued,
        ui::chrome::{counts, state_icon},
    };

    let t = Theme::tokyo_night();
    let fork = chat("fork", None);
    let mut sub = RunView::new(id("sub"), "sub", "coder", "gpt-5.5")
        .with_origin(Origin::SubAgent { parent: id("main") });
    let main = RunView::new(id("main"), "main", "coder", "gpt-5.5");
    let icon = |attention: &Attention, run: &RunView, is_main, unpushed| {
        state_icon(attention, run, is_main, unpushed, &t).0
    };
    let said =
        |attention: &Attention, run: &RunView, unpushed| -> Vec<String> {
            counts(attention, run, None, unpushed, &t)
                .into_iter()
                .map(|(count, _)| count)
                .collect()
        };
    // Main is its branch, at work or at rest, with what it would push.
    assert_eq!(icon(&Attention::Idle, &main, true, 2), Icon::Branch);
    assert_eq!(
        icon(&Attention::Working { turn: 1 }, &main, true, 0),
        Icon::Branch
    );
    assert_eq!(said(&Attention::Idle, &main, 2), ["↑2"]);
    assert!(said(&Attention::Idle, &main, 0).is_empty());
    // A fork at work is a draft; done, a pull request with its changes.
    assert_eq!(
        icon(&Attention::Working { turn: 1 }, &fork, false, 0),
        Icon::Draft
    );
    let ready = Attention::ReadyToLand { changes: 3 };
    assert_eq!(
        (icon(&ready, &fork, false, 0), said(&ready, &fork, 0)),
        (Icon::PullRequest, vec!["3".to_owned()])
    );
    let conflict = Attention::WouldConflict {
        files: vec!["a".into(), "b".into()],
    };
    assert_eq!(
        (icon(&conflict, &fork, false, 0), said(&conflict, &fork, 0)),
        (Icon::Warning, vec!["2 files".to_owned()])
    );
    let queued = Attention::Queued(Queued {
        position: 2,
        needs_confirmation: false,
    });
    assert_eq!(
        (icon(&queued, &fork, false, 0), said(&queued, &fork, 0)),
        (Icon::Clock, vec!["#2".to_owned()])
    );
    assert_eq!(
        icon(
            &Attention::Asks {
                question: "?".into()
            },
            &fork,
            false,
            0
        ),
        Icon::Question
    );
    assert_eq!(icon(&Attention::Failed, &sub, false, 0), Icon::Failed);
    // At the end: a merge, with what it landed, or a closed one.
    sub.ending = Some(Ending::Landed {
        on: id("main"),
        changes: 4,
    });
    assert_eq!(
        (
            icon(&Attention::Landed, &sub, false, 0),
            said(&Attention::Landed, &sub, 0)
        ),
        (Icon::Merge, vec!["4".to_owned()])
    );
    assert_eq!(icon(&Attention::Dropped, &fork, false, 0), Icon::Closed);
    // At rest, a run is what it is.
    assert_eq!(icon(&Attention::Idle, &fork, false, 0), Icon::Fork);
    sub.ending = None;
    assert_eq!(icon(&Attention::Idle, &sub, false, 0), Icon::SubAgent);
    // A sub-agent at work stays one, not a fork's draft.
    assert_eq!(
        icon(&Attention::Working { turn: 1 }, &sub, false, 0),
        Icon::SubAgent
    );
}

/// A repository menu's entry does what it says, not what the row drawn
/// under the menu does.
#[gpui::test]
fn the_repo_menu_opens_memory(cx: &mut TestAppContext) {
    use gpui::{Modifiers, size};
    use tau_ui_remote::route::Route;

    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs(), catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.simulate_resize(size(gpui::px(1200.), gpui::px(800.)));
    workspace.update(&mut cx, |ws, cx| ws.toggle_repo_menu(REPO, cx));
    cx.run_until_parked();
    let entry = cx.debug_bounds("menu-memory").expect("the menu is drawn");
    cx.simulate_click(entry.center(), Modifiers::none());
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, _| {
        assert!(
            matches!(
                ws.route(),
                Route::Plugin { plugin, page, .. }
                    if plugin == tau_memory::NAME && page == "notes"
            ),
            "the menu opened {:?}",
            ws.route(),
        );
    });
}
