//! tau-schedule's interface half: what a chat has waiting, in its
//! inspector, with a way to cancel each, and the message it wakes on,
//! drawn as a card. Both are view trees (ADR 0045): the time left is a
//! countdown the interface keeps, from when each comes due.

use gpui::AnyElement;
use serde_json::Value;
use tau_ui_plugin::{
    Manifest,
    UiPlugin,
    ViewCx,
    points::{self, AtMessage, AtRun},
    tree::{Node, Tone},
};

use crate::{Act, NAME, Overview, Wake, Woken};

/// tau-schedule's interface half.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScheduleUi;

impl UiPlugin for ScheduleUi {
    type State = ();
    type Data = Overview;
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute_at(points::INSPECTOR, -150, panel)
            .contribute(points::USER_MESSAGE, card)
    }
}

/// The button's action: cancel the wake its arguments name.
const CANCEL: &str = "cancel";

/// An icon of tau's, by name.
fn icon(name: &str, tone: Tone) -> Node {
    Node::Icon {
        name: name.into(),
        tone,
    }
}

/// What the chat has waiting, soonest first, each with when and a way to
/// cancel it; nothing when none.
fn panel(at: &AtRun, view: &mut ViewCx<'_, ScheduleUi>) -> Option<AnyElement> {
    let run = at.run.id.to_string();
    let waiting: Vec<Wake> = view.data.of_run(&run).cloned().collect();
    if waiting.is_empty() {
        return None;
    }
    let tree = Node::stack(
        std::iter::once(Node::Heading {
            text: "Scheduled".into(),
        })
        .chain(waiting.iter().map(row)),
    );
    let handle = view.handle.clone();
    Some(view.tree(
        &format!("scheduled-{run}"),
        &tree,
        move |action, args, cx| {
            if action != CANCEL {
                return;
            }
            if let Ok(act) = serde_json::from_value::<Act>(args.clone()) {
                handle.act(act, cx);
            }
        },
    ))
}

/// One wake: its name, when it comes due, the button that cancels it,
/// and its note.
fn row(wake: &Wake) -> Node {
    let cancel = Act::Cancel {
        run: wake.run.clone(),
        name: wake.name.clone(),
    };
    Node::stack([
        Node::row([
            icon("clock", Tone::Neutral),
            Node::text(wake.name.clone()),
            Node::Spacer,
            Node::Due {
                id: format!("{}-{}", wake.run, wake.name),
                at: wake.at.saturating_mul(1000),
            },
            Node::Button {
                label: String::new(),
                icon: Some("close".into()),
                action: CANCEL.into(),
                args: serde_json::to_value(cancel).unwrap_or(Value::Null),
                kind: Default::default(),
            },
        ]),
        Node::text(wake.note.clone()),
    ])
}

/// The message a chat woke on, as a card: each wake-up that came due,
/// its name, when, and its note.
fn card(
    at: &AtMessage,
    view: &mut ViewCx<'_, ScheduleUi>,
) -> Option<AnyElement> {
    let woken = Woken::parse(&at.text)?;
    let mut blocks = Vec::new();
    for (index, woken) in woken.into_iter().enumerate() {
        if index > 0 {
            blocks.push(Node::Rule);
        }
        blocks.push(Node::stack([
            Node::row([
                icon("clock", Tone::Info),
                Node::text(format!("Scheduled · {}", woken.name)),
                Node::Spacer,
                Node::Mono { text: woken.due },
            ]),
            Node::text(woken.note),
        ]));
    }
    let tree = Node::Card {
        children: vec![Node::stack(blocks)],
    };
    Some(view.tree(&format!("woken-{}", at.index), &tree, |_, _, _| {}))
}
