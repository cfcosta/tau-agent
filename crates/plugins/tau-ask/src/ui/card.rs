//! The card of an `ask` call: what it asked, then each answer with its
//! note, read from the call's own arguments and result so a stored run
//! shows it as the live one did. A view tree (ADR 0045).

use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    CallData,
    ViewCx,
    points::{AtCard, CardView},
    tree::{self, Node},
};

use super::AskUi;
use crate::{Ask, Reply, TOOL};

/// What the card shows: the questions, and the reply once there is one.
struct Shown {
    ask: Option<Ask>,
    reply: Option<Reply>,
    /// The call failed: cancelled, or refused questions.
    failed: Option<String>,
}

fn shown(data: &CallData) -> Shown {
    let result = data.result.as_ref();
    Shown {
        ask: serde_json::from_value(data.args.clone()).ok(),
        reply: result
            .filter(|result| !result.error)
            .and_then(|result| result.details.clone())
            .and_then(|details| serde_json::from_value(details).ok()),
        failed: result.filter(|result| result.error).map(|result| {
            result.text.lines().next().unwrap_or_default().to_owned()
        }),
    }
}

/// The card of a call to `ask`.
pub fn card(at: &AtCard, _view: &mut ViewCx<'_, AskUi>) -> Option<CardView> {
    if at.tool != TOOL {
        return None;
    }
    let shown = shown(&at.data);
    let n = shown.ask.as_ref().map_or(0, |ask| ask.questions.len());
    let label = match (&shown.reply, &shown.failed) {
        (Some(Reply::Answered { .. }), _) => format!("{n} answered"),
        (Some(Reply::Declined), _) => "declined".to_owned(),
        (None, Some(_)) => "not answered".to_owned(),
        (None, None) if at.run.live => "waiting for you".to_owned(),
        (None, None) => "not answered".to_owned(),
    };
    let head = shown.ask.as_ref().map(|ask| {
        let headers: Vec<&str> =
            ask.questions.iter().map(|q| q.header.as_str()).collect();
        Node::Mono {
            text: format!("ask · {}", headers.join(" · ")),
        }
    });
    let waiting =
        shown.reply.is_none() && shown.failed.is_none() && at.run.live;
    let body = shown
        .ask
        .as_ref()
        .map(|ask| body(ask, shown.reply.as_ref(), waiting));
    Some(CardView {
        head,
        label: Some(label),
        failed: shown.failed.clone(),
        edge: if waiting {
            Some(Tone::Warn)
        } else {
            shown.failed.as_ref().map(|_| Tone::Danger)
        },
        shape: None,
        body,
        folds: false,
        inset: false,
    })
}

/// Each question's header and its answer, with the note under it.
pub fn body(ask: &Ask, reply: Option<&Reply>, waiting: bool) -> Node {
    let answers = match reply {
        Some(Reply::Answered { answers }) => Some(answers),
        _ => None,
    };
    let mut rows: Vec<Vec<Node>> = Vec::new();
    for (i, question) in ask.questions.iter().enumerate() {
        let answer = answers.and_then(|answers| answers.get(i));
        let mut said = vec![match answer {
            Some(answer) => Node::Rich {
                text: format!("**{}**", answer.text()),
            },
            None => Node::Toned {
                text: question.question.clone(),
                tone: tree::Tone::Neutral,
                mono: false,
            },
        }];
        if let Some(note) = answer.and_then(|answer| answer.note.clone()) {
            said.push(Node::row([
                Node::Icon {
                    name: "pencil".into(),
                    tone: tree::Tone::Info,
                },
                Node::Toned {
                    text: note,
                    tone: tree::Tone::Neutral,
                    mono: false,
                },
            ]));
        }
        rows.push(vec![
            Node::Toned {
                text: question.header.clone(),
                tone: tree::Tone::Neutral,
                mono: false,
            },
            Node::stack(said),
        ]);
    }
    let mut pieces = vec![Node::Table {
        head: Vec::new(),
        rows,
        widths: vec![1., 4.],
    }];
    if waiting {
        pieces.push(Node::Toned {
            text: "Answer in the panel below.".into(),
            tone: tree::Tone::Info,
            mono: false,
        });
    }
    if matches!(reply, Some(Reply::Declined)) {
        pieces.push(Node::Toned {
            text: "You declined to answer.".into(),
            tone: tree::Tone::Neutral,
            mono: false,
        });
    }
    Node::stack(pieces)
}
