//! The run's canvas in its pane: a tab per item when there are several,
//! the revision shown with a way through the others, and the image or
//! the document itself.

use gpui::{Div, ObjectFit, SharedString, div, img, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, mono},
    prose,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
};
use tau_ui_plugin::{
    ViewCx,
    points::{AtRun, PaneView},
};

use super::{CanvasUi, Loaded, Pick, image, shown};
use crate::{Canvas, Content, Item, Kind};

/// The pane, once the run has anything on its canvas.
pub fn pane(at: &AtRun, view: &mut ViewCx<'_, CanvasUi>) -> Option<PaneView> {
    let canvas = view.state.filter(|canvas| !canvas.items.is_empty())?;
    let t = view.theme().clone();
    let run = at.run.id.clone();
    let pick = view.read_ui().picked.get(&run).copied();
    let (item, revision) = shown(canvas, pick)?;
    let content = match &item.revisions[revision].content {
        Content::Image {
            image: shown,
            caption,
        } => {
            let loaded = image(view, &at.run.repo, &shown.blob, &shown.mime);
            let meta = format!("{} · {}", shown.describe(), shown.path);
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
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(sp(1.))
                        .px(sp(5.))
                        .pb(sp(4.))
                        .when(!caption.is_empty(), |under| {
                            under.child(ui::text(
                                caption.clone(),
                                Type::SMALL,
                                t.text_soft,
                            ))
                        })
                        .child(mono(meta, Type::MICRO, t.dim).truncate()),
                )
                .into_any_element()
        }
        Content::Document { text } => div()
            .id(SharedString::from(format!(
                "canvas-document-{}-{}-{revision}",
                run.0, item.id
            )))
            .flex_1()
            .min_h(rems(0.))
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(rems(46.))
                    .mx_auto()
                    .px(sp(if view.compact { 4. } else { 7. }))
                    .py(sp(5.))
                    .flex()
                    .flex_col()
                    .gap(sp(4.))
                    .child(
                        ui::text(item.title.clone(), Type::HEADING, t.text)
                            .font_weight(weight::STRONG),
                    )
                    .child(prose::markdown(
                        &format!("canvas-{}-{}-{revision}", run.0, item.id),
                        text,
                        t.text,
                        &t,
                    )),
            )
            .into_any_element(),
    };
    let body = div()
        .flex_1()
        .min_h(rems(0.))
        .flex()
        .flex_col()
        .when(canvas.items.len() > 1, |body| {
            body.child(tabs(canvas, item.id, &run, view, &t))
        })
        .when(item.revisions.len() > 1, |body| {
            body.child(revisions(canvas, item, revision, &run, view, &t))
        })
        .child(content);
    Some(PaneView {
        title: "Canvas".into(),
        fresh: canvas.fresh,
        body: body.into_any_element(),
    })
}

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
        Loaded::Waiting(_) => frame
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

/// A tab per item, the one shown marked; a click shows another.
fn tabs(
    canvas: &Canvas,
    shown: u32,
    run: &tau_agent::tool::RunId,
    view: &ViewCx<'_, CanvasUi>,
    t: &Theme,
) -> impl IntoElement {
    let tabs = canvas.items.iter().map(|item| {
        let selected = item.id == shown;
        let pick = Pick {
            item: item.id,
            revision: item.revisions.len() - 1,
            fresh: canvas.fresh,
        };
        let (ui_state, run) = (view.ui.clone(), run.clone());
        let handle = view.handle.clone();
        div()
            .id(SharedString::from(format!("canvas-tab-{}", item.id)))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(sp(1.5))
            .max_w(rems(14.))
            .px(sp(2.5))
            .py(sp(1.25))
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .when(selected, |tab| tab.bg(t.selected))
            .hover(|style| style.bg(t.raised))
            .child(ui::icon(
                icon_of(item.kind()),
                IconSize::COMPACT,
                t.text_soft,
            ))
            .child(
                div()
                    .min_w(rems(0.))
                    .truncate()
                    .typeset(Type::CAPTION)
                    .text_color(if selected { t.text } else { t.muted })
                    .when(selected, |title| title.font_weight(weight::EMPHASIS))
                    .child(item.title.clone()),
            )
            .on_click(move |_, _, cx| {
                ui_state.update(cx, |ui, _| ui.pick(&run, pick));
                handle.refresh(cx);
            })
    });
    div()
        .id("canvas-tabs")
        .flex_shrink_0()
        .flex()
        .gap(sp(1.))
        .px(sp(3.))
        .py(sp(2.))
        .border_b_1()
        .border_color(t.border)
        .overflow_x_scroll()
        .children(tabs)
}

/// Which revision is shown, of how many, with the ones before and
/// after it a click away.
fn revisions(
    canvas: &Canvas,
    item: &Item,
    revision: usize,
    run: &tau_agent::tool::RunId,
    view: &ViewCx<'_, CanvasUi>,
    t: &Theme,
) -> impl IntoElement {
    let count = item.revisions.len();
    let step = |id: &'static str, glyph: Icon, to: Option<usize>| {
        let (ui_state, run, handle) =
            (view.ui.clone(), run.clone(), view.handle.clone());
        let (item, fresh) = (item.id, canvas.fresh);
        div()
            .id(id)
            .size(rems(1.75))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::BOX)
            .child(ui::icon(
                glyph,
                IconSize::COMPACT,
                if to.is_some() { t.text_soft } else { t.dim },
            ))
            .when_some(to, |button, revision| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(t.raised))
                    .on_click(move |_, _, cx| {
                        let pick = Pick {
                            item,
                            revision,
                            fresh,
                        };
                        ui_state.update(cx, |ui, _| ui.pick(&run, pick));
                        handle.refresh(cx);
                    })
            })
    };
    let noun = match item.kind() {
        Kind::Image => "Version",
        Kind::Document => "Revision",
    };
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(sp(1.))
        .px(sp(4.))
        .py(sp(1.5))
        .border_b_1()
        .border_color(t.border)
        .child(step("canvas-older", Icon::Back, revision.checked_sub(1)))
        .child(step(
            "canvas-newer",
            Icon::Chevron,
            (revision + 1 < count).then_some(revision + 1),
        ))
        .child(ui::text(
            format!("{noun} {} of {count}", revision + 1),
            Type::CAPTION,
            t.muted,
        ))
        .when(revision + 1 < count, |bar| {
            bar.child(ui::text("· not the latest", Type::CAPTION, t.dim))
        })
}

/// The icon of an item of `kind`.
pub(super) fn icon_of(kind: Kind) -> Icon {
    match kind {
        Kind::Image => Icon::Image,
        Kind::Document => Icon::File,
    }
}
