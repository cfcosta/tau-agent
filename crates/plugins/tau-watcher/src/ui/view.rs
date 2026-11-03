//! The note as the transcript draws it, and as the line above the
//! composer: both are view trees (ADR 0045).

use gpui::AnyElement;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_plugin::{
    Handle,
    RunInfo,
    ViewCx,
    points::{AtAnchor, AtRun},
    tree::{Node, Tone},
};

use super::WatcherUi;
use crate::{
    cadence::chat_text,
    record::{Answer, Record},
    state::{Note, Status},
};

/// The buttons' action: answer the note its arguments name.
const ANSWER: &str = "answer";

/// What a button's answer carries: the note's key, the answer, and the
/// note's line, for the composer when the answer is a chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Answered {
    key: String,
    answer: Answer,
    line: String,
}

/// The note under the step that asked for it: its line, what the person
/// can do about it, and the explanation once they asked for it.
pub fn annotation(
    at: &AtAnchor,
    view: &mut ViewCx<'_, WatcherUi>,
) -> Option<AnyElement> {
    let note = view
        .state?
        .notes
        .iter()
        .find(|note| note.key == at.key)?
        .clone();
    let muted = matches!(note.status, Status::Knew | Status::Dismissed);
    let mut children =
        vec![Node::row([tag(&note, muted), Node::text(&note.line)])];
    if note.status == Status::Learned
        && let Some(explain) = note.explain.clone()
    {
        children.push(Node::stack([
            Node::Rich {
                text: format!("**{}**", explain.title),
            },
            Node::List {
                items: explain.bullets.into_iter().map(Node::text).collect(),
            },
        ]));
    }
    if note.status == Status::Knew {
        children.push(Node::text("You knew this."));
    }
    let actions = actions(&note);
    if !actions.is_empty() {
        children.push(Node::row(actions));
    }
    let tree = Node::Card { children };
    let id = format!("watcher-{}-{}", at.run.id.0, at.key);
    Some(draw(view, &id, &tree, &at.run))
}

/// The newest note as one line above the composer, while it is new.
pub fn band(
    at: &AtRun,
    view: &mut ViewCx<'_, WatcherUi>,
) -> Option<AnyElement> {
    let note = view.state?.band()?.clone();
    let tree = Node::row(
        [tag(&note, false), Node::text(&note.line), Node::Spacer]
            .into_iter()
            .chain(actions(&note)),
    );
    let id = format!("watcher-band-{}", at.run.id.0);
    Some(draw(view, &id, &tree, &at.run))
}

/// Draws a note's tree, its buttons answering it in `run`.
fn draw(
    view: &mut ViewCx<'_, WatcherUi>,
    id: &str,
    tree: &Node,
    run: &RunInfo,
) -> AnyElement {
    let (handle, run) = (view.handle.clone(), run.clone());
    view.tree(id, tree, move |action, args, cx| {
        if action != ANSWER {
            return;
        }
        if let Ok(answered) = serde_json::from_value::<Answered>(args.clone()) {
            act(&handle, &run, answered, cx);
        }
    })
}

/// The note's tag, in amber (grey once answered), ahead of its line.
fn tag(note: &Note, muted: bool) -> Node {
    Node::Badge {
        text: note.tag.label().into(),
        tone: if muted { Tone::Neutral } else { Tone::Warn },
    }
}

/// The buttons a note gets: all of them while it is new, and Knew and
/// Chat once its explanation is open.
fn actions(note: &Note) -> Vec<Node> {
    let mut wanted = Vec::new();
    if note.status == Status::New && note.explain.is_some() {
        wanted.push(("Learn more", Answer::Learned));
    }
    if matches!(note.status, Status::New | Status::Learned) {
        wanted.push(("Knew this already", Answer::Knew));
        wanted.push(("Chat about it", Answer::Chatted));
    }
    if note.status == Status::New {
        wanted.push(("Dismiss", Answer::Dismissed));
    }
    wanted
        .into_iter()
        .map(|(label, answer)| {
            let answered = Answered {
                key: note.key.clone(),
                answer,
                line: note.line.clone(),
            };
            Node::Button {
                label: label.into(),
                action: ANSWER.into(),
                args: serde_json::to_value(answered).unwrap_or(Value::Null),
                kind: Default::default(),
                icon: None,
            }
        })
        .collect()
}

/// Stores the person's answer, which folds into the state as any record
/// does, and for a chat puts the note in the composer.
fn act(handle: &Handle, run: &RunInfo, answered: Answered, cx: &mut gpui::App) {
    if answered.answer == Answer::Chatted {
        handle.composer(chat_text(&answered.line), cx);
    }
    handle.record(
        &run.id,
        Record::Answered {
            key: answered.key,
            answer: answered.answer,
        },
        cx,
    );
}
