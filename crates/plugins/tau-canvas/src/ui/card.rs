//! The cards of the canvas tools: the revision a call made, small, and
//! a click away from the canvas showing it. A card reads the revision
//! from the run's canvas by its call, so a stored run shows it as the
//! live one did.

use gpui::{SharedString, div, prelude::*, rems};
use tau_ui_kit::{
    components::{self as ui, mono},
    theme::{Theme, Tone, Type, radius, sp},
};
use tau_ui_plugin::{
    ViewCx,
    points::{AtCard, CardView},
};

use super::{CanvasUi, Pick, image, pane::picture};
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
    let t = view.theme().clone();
    let result = at.data.result.as_ref();
    let failed = result.filter(|result| result.error).map(|result| {
        result.text.lines().next().unwrap_or_default().to_owned()
    });
    let item_arg = at.data.args.get("item").and_then(|item| item.as_u64());
    // A read makes no revision: its card says what it read.
    if at.tool == READ {
        return Some(CardView {
            head: Some(
                mono(
                    match item_arg {
                        Some(item) => format!("canvas · read item {item}"),
                        None => "canvas · read".to_owned(),
                    },
                    Type::CAPTION,
                    t.text_soft,
                )
                .into_any_element(),
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
            head: Some(
                mono(format!("canvas · {title}"), Type::CAPTION, t.text_soft)
                    .min_w(rems(0.))
                    .truncate()
                    .into_any_element(),
            ),
            edge: failed.as_ref().map(|_| Tone::Danger),
            failed,
            ..CardView::default()
        });
    };
    let content = &item.revisions[revision].content;
    let head = mono(
        format!("{} {} · {}", item.kind().label(), item.id, item.title),
        Type::CAPTION,
        t.text_soft,
    )
    .min_w(rems(0.))
    .truncate()
    .into_any_element();
    let label = match content {
        Content::Image { image, .. } => image.describe(),
        Content::Document { text } => format!(
            "revision {} · {} words",
            revision + 1,
            tau_ui_kit::format::grouped(words(text))
        ),
    };
    let preview = match content {
        Content::Image { image: shown, .. } => {
            let loaded = image(view, &at.run.repo, &shown.blob, &shown.mime);
            let aspect = match (shown.width, shown.height) {
                (Some(width), Some(height)) if height > 0 => {
                    width as f32 / height as f32
                }
                _ => 1.,
            };
            picture(&loaded, &t)
                .h(rems(THUMBNAIL))
                .w(rems((THUMBNAIL * aspect).clamp(4., 30.)))
                .max_w_full()
        }
        Content::Document { text } => excerpt(text, &t),
    };
    let pick = Pick {
        item: item.id,
        revision,
        fresh,
    };
    let (ui_state, handle, run) =
        (view.ui.clone(), view.handle.clone(), at.run.id.clone());
    let body = div()
        .id(SharedString::from(format!("canvas-card-{}", at.call_id)))
        .flex()
        .flex_col()
        .items_start()
        .gap(sp(2.))
        .px(sp(3.))
        .py(sp(2.5))
        .cursor_pointer()
        .child(preview)
        .child(ui::text_link("Open on the canvas", Type::CAPTION, &t))
        .on_click(move |_, _, cx| {
            ui_state.update(cx, |ui, _| ui.pick(&run, pick));
            handle.open_pane(cx);
        });
    Some(CardView {
        head: Some(head),
        label: Some(label),
        body: Some(body.into_any_element()),
        ..CardView::default()
    })
}

/// A document's first lines, plainly: without the marks that make
/// headings and code.
fn excerpt(text: &str, t: &Theme) -> gpui::Div {
    let lines: Vec<String> = text
        .lines()
        .map(|line| line.trim_start_matches('#').trim().replace('`', ""))
        .filter(|line| !line.is_empty())
        .take(EXCERPT_LINES)
        .collect();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .px(sp(3.))
        .py(sp(2.))
        .rounded(radius::CONTROL)
        .border_1()
        .border_color(t.border)
        .children(
            lines
                .into_iter()
                .map(|line| ui::text(line, Type::CAPTION, t.muted).truncate()),
        )
}
