//! A repository whose conversations a plugin sets a model for (an
//! agent's, ADR 0037): the person's messages there go to it unless they
//! pick another, and the composer's chip says so.

use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_agent::tool::RunId;
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Own, Repo},
    models::{Effort, ModelChoice},
    route::Route,
    view::RunView,
    workspace::{PickerTarget, WorkspaceEvent},
};

const AGENT: &str = "pr-triage";
const REPO: &str = "tau-agent";

fn id(name: &str) -> RunId {
    RunId(name.into())
}

fn agents_model() -> ModelChoice {
    ModelChoice::new("gpt-6-luna", Effort::Low)
}

fn catalog() -> Catalog {
    Catalog {
        repos: vec![
            Repo {
                name: REPO.into(),
                main: Some(id("main")),
                ..Default::default()
            },
            Repo {
                name: AGENT.into(),
                main: Some(id("agent-main")),
                own: Some(Own::Agent),
                model: Some(agents_model()),
                ..Default::default()
            },
        ],
        open_repos: vec![REPO.into()],
        ..Default::default()
    }
}

/// Both last ran on the person's model.
fn runs() -> Vec<RunView> {
    vec![
        RunView::new(id("main"), "main", "coder", "gpt-5.5").in_repo(REPO),
        RunView::new(id("agent-main"), "main", "coder", "gpt-5.5")
            .in_repo(AGENT),
    ]
}

type Events = Rc<RefCell<Vec<WorkspaceEvent>>>;

fn open(
    cx: &mut TestAppContext,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs(), catalog(), window, cx)
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
    cx.run_until_parked();
    (workspace, cx, events)
}

/// The model of the last message sent.
fn said(events: &Events) -> Option<ModelChoice> {
    events.borrow().iter().rev().find_map(|event| match event {
        WorkspaceEvent::Say { model, .. } => Some(model.clone()),
        _ => None,
    })
}

#[gpui::test]
fn an_agent_s_conversation_goes_to_its_model(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        let agent = PickerTarget::Run(id("agent-main"));
        assert_eq!(ws.choice_for(&agent), agents_model());
        // The person's own repository keeps what its run ran on.
        assert_eq!(
            ws.choice_for(&PickerTarget::Run(id("main"))),
            ModelChoice::new("gpt-5.5", Effort::Auto)
        );
        ws.navigate(Route::Run(id("agent-main")), cx);
        ws.submit_prompt("look at #412".into(), cx);
    });
    assert_eq!(said(&events), Some(agents_model()));
}

#[gpui::test]
fn a_model_the_person_picks_wins(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.navigate(Route::Run(id("agent-main")), cx);
        ws.open_picker(PickerTarget::Run(id("agent-main")), window, cx);
        ws.pick_effort(Effort::High, cx);
        ws.submit_prompt("think harder".into(), cx);
    });
    assert_eq!(
        said(&events),
        Some(ModelChoice::new("gpt-6-luna", Effort::High))
    );
}
