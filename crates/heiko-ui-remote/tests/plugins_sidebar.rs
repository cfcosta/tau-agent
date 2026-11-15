//! The sidebar's Plugins (ADR 0036): the plugins built in, the person's
//! own, which wait for them, and the chats of heiko's plugins repository.
//! Writing a plugin, from the sidebar or from a page with no composer,
//! starts on a new run's composer.

use gpui::{TestAppContext, VisualTestContext};
use heiko_agent::tool::RunId;
use heiko_ui_kit::theme::Tone;
use heiko_ui_plugin::{CatalogEntry, Group, Link, Note};
use heiko_ui_remote::{
    Workspace,
    catalog::{Catalog, PluginInfo, Repo},
    repos::TreeRow,
    route::Route,
    view::{Origin, RunView},
};

const OWN: &str = "heiko-plugins";
const REPO: &str = "heiko-agent";

fn id(name: &str) -> RunId {
    RunId(name.into())
}

fn plugin(name: &str, group: Group) -> PluginInfo {
    PluginInfo {
        name: name.into(),
        description: String::new(),
        reach: heiko_ui_plugin::Reach {
            seams: Vec::new(),
            ..Default::default()
        },
        spend: 0.0,
        page: None,
        group,
        note: None,
        entries: Vec::new(),
        settings: false,
        ui: true,
        rewrites_keep_transcript: false,
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
        page: Some(Link::page(heiko_luau_plugins::ui::PAGE)),
        entries: vec![
            entry("commit-style", None),
            entry("no-force-push", Some(Note::new("waits", Tone::Warn))),
            entry("test-before-stop", Some(Note::new("fails", Tone::Danger))),
        ],
        ..plugin(heiko_luau_plugins::NAME, Group::Yours)
    };
    Catalog {
        plugins: vec![
            plugin("heiko-reasoning", Group::Context),
            yours,
            plugin("heiko-tools", Group::Tools),
        ],
        repos: vec![
            Repo {
                name: OWN.into(),
                main: Some(id("plugins-main")),
                kind: Some(heiko_luau_plugins::KIND.to_owned()),
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
    install_plugins();
    cx.update(heiko_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("heiko", runs(), catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    (workspace, cx)
}

/// The titles of the plugins repository's chats the sidebar lists.
fn own_chats(ws: &Workspace) -> Vec<String> {
    let Some(rows) =
        ws.plugins_repo().and_then(|repo| ws.rows_named(&repo.name))
    else {
        return Vec::new();
    };
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
    workspace.update(&mut cx, |ws, _| {
        let catalog = ws.catalog();
        let built_in: Vec<&str> = catalog
            .built_in()
            .map(|plugin| plugin.name.as_str())
            .collect();
        assert_eq!(built_in, ["heiko-reasoning", "heiko-tools"]);
        assert_eq!(
            catalog.yours().map(|plugin| plugin.name.as_str()),
            Some(heiko_luau_plugins::NAME)
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
        // ...but its chats are its own tree's.
        assert_eq!(own_chats(ws), ["plugins", "Write commit-style"]);
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
        ws.plugin_handle(heiko_luau_plugins::NAME)
            .composer("/plugin block deploys");
        ws.drain_plugin_requests(cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.route(), &Route::NewRun);
        assert_eq!(ws.composer_text(cx), "/plugin block deploys");
    });
}

/// Gives this process every plugin heiko comes with, once.
fn install_plugins() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| heiko_ui_remote::plugins::install(heiko_plugins::plugins()));
}
