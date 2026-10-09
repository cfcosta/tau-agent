//! A repository's main chat before anything was said in it: the first
//! message goes to main, not to a chat forked from an empty one, and a
//! repository added while the person waits on a new run opens on main.

use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_agent::{event::StopReason, tool::RunId};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    route::Route,
    update::HostUpdate,
    view::RunView,
    workspace::WorkspaceEvent,
};

const REPO: &str = "tau-agent";

fn main_chat() -> RunView {
    let mut view =
        RunView::new(RunId("main".into()), "main", "coder", "gpt-5.5")
            .in_repo(REPO);
    view.finish_stored(StopReason::Stop, 0.0, 0.0);
    view
}

fn repo() -> Repo {
    Repo {
        name: REPO.into(),
        main: Some(RunId("main".into())),
        ..Default::default()
    }
}

type Events = Rc<RefCell<Vec<WorkspaceEvent>>>;

fn open(
    cx: &mut TestAppContext,
    runs: Vec<RunView>,
    catalog: Catalog,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
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
    let cx = VisualTestContext::from_window(window.into(), cx);
    (workspace, cx, events)
}

fn listed() -> Catalog {
    Catalog {
        repos: vec![repo()],
        open_repos: vec![REPO.into()],
        ..Default::default()
    }
}

#[gpui::test]
fn a_new_run_on_an_untouched_main_goes_to_main(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx, vec![main_chat()], listed());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.submit_prompt("hello".into(), cx);
        assert_eq!(ws.route(), &Route::Run(RunId("main".into())));
    });
    assert!(matches!(
        events.borrow().last(),
        Some(WorkspaceEvent::Say { run, text, .. })
            if run.0.as_ref() == "main" && text == "hello"
    ));
}

#[gpui::test]
fn a_new_run_on_a_main_talked_to_forks_it(cx: &mut TestAppContext) {
    let mut main = main_chat();
    main.push_user("earlier");
    let (workspace, mut cx, events) = open(cx, vec![main], listed());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.submit_prompt("a side task".into(), cx);
    });
    assert!(matches!(
        events.borrow().last(),
        Some(WorkspaceEvent::NewRun { repo, prompt, .. })
            if repo == REPO && prompt == "a side task"
    ));
}

#[gpui::test]
fn a_repo_added_while_waiting_opens_on_main(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx, Vec::new(), Catalog::default());
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.apply(
            HostUpdate::Repo {
                repo: repo(),
                main: Some(Box::new(main_chat())),
            },
            cx,
        );
        assert_eq!(ws.route(), &Route::Run(RunId("main".into())));
        assert_eq!(ws.selected_repo(), Some(REPO));
    });
}
