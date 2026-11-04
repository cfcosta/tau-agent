//! The run's canvas in its pane: a tab per item when there are several,
//! the revision shown with a way through the others, and the image or
//! the document itself. All but the image is a view tree (ADR 0045).

use gpui::{Div, ObjectFit, div, img, prelude::*, rems};
use tau_ui_kit::{
    components as ui,
    theme::{Theme, Type, radius, sp},
};
use tau_ui_plugin::{
    ViewCx,
    points::{AtRun, PaneView},
    tree::{Kind, Node, Tone},
};

use super::{CanvasUi, Loaded, PICK, Pick, image, on_action, picked, shown};
use crate::{Canvas, Content, Item, Kind as ItemKind};

/// How tall a document scrolls, at most, in rems: the pane clips it
/// before that.
const DOCUMENT_HEIGHT: f32 = 80.;

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
            let t = view.theme().clone();
            let loaded = image(view, &at.run.repo, &shown.blob, &shown.mime);
            let mut under = Vec::new();
            if !caption.is_empty() {
                under.push(Node::text(caption.clone()));
            }
            under.push(Node::Mono {
                text: format!("{} · {}", shown.describe(), shown.path),
            });
            let under = view.tree(
                &format!("{id}-under"),
                &Node::stack(under),
                |_, _, _| {},
            );
            div()
                .flex_1()
                .min_h(rems(0.))
                .flex()
                .flex_col()
                .child(
                    picture(&loaded, &t)
                        .flex_1()
                        .min_h(rems(0.))
                        .m(sp(if view.compact { 3. } else { 5. })),
                )
                .child(div().flex_shrink_0().px(sp(5.)).pb(sp(4.)).child(under))
                .into_any_element()
        }
        Content::Document { text } => {
            let document = Node::Scroll {
                height: Some(DOCUMENT_HEIGHT),
                children: vec![
                    Node::Title {
                        text: item.title.clone(),
                        subtitle: None,
                    },
                    Node::Markdown { text: text.clone() },
                ],
            };
            let document = view.tree(&id, &document, |_, _, _| {});
            div()
                .flex_1()
                .min_h(rems(0.))
                .flex()
                .flex_col()
                .px(sp(if view.compact { 4. } else { 7. }))
                .py(sp(5.))
                .child(document)
                .into_any_element()
        }
    };
    let handle = view.handle.clone();
    let bars = (!bars.is_empty()).then(|| {
        view.tree("canvas-bars", &Node::stack(bars), on_action(handle, run))
    });
    let body = div()
        .flex_1()
        .min_h(rems(0.))
        .flex()
        .flex_col()
        .when_some(bars, |body, bars| {
            body.child(div().flex_shrink_0().px(sp(3.)).py(sp(2.)).child(bars))
        })
        .child(content);
    Some(PaneView {
        title: "Canvas".into(),
        fresh: canvas.fresh,
        body: body.into_any_element(),
    })
}

// ADR 0045: not a tree yet — needs an image piece (the bytes of a blob
// the plugin fetched, fitted inside the box it is given).
/// An image as the window has it, fitted inside whatever box it is
/// given: the picture, or a word on why it is not there yet.
pub(super) fn picture(loaded: &Loaded, t: &Theme) -> Div {
    let frame = div().flex().items_center().justify_center();
    match loaded {
        Loaded::Ready(image) => frame.child(
            img(image.clone())
                .size_full()
                .object_fit(ObjectFit::Contain),
        ),
        Loaded::Waiting(..) => frame
            .rounded(radius::CARD)
            .bg(t.raised)
            .child(ui::text("Loading the image…", Type::CAPTION, t.dim)),
        Loaded::Failed(error) => frame
            .rounded(radius::CARD)
            .bg(t.raised)
            .p(sp(3.))
            .child(ui::text(
                format!("The image could not be loaded: {error}"),
                Type::CAPTION,
                t.muted,
            )),
    }
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
