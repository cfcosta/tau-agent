//! tau-watcher in the workspace: a note is a line above the composer
//! while it is new, and an annotation in the transcript for good.

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_ui::demo;
use tau_ui_plugin::points::{self, AtAnchor, AtRun};
use tau_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    route::Route,
    update::HostUpdate,
};
use tau_watcher::{
    NAME,
    Record,
    record::{Answer, Tag},
};

fn open(cx: &mut TestAppContext) -> (Entity<Workspace>, VisualTestContext) {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let repo = Repo {
            name: "tau-agent".into(),
            main: Some(demo::run_id()),
            ..Default::default()
        };
        let catalog = Catalog {
            repos: vec![repo],
            ..Default::default()
        };
        Workspace::new("tau", vec![demo::retry_after()], catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    (workspace, VisualTestContext::from_window(window.into(), cx))
}

fn fold(
    workspace: &Entity<Workspace>,
    record: &Record,
    cx: &mut VisualTestContext,
) {
    let update = HostUpdate::PluginFold {
        run: demo::run_id(),
        plugin: NAME.into(),
        body: serde_json::to_value(record).unwrap(),
    };
    workspace.update(cx, |ws, cx| ws.apply(update, cx));
    cx.run_until_parked();
}

/// Contributions in the band above the composer, and in the transcript
/// at the note's anchor.
fn drawn(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> (usize, usize) {
    workspace.update(cx, |ws, cx| {
        let run = ws.run(&demo::run_id()).unwrap().info();
        let anchor = AtAnchor {
            run: run.clone(),
            key: "n0".into(),
            index: 0,
        };
        (
            ws.contributions_of(
                NAME,
                points::COMPOSER_BAND,
                &AtRun { run },
                cx,
            )
            .len(),
            ws.contributions_of(NAME, points::TRANSCRIPT, &anchor, cx)
                .len(),
        )
    })
}

#[gpui::test]
fn a_note_is_a_band_while_new_and_an_annotation_always(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    assert_eq!(
        drawn(&workspace, &mut cx),
        (0, 0),
        "nothing before a record"
    );

    let note = Record::Noted {
        step: 6,
        tag: Tag::HeadsUp,
        line: "The key expires tomorrow.".into(),
        explain: None,
    };
    let steps = [
        (note, (1, 1)),
        // Written past once: still a band. Twice: only the annotation.
        (Record::TypedPast, (1, 1)),
        (Record::TypedPast, (0, 1)),
    ];
    for (record, expected) in steps {
        fold(&workspace, &record, &mut cx);
        assert_eq!(drawn(&workspace, &mut cx), expected, "{record:?}");
    }
}

#[gpui::test]
fn acting_on_a_note_takes_the_band_away(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    fold(
        &workspace,
        &Record::Noted {
            step: 6,
            tag: Tag::YouShouldKnow,
            line: "Tests run in debug.".into(),
            explain: None,
        },
        &mut cx,
    );
    assert_eq!(drawn(&workspace, &mut cx), (1, 1));
    fold(
        &workspace,
        &Record::Answered {
            key: "n0".into(),
            answer: Answer::Knew,
        },
        &mut cx,
    );
    assert_eq!(drawn(&workspace, &mut cx), (0, 1));
}
