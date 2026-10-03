//! The sidebar says what each chat needs of the person: a chat that
//! asks, one ready to land, one that would conflict, one tau's closing
//! cut off, one that works. The repository counts those that need the
//! person, the rows keep their order, and a phone that takes the
//! computer's snapshot says the same.

use gpui::{TestAppContext, VisualTestContext};
use tau_agent::{event::StopReason, tool::RunId};
use tau_ui_remote::{
    Workspace,
    attention::{Attention, Forecast},
    catalog::{Catalog, Repo},
    repos::TreeRow,
    update::HostUpdate,
    view::{INTERRUPTED, Origin, RunView},
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
        chat("interrupted", Some(StopReason::Error(INTERRUPTED.into()))),
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
    // says so, and no longer needs the person.
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
        let view = ws.run(&id("ready")).unwrap().clone();
        assert_eq!(ws.attention(&view, cx), Attention::Landed);
        assert_eq!(ws.need_you(REPO, cx), 1);
    });
}
