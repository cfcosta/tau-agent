//! tau's own repositories, like its plugins repository (ADR 0027), are
//! in the catalog so their chats work, but the person never sees them as
//! a repository: the sidebar, search and new runs pass them by, even
//! while one of their chats is open.

use gpui::{TestAppContext, VisualTestContext};
use tau_agent::tool::RunId;
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    route::Route,
    search::Pick,
    view::RunView,
};

const OWN: &str = "tau-plugins";
const REPO: &str = "tau-agent";

fn id(name: &str) -> RunId {
    RunId(name.into())
}

fn catalog() -> Catalog {
    Catalog {
        // tau's own is listed first, as a first start leaves it.
        repos: vec![
            Repo {
                name: OWN.into(),
                main: Some(id("plugins-main")),
                own: true,
                ..Default::default()
            },
            Repo {
                name: REPO.into(),
                main: Some(id("main")),
                ..Default::default()
            },
        ],
        open_repos: vec![OWN.into(), REPO.into()],
        ..Default::default()
    }
}

fn runs() -> Vec<RunView> {
    vec![
        RunView::new(id("plugins-main"), "plugins", "coder", "gpt-5.5")
            .in_repo(OWN),
        RunView::new(id("main"), "main", "coder", "gpt-5.5").in_repo(REPO),
    ]
}

#[gpui::test]
fn tau_s_own_repositories_are_not_listed(cx: &mut TestAppContext) {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs(), catalog(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let listed = |ws: &Workspace| -> Vec<String> {
        ws.repo_rows("")
            .iter()
            .map(|rows| rows.repo.name.clone())
            .collect()
    };
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(listed(ws), [REPO]);
        assert_eq!(ws.selected_repo(), Some(REPO));
        // Its main chat is still one.
        assert!(ws.is_main(&id("plugins-main")));
        let repos: Vec<Pick> = ws
            .search_hits("", cx)
            .into_iter()
            .map(|hit| hit.pick)
            .filter(|pick| matches!(pick, Pick::Repo(_) | Pick::NewRunIn(_)))
            .collect();
        assert_eq!(
            repos,
            [Pick::Repo(REPO.into()), Pick::NewRunIn(REPO.into())]
        );
        // Opening one of its chats neither lists nor selects it.
        ws.navigate(Route::Run(id("plugins-main")), cx);
    });
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, _| {
        assert_eq!(listed(ws), [REPO]);
        assert_eq!(ws.selected_repo(), Some(REPO));
    });
}
