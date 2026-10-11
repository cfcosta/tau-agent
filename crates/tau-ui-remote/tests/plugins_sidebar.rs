//! The sidebar's Plugins (ADR 0036): the plugins built in, the person's
//! own, which wait for them, and the chats of tau's plugins repository,
//! folded away with Plugins. Writing a plugin, from the sidebar or from
//! a page with no composer, starts on a new run's composer.

use gpui::{TestAppContext, VisualTestContext};
use tau_agent::tool::RunId;
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{CatalogEntry, Group, Link, Note};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Own, PluginInfo, Repo},
    repos::TreeRow,
    route::Route,
    view::{Origin, RunView},
};

const OWN: &str = "tau-plugins";
const REPO: &str = "tau-agent";

fn id(name: &str) -> RunId {
    RunId(name.into())
}

fn plugin(name: &str, group: Group) -> PluginInfo {
    PluginInfo {
        name: name.into(),
        description: String::new(),
        seams: Vec::new(),
        spend: 0.0,
        page: None,
        group,
        note: None,
        entries: Vec::new(),
        settings: false,
    }
}

fn entry(name: &str, note: Option<Note>) -> CatalogEntry {
    CatalogEntry {
        name: name.into(),
        description: String::new(),
        group: Group::Yours,
        seams: Vec::new(),
        note,
    }
}

/// Two plugins built in, and the person's own three: one active, one
/// waiting for them, one whose tests fail.
fn catalog() -> Catalog {
    let yours = PluginInfo {
        page: Some(Link::page(tau_luau_plugins::ui::PAGE)),
        entries: vec![
            entry("commit-style", None),
            entry("no-force-push", Some(Note::new("waits", Tone::Warn))),
            entry("test-before-stop", Some(Note::new("fails", Tone::Danger))),
        ],
        ..plugin(tau_luau_plugins::NAME, Group::Yours)
    };
    Catalog {
        plugins: vec![
            plugin("tau-reasoning", Group::Context),
            yours,
            plugin("tau-tools", Group::Tools),
        ],
        repos: vec![
            Repo {
                name: OWN.into(),
                main: Some(id("plugins-main")),
                own: Some(Own::Plugins),
                ..Default::default()
            },
            Repo {
                name: REPO.into(),
                main: Some(id("main")),
                ..Default::default()
            },
        ],
        open_repos: vec![REPO.into()],
        ..Default::default()
    }
}

fn runs() -> Vec<RunView> {
    vec![
        RunView::new(id("plugins-main"), "plugins", "coder", "gpt-5.5")
            .in_repo(OWN),
        RunView::new(id("commit-style"), "Write commit-style", "coder", "m")
            .in_repo(OWN)
            .with_origin(Origin::Fork {
                from: id("plugins-main"),
                turn: 0,
            }),
        RunView::new(id("main"), "main", "coder", "gpt-5.5").in_repo(REPO),
    ]
}

fn open(
    cx: &mut TestAppContext,
) -> (gpui::Entity<Workspace>, VisualTestContext) {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs(), catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    (workspace, cx)
}

/// The titles of the plugins repository's chats the sidebar lists.
fn own_chats(ws: &Workspace) -> Vec<String> {
    let Some(rows) = ws.own_rows() else {
        return Vec::new();
    };
    if !rows.open {
        return Vec::new();
    }
    ws.repo_tree(&rows)
        .into_iter()
        .filter_map(|row| match row {
            TreeRow::Run { run, .. } => Some(run.title.clone()),
            TreeRow::Child { .. } => None,
        })
        .collect()
}

#[gpui::test]
fn plugins_lists_the_built_in_the_person_s_own_and_their_chats(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        let catalog = ws.catalog();
        let built_in: Vec<&str> = catalog
            .built_in()
            .map(|plugin| plugin.name.as_str())
            .collect();
        assert_eq!(built_in, ["tau-reasoning", "tau-tools"]);
        assert_eq!(
            catalog.yours().map(|plugin| plugin.name.as_str()),
            Some(tau_luau_plugins::NAME)
        );
        // One waits for the person, one fails its tests: both need them.
        assert_eq!(catalog.yours_need_you(), 2);
        // The plugins repository is not one of the person's...
        let listed: Vec<String> = ws
            .repo_rows("")
            .iter()
            .map(|rows| rows.repo.name.clone())
            .collect();
        assert_eq!(listed, [REPO]);
        // ...but its chats are under Plugins, until it is folded.
        assert_eq!(own_chats(ws), ["plugins", "Write commit-style"]);
        ws.toggle_plugins_open(cx);
        assert!(own_chats(ws).is_empty());
        ws.toggle_plugins_open(cx);
        assert_eq!(own_chats(ws), ["plugins", "Write commit-style"]);
    });
}

#[gpui::test]
fn the_person_s_plugins_and_each_of_them_have_a_page(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx);
    workspace.update(&mut cx, |ws, _| {
        let page = |entry: Option<&str>| Route::Plugin {
            plugin: tau_luau_plugins::NAME.into(),
            page: tau_luau_plugins::ui::PAGE.into(),
            params: entry
                .map(|entry| {
                    (tau_ui_plugin::ENTRY.to_owned(), entry.to_owned())
                })
                .into_iter()
                .collect(),
        };
        assert_eq!(ws.yours_route(None), Some(page(None)));
        assert_eq!(
            ws.yours_route(Some("no-force-push")),
            Some(page(Some("no-force-push")))
        );
    });
}

#[gpui::test]
fn writing_a_plugin_starts_on_a_new_run_s_composer(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx);
    workspace.update_in(&mut cx, |ws, window, cx| {
        ws.navigate(Route::Plugins, cx);
        ws.write_plugin(window, cx);
        assert_eq!(ws.route(), &Route::NewRun);
        assert_eq!(ws.composer_text(cx), "/plugin ");
    });
    // A plugin's page asks for the composer: it is a new run's too.
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(ws.yours_route(None).unwrap(), cx);
        ws.plugin_handle(tau_luau_plugins::NAME)
            .composer("/plugin block deploys", cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.route(), &Route::NewRun);
        assert_eq!(ws.composer_text(cx), "/plugin block deploys");
    });
}
