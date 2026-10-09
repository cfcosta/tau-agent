//! A chat that landed by itself and went on (ADR 0034): its card shows in
//! its parent's chat, in place of the fork waiting there, and in its own,
//! and the chat stays open.

use gpui::{TestAppContext, VisualTestContext};
use tau_agent::{event::StopReason, tool::RunId};
use tau_ui_remote::{
    Workspace,
    catalog::Catalog,
    update::HostUpdate,
    view::{Item, LandingRecord, Origin, RunView},
};

fn id(name: &str) -> RunId {
    RunId(name.into())
}

#[gpui::test]
fn a_chat_that_landed_itself_shows_it_and_stays_open(cx: &mut TestAppContext) {
    let mut main = RunView::new(id("main"), "main", "coder", "gpt-5.5");
    main.finish_stored(StopReason::Stop, 0.0, 0.0);
    main.fork_finished(&id("chat"));
    let mut chat = RunView::new(id("chat"), "greet plugin", "coder", "gpt-5.5")
        .with_origin(Origin::Fork {
            from: id("main"),
            turn: 0,
        });
    chat.finish_stored(StopReason::Stop, 0.0, 0.0);
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", vec![main, chat], Catalog::default(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let record = LandingRecord {
        from: "chat".into(),
        title: "greet plugin".into(),
        landing: tau_vcs::Landing {
            changes: Vec::new(),
            conflicts: Vec::new(),
            head: "0".repeat(40),
        },
        recovered: false,
        kept: true,
    };
    workspace.update(&mut cx, |ws, cx| {
        ws.apply(HostUpdate::LandedItself(record), cx);
        let items = |run: &str| ws.run(&id(run)).unwrap().items.clone();
        let landed = |items: &[Item]| {
            items
                .iter()
                .filter(|item| matches!(item, Item::Landed(card) if card.kept))
                .count()
        };
        assert_eq!(landed(&items("main")), 1);
        assert!(
            !items("main")
                .iter()
                .any(|item| matches!(item, Item::ForkReady { .. })),
            "it no longer waits to land"
        );
        assert_eq!(landed(&items("chat")), 1);
        assert_eq!(ws.run(&id("chat")).unwrap().ending, None);
    });
}
