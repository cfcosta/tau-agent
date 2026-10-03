//! tau-direnv in the workspace: the question takes the composer's place
//! while a live run is asked, a card shows above the transcript while
//! the environment loads or after it failed, and the repository's menu
//! has the toggle only for a repository with an `.envrc`.

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_direnv::{NAME, Record, RepoData};
use tau_ui::demo;
use tau_ui_plugin::{
    PluginValue,
    points::{self, AtRepo, AtRun},
};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    route::Route,
    update::HostUpdate,
};

fn open(
    cx: &mut TestAppContext,
    data: RepoData,
) -> (Entity<Workspace>, VisualTestContext) {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let mut repo = Repo {
            name: "tau-agent".into(),
            main: Some(demo::run_id()),
            ..Default::default()
        };
        repo.plugins.insert(NAME.into(), PluginValue::typed(data));
        let catalog = Catalog {
            repos: vec![repo],
            ..Default::default()
        };
        Workspace::new("tau", vec![demo::retry_after()], catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    (workspace, VisualTestContext::from_window(window.into(), cx))
}

fn fold(record: &Record) -> HostUpdate {
    HostUpdate::PluginFold {
        run: demo::run_id(),
        plugin: NAME.into(),
        body: serde_json::to_value(record).unwrap(),
    }
}

/// How many contributions tau-direnv gives the run in the composer's
/// place and above its transcript.
fn drawn(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> (usize, usize) {
    workspace.update(cx, |ws, cx| {
        let at = AtRun {
            run: ws.run(&demo::run_id()).unwrap().info(),
        };
        (
            ws.contributions_of(NAME, points::COMPOSER, &at, cx).len(),
            ws.contributions_of(NAME, points::RUN_BANNER, &at, cx).len(),
        )
    })
}

#[gpui::test]
fn the_question_and_the_cards_follow_the_records(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(
        cx,
        RepoData {
            envrc: true,
            direnv: true,
        },
    );
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    assert_eq!(
        drawn(&workspace, &mut cx),
        (0, 0),
        "nothing before a record"
    );

    let cases = [
        (
            Record::Asked {
                repo: "tau-agent".into(),
                envrc: "use flake\n".into(),
            },
            (1, 0),
        ),
        (Record::Loading { since: 0 }, (0, 1)),
        (Record::Loaded, (0, 0)),
        (
            Record::Failed {
                status: "direnv exited 1".into(),
                output: "error: no devShell".into(),
            },
            (0, 1),
        ),
        (Record::Denied, (0, 0)),
        (Record::Off, (0, 0)),
    ];
    for (record, expected) in cases {
        workspace.update(&mut cx, |ws, cx| ws.apply(fold(&record), cx));
        cx.run_until_parked();
        assert_eq!(drawn(&workspace, &mut cx), expected, "{record:?}");
    }
}

#[gpui::test]
fn the_menu_has_the_toggle_for_a_repository_with_an_envrc(
    cx: &mut TestAppContext,
) {
    for (data, entries) in [
        (
            RepoData {
                envrc: true,
                direnv: true,
            },
            1,
        ),
        (
            RepoData {
                envrc: true,
                direnv: false,
            },
            1,
        ),
        (
            RepoData {
                envrc: false,
                direnv: true,
            },
            0,
        ),
    ] {
        let (workspace, mut cx) = open(cx, data.clone());
        let drawn = workspace.update(&mut cx, |ws, cx| {
            ws.contributions_of(
                NAME,
                points::REPO_MENU,
                &AtRepo {
                    repo: "tau-agent".into(),
                },
                cx,
            )
            .len()
        });
        assert_eq!(drawn, entries, "{data:?}");
    }
}
