//! The card of a `skill` call: the skill's name and what it is for,
//! then, folded, its file and the instructions the model read, from the
//! call's own arguments and result, so a stored run shows it as the live
//! one did. Both are view trees (ADR 0045).

use tau_ui_kit::theme::Tone as Edge;
use tau_ui_plugin::{
    ViewCx,
    points::{AtCard, CardView},
    tree::{Node, Tone},
};

use super::SkillsUi;
use crate::{Loaded, TOOL};

/// The card of a call to `skill`.
pub fn card(at: &AtCard, _view: &mut ViewCx<'_, SkillsUi>) -> Option<CardView> {
    if at.tool != TOOL {
        return None;
    }
    let name = at.data.args["name"].as_str().unwrap_or_default().to_owned();
    let result = at.data.result.as_ref();
    let loaded: Option<Loaded> = result
        .filter(|result| !result.error)
        .and_then(|result| result.details.clone())
        .and_then(|details| serde_json::from_value(details).ok());
    let failed = result.filter(|result| result.error).map(|result| {
        result.text.lines().next().unwrap_or_default().to_owned()
    });
    let mut head = vec![
        Node::Icon {
            name: "skill".into(),
            tone: if failed.is_some() {
                Tone::Bad
            } else {
                Tone::Info
            },
        },
        Node::Mono { text: name },
    ];
    if let Some(loaded) = &loaded {
        head.push(Node::text(loaded.description.clone()));
    }
    // What the model read: the result after its line naming the folder.
    let instructions = result
        .filter(|result| !result.error)
        .and_then(|result| result.text.split_once("\n\n"))
        .map(|(_, body)| body.to_owned());
    let body = loaded.map(|loaded| {
        Node::stack(
            std::iter::once(Node::Mono {
                text: loaded.file.display().to_string(),
            })
            .chain(instructions.map(Node::text)),
        )
    });
    Some(CardView {
        head: Some(Node::row(head)),
        label: None,
        edge: failed.as_ref().map(|_| Edge::Danger),
        failed,
        shape: None,
        body,
        folds: true,
        inset: false,
    })
}
