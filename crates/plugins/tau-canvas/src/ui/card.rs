//! The cards of the canvas tools: the revision a call made, small, and
//! a click away from the canvas showing it. A card reads the revision
//! from the run's canvas by its call, so a stored run shows it as the
//! live one did. Each is a view tree (ADR 0045), but for an image's
//! thumbnail.

use gpui::{div, prelude::*, rems};
use tau_ui_kit::theme::{Tone, sp};
use tau_ui_plugin::{
    ViewCx,
    points::{AtCard, CardView},
    tree::{Kind, Node},
};

use super::{CanvasUi, OPEN, Pick, image, on_action, pane::picture};
use crate::{Content, READ, TOOLS, words};

/// How tall an image's thumbnail is.
const THUMBNAIL: f32 = 10.;

/// The lines of a document a card shows.
const EXCERPT_LINES: usize = 6;

/// The card of a call to a canvas tool.
pub fn card(at: &AtCard, view: &mut ViewCx<'_, CanvasUi>) -> Option<CardView> {
    if !TOOLS.contains(&at.tool.as_str()) {
        return None;
    }
    let result = at.data.result.as_ref();
    let failed = result.filter(|result| result.error).map(|result| {
        result.text.lines().next().unwrap_or_default().to_owned()
    });
    let item_arg = at.data.args.get("item").and_then(|item| item.as_u64());
    let head_id = format!("canvas-card-head-{}", at.call_id);
    let head = |view: &mut ViewCx<'_, CanvasUi>, text: String| {
        Some(view.tree(&head_id, &Node::Mono { text }, |_, _, _| {}))
    };
    // A read makes no revision: its card says what it read.
    if at.tool == READ {
        return Some(CardView {
            head: head(
                view,
                match item_arg {
                    Some(item) => format!("canvas · read item {item}"),
                    None => "canvas · read".to_owned(),
                },
            ),
            edge: failed.as_ref().map(|_| Tone::Danger),
            failed,
            ..CardView::default()
        });
    }
    let found = view.state.and_then(|canvas| {
        let (item, revision) = canvas.by_call(&at.call_id)?;
        Some((item.clone(), revision, canvas.fresh))
    });
    let Some((item, revision, fresh)) = found else {
        // Still running, or it failed: what it was asked.
        let title = at
            .data
            .args
            .get("title")
            .and_then(|title| title.as_str())
            .unwrap_or_default();
        return Some(CardView {
            head: head(view, format!("canvas · {title}")),
            edge: failed.as_ref().map(|_| Tone::Danger),
            failed,
            ..CardView::default()
        });
    };
    let content = &item.revisions[revision].content;
    let head = head(
        view,
        format!("{} {} · {}", item.kind().label(), item.id, item.title),
    );
    let label = match content {
        Content::Image { image, .. } => image.describe(),
        Content::Document { text } => format!(
            "revision {} · {} words",
            revision + 1,
            tau_ui_kit::format::grouped(words(text))
        ),
    };
    let open = Node::Button {
        label: "Open on the canvas".into(),
        action: OPEN.into(),
        args: serde_json::to_value(Pick {
            item: item.id,
            revision,
            fresh,
        })
        .unwrap_or_default(),
        kind: Kind::Secondary,
        icon: None,
    };
    let handle = view.handle.clone();
    let act = on_action(handle, at.run.id.clone());
    let id = format!("canvas-card-{}", at.call_id);
    let body = match content {
        Content::Image { image: shown, .. } => {
            let t = view.theme().clone();
            let loaded = image(view, &at.run.repo, &shown.blob, &shown.mime);
            let aspect = match (shown.width, shown.height) {
                (Some(width), Some(height)) if height > 0 => {
                    width as f32 / height as f32
                }
                _ => 1.,
            };
            let thumbnail = picture(&loaded, &t)
                .h(rems(THUMBNAIL))
                .w(rems((THUMBNAIL * aspect).clamp(4., 30.)))
                .max_w_full();
            let open = view.tree(&id, &open, act);
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap(sp(2.))
                .px(sp(3.))
                .py(sp(2.5))
                .child(thumbnail)
                .child(open)
                .into_any_element()
        }
        Content::Document { text } => {
            let tree = Node::stack([excerpt(text), open]);
            div()
                .px(sp(3.))
                .py(sp(2.5))
                .child(view.tree(&id, &tree, act))
                .into_any_element()
        }
    };
    Some(CardView {
        head,
        label: Some(label),
        body: Some(body),
        ..CardView::default()
    })
}

/// A document's first lines, plainly: without the marks that make
/// headings and code.
fn excerpt(text: &str) -> Node {
    Node::Card {
        children: text
            .lines()
            .map(|line| line.trim_start_matches('#').trim().replace('`', ""))
            .filter(|line| !line.is_empty())
            .take(EXCERPT_LINES)
            .map(Node::text)
            .collect(),
    }
}
