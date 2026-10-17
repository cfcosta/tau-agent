//! Memory's import as the interface draws it, from memory's catalog data
//! alone (ADR 0044): its step in first-run setup, there only when Claude
//! Code or Codex is on this computer; the review page of what an import
//! proposes; and the Memory tab's way in and its banner.

use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, size};
use tau_memory::{
    import::{
        Act,
        FolderFound,
        Imports,
        Parts,
        Place,
        Request,
        Tool,
        ToolFound,
    },
    ui::MemoryData,
};
use tau_ui_plugin::PluginValue;
use tau_ui_remote::{
    Workspace,
    catalog::Catalog,
    plugins,
    route::Route,
    setup::{ModelAccess, RepoChoice, Setup, SetupStep},
    workspace::WorkspaceEvent,
};

const MEMORY: &str = tau_memory::NAME;

type Events = Rc<RefCell<Vec<WorkspaceEvent>>>;

/// What Claude Code and Codex have, as memory's host half would say.
fn found() -> Vec<ToolFound> {
    let hello = Place {
        folder: Some("/home/p/work/hello".into()),
        remote: Some("git@github.com:octocat/hello.git".into()),
    };
    vec![
        ToolFound {
            tool: Tool::ClaudeCode,
            saved: 5,
            chats: 3,
            about_you: 1,
            folders: vec![
                FolderFound {
                    place: hello.clone(),
                    label: "~/work/hello".into(),
                    saved: 3,
                    chats: 2,
                    unread_saved: 3,
                    unread_chats: 2,
                },
                FolderFound {
                    place: Place {
                        folder: Some("/home/p/work/other".into()),
                        remote: None,
                    },
                    label: "~/work/other".into(),
                    saved: 1,
                    chats: 1,
                    unread_saved: 1,
                    unread_chats: 1,
                },
            ],
        },
        ToolFound {
            tool: Tool::Codex,
            saved: 2,
            chats: 4,
            about_you: 0,
            folders: vec![FolderFound {
                place: hello,
                label: "~/work/hello".into(),
                saved: 2,
                chats: 4,
                unread_saved: 2,
                unread_chats: 4,
            }],
        },
    ]
}

fn data(imports: Imports) -> PluginValue {
    PluginValue::from_json(
        serde_json::to_value(MemoryData {
            imports,
            ..MemoryData::default()
        })
        .unwrap(),
    )
}

fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| plugins::install(plugins::plugins()));
}

/// A desktop window whose catalog holds `imports` as memory's data.
fn open(
    imports: Imports,
    cx: &mut TestAppContext,
) -> (Entity<Workspace>, VisualTestContext, Events) {
    install();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let mut catalog = Catalog::default();
        catalog.plugin_data.insert(MEMORY.into(), data(imports));
        Workspace::new("tau", Vec::new(), catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let events: Events = Rc::default();
    let seen = events.clone();
    cx.update(|cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    // Tall enough that setup's step needs no scrolling.
    cx.simulate_resize(size(gpui::px(1440.), gpui::px(1600.)));
    (workspace, cx, events)
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// What memory's UI asked its host half, in order.
fn acts(events: &Events) -> Vec<Act> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            WorkspaceEvent::PluginAct { plugin, action }
                if plugin == MEMORY =>
            {
                Some(serde_json::from_value::<Act>(action.clone()).unwrap())
            }
            _ => None,
        })
        .collect()
}

/// Onboarding with one repository picked, on the repositories' step.
fn at_repositories(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update(cx, |ws, cx| {
        ws.set_setup(
            Setup {
                model: ModelAccess::Connected { label: "m".into() },
                repos: vec![RepoChoice {
                    name: "octocat/hello".into(),
                    description: String::new(),
                    branch: "main".into(),
                    selected: true,
                    pushed_at: String::new(),
                    remote: Some("https://github.com/octocat/hello".into()),
                }],
                ..Setup::default()
            },
            cx,
        );
        ws.start_setup(SetupStep::Repos, cx);
    });
}

fn stage_names(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    workspace.update(cx, |ws, cx| {
        ws.setup_stages(cx)
            .into_iter()
            .map(|stage| stage.name)
            .collect()
    })
}

fn route(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Route {
    workspace.read_with(cx, |ws, _| ws.route().clone())
}

/// Neither tool on this computer: setup has no Memories step.
#[gpui::test]
fn the_setup_step_is_absent_without_the_tools(cx: &mut TestAppContext) {
    let (workspace, mut cx, _) = open(Imports::default(), cx);
    at_repositories(&workspace, &mut cx);
    assert_eq!(
        stage_names(&workspace, &mut cx),
        ["GitHub", "Model", "Repositories", "Ready"]
    );
}

/// With them, the step comes after the repositories. Its parts start
/// ticked; unticking Codex's chats and starting asks memory to import
/// what is left, for the picked repository's place, and ends the step.
#[gpui::test]
fn the_setup_step_starts_an_import_of_what_was_ticked(cx: &mut TestAppContext) {
    let (workspace, mut cx, events) = open(
        Imports {
            found: found(),
            ..Imports::default()
        },
        cx,
    );
    at_repositories(&workspace, &mut cx);
    assert_eq!(
        stage_names(&workspace, &mut cx),
        ["GitHub", "Model", "Repositories", "Memories", "Ready"]
    );
    workspace.update(&mut cx, |ws, cx| ws.clone_selected(cx));
    assert_eq!(
        route(&workspace, &mut cx),
        Route::Setup(SetupStep::Plugin {
            plugin: MEMORY,
            step: "memories"
        })
    );
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("setup-reach").is_some(),
        "it says what it reads"
    );
    click("memory-setup-codex-chats", &mut cx);
    click("memory-setup-others", &mut cx);
    click("memory-setup-start", &mut cx);
    assert_eq!(
        acts(&events),
        [Act::Import(Request {
            parts: Parts {
                codex_chats: false,
                ..Parts::default()
            },
            only: vec![Place {
                folder: None,
                remote: Some("https://github.com/octocat/hello".into()),
            }],
            others: true,
        })]
    );
    assert_eq!(route(&workspace, &mut cx), Route::Setup(SetupStep::Ready));
}
