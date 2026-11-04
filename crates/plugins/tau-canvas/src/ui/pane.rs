//! The run's canvas in its pane: a tab per item when there are several,
//! the revision shown with a way through the others, and the image or
//! the document itself. A view tree (ADR 0045); the interface fetches
//! the image.

use tau_ui_plugin::{
    ViewCx,
    points::{AtRun, PaneView},
    tree::{Kind, Node, Tone},
};

use super::{CanvasUi, PICK, Pick, image, on_action, picked, shown};
use crate::{Canvas, Content, Item, Kind as ItemKind};

/// How tall a document scrolls, at most, in rems: the pane clips it
/// before that.
const DOCUMENT_HEIGHT: f32 = 80.;

/// How tall an image is in the pane, in rems; it keeps its shape.
const IMAGE_HEIGHT: f32 = 30.;

/// The pane, once the run has anything on its canvas.
pub fn pane(at: &AtRun, view: &mut ViewCx<'_, CanvasUi>) -> Option<PaneView> {
    let canvas = view.state.filter(|canvas| !canvas.items.is_empty())?;
    let run = at.run.id.clone();
    let (item, revision) = shown(canvas, picked(view, &run))?;
    let mut bars = Vec::new();
    if canvas.items.len() > 1 {
        bars.push(tabs(canvas, item.id));
    }
    if item.revisions.len() > 1 {
        bars.push(revisions(canvas, item, revision));
    }
    let id = format!("canvas-{}-{}-{revision}", run.0, item.id);
    let content = match &item.revisions[revision].content {
        Content::Image {
            image: shown,
            caption,
        } => {
            let mut parts = vec![image(&at.run.repo, shown, IMAGE_HEIGHT)];
            if !caption.is_empty() {
                parts.push(Node::text(caption.clone()));
            }
            parts.push(Node::Mono {
                text: format!("{} · {}", shown.describe(), shown.path),
            });
            Node::stack(parts)
        }
        Content::Document { text } => Node::Scroll {
            height: Some(DOCUMENT_HEIGHT),
            children: vec![
                Node::Title {
                    text: item.title.clone(),
                    subtitle: None,
                },
                Node::Markdown { text: text.clone() },
            ],
        },
    };
    bars.push(content);
    let handle = view.handle.clone();
    let body = view.tree(&id, &Node::stack(bars), on_action(handle, run));
    Some(PaneView {
        title: "Canvas".into(),
        fresh: canvas.fresh,
        body,
    })
}

/// A button that shows `pick`.
fn pick_button(
    label: String,
    icon: Option<&str>,
    kind: Kind,
    pick: Pick,
) -> Node {
    Node::Button {
        label,
        action: PICK.into(),
        args: serde_json::to_value(pick).unwrap_or_default(),
        kind,
        icon: icon.map(str::to_owned),
    }
}

/// A tab per item, the one shown marked; a click shows another.
fn tabs(canvas: &Canvas, shown: u32) -> Node {
    Node::row(canvas.items.iter().map(|item| {
        let pick = Pick {
            item: item.id,
            revision: item.revisions.len() - 1,
            fresh: canvas.fresh,
        };
        let kind = if item.id == shown {
            Kind::Primary
        } else {
            Kind::Secondary
        };
        let label = format!("{} {}", noun_of(item.kind()), item.title);
        pick_button(label, None, kind, pick)
    }))
}

/// Which revision is shown, of how many, with the ones before and
/// after it a click away.
fn revisions(canvas: &Canvas, item: &Item, revision: usize) -> Node {
    let count = item.revisions.len();
    let step = |glyph: &str, to: Option<usize>| match to {
        Some(revision) => pick_button(
            String::new(),
            Some(glyph),
            Kind::Secondary,
            Pick {
                item: item.id,
                revision,
                fresh: canvas.fresh,
            },
        ),
        None => Node::Icon {
            name: glyph.into(),
            tone: Tone::Neutral,
        },
    };
    let noun = match item.kind() {
        ItemKind::Image => "Version",
        ItemKind::Document => "Revision",
    };
    let mut bar = vec![
        step("back", revision.checked_sub(1)),
        step("chevron", (revision + 1 < count).then_some(revision + 1)),
        Node::text(format!("{noun} {} of {count}", revision + 1)),
    ];
    if revision + 1 < count {
        bar.push(Node::text("· not the latest"));
    }
    Node::row(bar)
}

/// What a tab says an item of `kind` is.
fn noun_of(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Image => "Image ·",
        ItemKind::Document => "Document ·",
    }
}
