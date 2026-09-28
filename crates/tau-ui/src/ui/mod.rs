//! The pieces screens are built from. Each function draws from a
//! [`RunView`](crate::view::RunView) and the [`Theme`]; the workspace
//! decides where they go for the window's width.

pub mod chrome;
pub mod inspector;
pub mod transcript;

use gpui::{
    Div,
    FontWeight,
    HighlightStyle,
    Hsla,
    IntoElement,
    SharedString,
    Styled,
    StyledText,
    Svg,
    div,
    prelude::*,
    px,
    svg,
};
use tau_agent::event::StopReason;

use crate::{
    assets::Icon,
    theme::{MONO, Theme},
    view::{RunStatus, RunView},
};

pub fn icon(icon: Icon, size: f32, color: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .size(px(size))
        .flex_shrink_0()
        .text_color(color)
}

/// Monospace text.
pub fn mono(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font_family(MONO)
        .text_size(px(size))
        .text_color(color)
        .child(text.into())
}

/// A small uppercase section heading.
pub fn heading(text: &str, t: &Theme) -> Div {
    div()
        .text_size(px(11.))
        .text_color(t.dim)
        .child(text.to_uppercase())
}

/// Prose with `code` spans marked the way the transcript shows them.
pub fn rich(text: &str, t: &Theme) -> StyledText {
    let mut plain = String::with_capacity(text.len());
    let mut code = Vec::new();
    for (index, part) in text.split('`').enumerate() {
        let start = plain.len();
        plain.push_str(part);
        if index % 2 == 1 {
            code.push((
                start..plain.len(),
                HighlightStyle {
                    color: Some(t.text),
                    background_color: Some(t.raised),
                    ..HighlightStyle::default()
                },
            ));
        }
    }
    StyledText::new(plain).with_highlights(code)
}

/// A rounded badge of text.
pub fn pill(text: impl Into<SharedString>, color: Hsla, bg: Hsla) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(10.))
        .bg(bg)
        .text_color(color)
        .text_size(px(12.))
        .child(dot(color, 6.))
        .child(text.into())
}

pub fn dot(color: Hsla, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(size / 2.))
        .bg(color)
}

/// A horizontal meter: `share` of the track filled.
pub fn bar(share: f32, height: f32, fill: Hsla, track: Hsla) -> Div {
    div()
        .relative()
        .h(px(height))
        .w_full()
        .rounded(px(height / 2.))
        .bg(track)
        .child(
            div()
                .h(px(height))
                .w(gpui::relative(share.clamp(0.0, 1.0)))
                .rounded(px(height / 2.))
                .bg(fill),
        )
}

/// A labelled button outline; callers add the id and the click.
pub fn button(label: &'static str, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(6.))
        .border_1()
        .border_color(t.border)
        .text_color(t.text_soft)
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(gpui::white().opacity(0.04)))
        .child(label)
}

pub fn primary_button(label: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(px(30.))
        .px(px(14.))
        .rounded(px(6.))
        .bg(t.accent)
        .text_color(t.bg)
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .child(label.into())
}

/// Two columns of name and value.
pub fn key_values(
    rows: impl IntoIterator<Item = (SharedString, Div)>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(rows.into_iter().map(|(key, value)| {
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(mono(key, 12., t.muted).flex_1())
                .child(value)
        }))
}

/// The dot color and words a run's status reads as.
pub fn status_look(status: &RunStatus, t: &Theme) -> (Hsla, SharedString) {
    match status {
        RunStatus::Planning => (t.blue, "planning".into()),
        RunStatus::Running => (t.accent, "running".into()),
        RunStatus::Finished(stop) => stop_look(stop, t),
    }
}

pub fn stop_look(stop: &StopReason, t: &Theme) -> (Hsla, SharedString) {
    match stop {
        StopReason::Stop => (t.green, "finished".into()),
        StopReason::Limit(kind) => {
            (t.red, format!("limit · {kind:?}").to_lowercase().into())
        }
        StopReason::Cancelled => (t.muted, "cancelled".into()),
        StopReason::Error(_) => (t.red, "error".into()),
    }
}

/// The icon a run shows in lists.
pub fn status_icon(run: &RunView, t: &Theme, size: f32) -> gpui::AnyElement {
    match &run.status {
        RunStatus::Planning | RunStatus::Running => {
            dot(t.accent, 8.).into_any_element()
        }
        RunStatus::Finished(StopReason::Stop) => {
            icon(Icon::Check, size, t.green).into_any_element()
        }
        RunStatus::Finished(StopReason::Cancelled) => {
            icon(Icon::Stop, size, t.muted).into_any_element()
        }
        RunStatus::Finished(_) => {
            icon(Icon::Warning, size, t.red).into_any_element()
        }
    }
}
