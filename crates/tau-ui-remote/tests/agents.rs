//! Agents in the sidebar (ADR 0037): a section of their own, each
//! agent's row opening its conversation and folding over its crew, and
//! their commands in every composer, which go to the agent and start
//! nothing where they were typed.

use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::json;
use tau_agent::tool::RunId;
use tau_agents::{Agent, Overview, Status, Trigger, TriggerKind};
use tau_ui_plugin::PluginValue;
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Own, Repo},
    repos::TreeRow,
    route::Route,
    view::{Origin, RunView},
    workspace::WorkspaceEvent,
};

const AGENT: &str = "pr-triage";
const NEW: &str = "deps-watch";
const REPO: &str = "tau-agent";

fn id(name: &str) -> RunId {
    RunId(name.into())
}

fn agent(name: &str, status: Status, triggers: Vec<Trigger>) -> Agent {
    Agent {
        name: name.into(),
        purpose: "Labels new pull requests.".into(),
        status,
        triggers,
        versions: Vec::new(),
        spend_today: 0.41,
        cap_today: 1.0,
        queued: 0,
        last: None,
        model: "gpt-6-luna".into(),
        effort: "low".into(),
        main: None,
        secrets: Vec::new(),
    }
}

fn overview() -> Overview {
    Overview {
        agents: vec![
            agent(NEW, Status::Waiting, Vec::new()),
            agent(
                AGENT,
                Status::Working,
                vec![Trigger {
                    name: "triage".into(),
                    kind: TriggerKind::Command {
                        command: "triage".into(),
                        hint: Some("<number>".into()),
                    },
                    last: None,
                    next: None,
                }],
            ),
        ],
    }
}

/// The person's repository, an agent with its conversation, and one the
/// host has not made a conversation for yet.
fn catalog() -> Catalog {
    let mut catalog = Catalog {
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
                ..Default::default()
            },
            Repo {
                name: NEW.into(),
                own: Some(Own::Agent),
                ..Default::default()
            },
        ],
        open_repos: vec![REPO.into()],
        ..Default::default()
    };
    catalog.plugin_data.insert(
        tau_agents::NAME.into(),
        PluginValue::from_json(serde_json::to_value(overview()).unwrap()),
    );
    catalog
}

fn runs() -> Vec<RunView> {
    vec![
        RunView::new(id("main"), "main", "coder", "gpt-5.5").in_repo(REPO),
        RunView::new(id("agent-main"), "pr-triage", "coder", "gpt-6-luna")
            .in_repo(AGENT),
        RunView::new(id("review"), "Review #409", "coder", "gpt-6-luna")
            .in_repo(AGENT)
            .with_origin(Origin::SubAgent {
                parent: id("agent-main"),
            }),
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

#[gpui::test]
fn agents_are_apart_from_the_person_s_repositories(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx);
    workspace.update(&mut cx, |ws, _| {
        let agents: Vec<&str> = ws
            .catalog()
            .agents()
            .map(|repo| repo.name.as_str())
            .collect();
        assert_eq!(agents, [NEW, AGENT]);
        let listed: Vec<&str> = ws
            .repo_rows("")
            .iter()
            .map(|rows| rows.repo.name.as_str())
            .collect();
        assert_eq!(listed, [REPO]);
        assert_eq!(
            ws.agents()
                .and_then(|overview| overview.agent(AGENT))
                .map(|a| &a.status),
            Some(&Status::Working)
        );
    });
}

#[gpui::test]
fn an_agent_s_row_opens_its_conversation(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.open_agent(AGENT, cx);
        assert_eq!(ws.route(), &Route::Run(id("agent-main")));
        // One the host has not made a conversation for opens the
        // Agents page.
        ws.open_agent(NEW, cx);
        assert_eq!(ws.route(), &Route::agents(tau_agents::ui::OVERVIEW));
    });
}

#[gpui::test]
fn an_agent_s_row_folds_over_its_crew(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(cx);
    let crew = |ws: &Workspace| -> Option<Vec<String>> {
        let rows = ws.agent_rows(AGENT)?;
        rows.open.then(|| {
            ws.repo_tree(&rows)
                .into_iter()
                .filter_map(|row| match row {
                    TreeRow::Run { run, .. } => Some(run.title.clone()),
                    TreeRow::Child { .. } => None,
                })
                .collect()
        })
    };
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(crew(ws), None);
        ws.toggle_agent_open(AGENT, cx);
        assert_eq!(
            crew(ws),
            Some(vec!["pr-triage".to_owned(), "Review #409".to_owned()])
        );
        ws.toggle_agent_open(AGENT, cx);
        assert_eq!(crew(ws), None);
        // The person's repository has no agent's rows.
        assert!(ws.agent_rows(REPO).is_none());
    });
}

#[gpui::test]
fn an_agent_s_command_goes_to_it_and_starts_nothing(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(id("main")), cx);
        ws.submit_prompt("/triage 412".into(), cx);
    });
    cx.run_until_parked();
    let events = events.borrow();
    assert!(
        !events.iter().any(|event| matches!(
            event,
            WorkspaceEvent::Say { .. } | WorkspaceEvent::NewRun { .. }
        )),
        "nothing starts where the command was typed"
    );
    let acts: Vec<&serde_json::Value> = events
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::PluginAct { plugin, action }
                if plugin == tau_agents::NAME =>
            {
                Some(action)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        acts,
        [&json!({
            "act": "command",
            "agent": AGENT,
            "command": "triage",
            "args": "412",
            "from": "main",
        })]
    );
}

#[gpui::test]
fn slash_agent_opens_the_new_agent_form(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(id("main")), cx);
        ws.submit_prompt("/agent watch the advisories feed".into(), cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, _| {
        assert_eq!(ws.route(), &Route::agents(tau_agents::ui::NEW));
    });
    assert!(
        !events
            .borrow()
            .iter()
            .any(|event| matches!(event, WorkspaceEvent::Say { .. })),
        "nothing is sent to the chat it was typed in"
    );
}
