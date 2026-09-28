//! The larger pieces of task screens (onboarding, pull requests): big
//! buttons, fields, checkboxes and the phone's plain header.

use gpui::{
    Context,
    Div,
    Entity,
    FontWeight,
    Hsla,
    SharedString,
    Stateful,
    div,
    prelude::*,
    px,
    relative,
};

use super::{icon, prose};
use crate::{
    assets::Icon,
    input::TextInput,
    theme::{MONO, Theme},
    workspace::Workspace,
};

/// A screen's big title.
pub fn title(text: impl Into<SharedString>, size: f32) -> Div {
    div()
        .text_size(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(relative(1.25))
        .child(text.into())
}

/// The paragraph under a title. `code` spans are set in mono.
pub fn lead(text: &str, size: f32, t: &Theme) -> Div {
    div()
        .text_size(px(size))
        .line_height(relative(1.6))
        .child(prose(text, t.muted, t))
}

/// A 44 px button: filled in the accent, or outlined.
pub fn big_button(
    label: impl Into<SharedString>,
    glyph: Option<Icon>,
    primary: bool,
    t: &Theme,
) -> Div {
    let color = if primary { t.bg } else { t.text_soft };
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .min_h(px(44.))
        .px(px(18.))
        .rounded(px(8.))
        .text_size(px(14.))
        .text_color(color)
        .cursor_pointer()
        .when(primary, |button| {
            button
                .bg(t.accent)
                .font_weight(FontWeight::SEMIBOLD)
                .hover(|style| style.opacity(0.9))
        })
        .when(!primary, |button| {
            button
                .border_1()
                .border_color(t.border_strong)
                .hover(|style| style.bg(gpui::white().opacity(0.04)))
        })
        .children(glyph.map(|glyph| icon(glyph, 16., color)))
        .child(label.into())
}

/// A panel-colored box with rounded corners.
pub fn panel(padding: f32, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .p(px(padding))
        .bg(t.panel)
        .border_1()
        .border_color(t.border)
        .rounded(px(12.))
}

/// A text field in a 44 px box.
pub fn field(input: &Entity<TextInput>, mono: bool, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(44.))
        .px(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(t.border_strong)
        .bg(t.bg)
        .text_size(px(if mono { 13. } else { 14. }))
        .when(mono, |field| field.font_family(MONO))
        .child(input.clone())
}

/// A field's label above it.
pub fn label(text: &'static str, t: &Theme) -> Div {
    div().text_size(px(13.)).text_color(t.muted).child(text)
}

pub fn checkbox(checked: bool, size: f32, t: &Theme) -> Div {
    let checkbox = div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.));
    if checked {
        checkbox
            .bg(t.accent)
            .child(icon(Icon::Check, size * 0.67, t.bg))
    } else {
        checkbox.border(px(1.5)).border_color(t.border_strong)
    }
}

/// A line of news in a tinted box: waiting, or a failure.
pub fn notice(
    glyph: Icon,
    text: impl Into<SharedString>,
    tone: Hsla,
    size: f32,
    t: &Theme,
) -> Div {
    let danger = tone == t.red;
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(8.))
        .border_1()
        .bg(if danger { t.red_soft } else { t.blue_soft })
        .border_color(if danger { t.red_border } else { t.blue_border })
        .text_size(px(size))
        .text_color(if danger { t.red } else { t.text_soft })
        .child(icon(glyph, 16., tone))
        .child(text.into())
}

/// Text that acts like a link, in blue.
pub fn text_link(text: impl Into<SharedString>, size: f32, t: &Theme) -> Div {
    div()
        .text_size(px(size))
        .text_color(t.blue)
        .cursor_pointer()
        .hover(|style| style.underline())
        .child(text.into())
}

/// The phone's header on a task screen: back, and the title.
pub fn phone_bar(
    ws: &Workspace,
    title: impl Into<SharedString>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    div()
        .h(px(56.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(8.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .when(ws.can_go_back(), |bar| {
            bar.child(
                div()
                    .id("task-back")
                    .size(px(44.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(icon(Icon::Back, 20., t.text_soft))
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .when(!ws.can_go_back(), |bar| bar.pl(px(16.)))
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
}

/// A scrolling phone body.
pub fn phone_body(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(18.))
        .px(px(16.))
        .py(px(20.))
        .text_size(px(15.))
}
