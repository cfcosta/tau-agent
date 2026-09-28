//! The components screens are built from: text, buttons, badges, chips,
//! surfaces, fields and notices. Each takes its look from the design
//! tokens in [`crate::theme`], so a component looks the same wherever it
//! is used, and changing it here changes it everywhere.
//!
//! Components return plain elements; callers add ids, clicks and layout.

use gpui::{
    AnyElement,
    Context,
    Div,
    Entity,
    Hsla,
    IntoElement,
    SharedString,
    Stateful,
    Svg,
    div,
    prelude::*,
    px,
    relative,
    svg,
};

use super::prose;
use crate::{
    assets::Icon,
    catalog::Repo,
    input::TextInput,
    theme::{Design, IconSize, MONO, Theme, Type, control, radius, sp, weight},
    workspace::Workspace,
};

// Text.

pub fn icon(icon: Icon, size: IconSize, color: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .size(px(size.0))
        .flex_shrink_0()
        .text_color(color)
}

/// Text in the mono face at `style`'s size.
pub fn mono(text: impl Into<SharedString>, style: Type, color: Hsla) -> Div {
    div()
        .typeset(style.mono())
        .text_color(color)
        .child(text.into())
}

/// Text at `style`, in `color`.
pub fn text(text: impl Into<SharedString>, style: Type, color: Hsla) -> Div {
    div().typeset(style).text_color(color).child(text.into())
}

/// A small uppercase section heading.
pub fn heading(text: &str, t: &Theme) -> Div {
    div()
        .typeset(Type::MICRO)
        .text_color(t.dim)
        .child(text.to_uppercase())
}

/// A screen's big title.
pub fn title(text: impl Into<SharedString>, style: Type) -> Div {
    div()
        .typeset(style)
        .font_weight(weight::STRONG)
        .leading(1.25)
        .child(text.into())
}

/// The paragraph under a big title. `code` spans are set in mono.
pub fn lead(text: &str, style: Type, t: &Theme) -> Div {
    div()
        .typeset(style)
        .leading(1.6)
        .child(prose(text, t.muted, t))
}

/// A screen's title and the sentence under it.
pub fn screen_title(
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(
            div()
                .typeset(Type::HEADING)
                .font_weight(weight::STRONG)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(760.))
                .text_color(t.muted)
                .leading(1.5)
                .child(subtitle.into()),
        )
}

/// A field's label above it.
pub fn label(text: &'static str, t: &Theme) -> Div {
    div().typeset(Type::SMALL).text_color(t.muted).child(text)
}

/// Text that acts like a link, in blue.
pub fn text_link(text: impl Into<SharedString>, style: Type, t: &Theme) -> Div {
    div()
        .typeset(style)
        .text_color(t.blue)
        .cursor_pointer()
        .hover(|style| style.underline())
        .child(text.into())
}

/// A link that navigates somewhere, with a chevron.
pub fn link(label: impl Into<SharedString>, t: &Theme) -> Div {
    text_link(label, Type::CAPTION, t)
        .flex()
        .items_center()
        .gap(sp(1.))
        .flex_shrink_0()
        .child(icon(Icon::Chevron, IconSize::TINY, t.blue))
}

// Buttons.

/// What a button does, and so how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The one thing to do on a screen: filled in the accent.
    Primary,
    /// Anything else: outlined.
    Secondary,
    /// Stopping or throwing away: outlined in red.
    Danger,
}

/// A small button for headers, lists and toolbars.
pub fn button(
    label: impl Into<SharedString>,
    kind: ButtonKind,
    t: &Theme,
) -> Div {
    base_button(label.into(), None, kind, false, t)
}

/// A big button for task screens and phones: 44 px, easy to hit, with an
/// optional leading icon.
pub fn big_button(
    label: impl Into<SharedString>,
    glyph: Option<Icon>,
    kind: ButtonKind,
    t: &Theme,
) -> Div {
    base_button(label.into(), glyph, kind, true, t)
}

fn base_button(
    label: SharedString,
    glyph: Option<Icon>,
    kind: ButtonKind,
    big: bool,
    t: &Theme,
) -> Div {
    let color = match kind {
        ButtonKind::Primary => t.bg,
        ButtonKind::Secondary => t.text_soft,
        ButtonKind::Danger => t.red,
    };
    let button = div()
        .flex()
        .items_center()
        .justify_center()
        .gap(sp(if big { 2. } else { 1.5 }))
        .flex_shrink_0()
        .cursor_pointer()
        .text_color(color)
        .when(big, |button| {
            button
                .min_h(control::LARGE)
                .px(sp(4.5))
                .rounded(radius::BOX)
                .typeset(Type::BODY)
        })
        .when(!big, |button| {
            button
                .h(control::SMALL)
                .px(sp(if kind == ButtonKind::Primary {
                    3.5
                } else {
                    2.5
                }))
                .rounded(radius::CONTROL)
                .typeset(Type::CAPTION)
        })
        .children(glyph.map(|glyph| {
            icon(
                glyph,
                if big {
                    IconSize::LARGE
                } else {
                    IconSize::COMPACT
                },
                color,
            )
        }))
        .child(label);
    match kind {
        ButtonKind::Primary => button
            .bg(t.accent)
            .font_weight(weight::STRONG)
            .hover(|style| style.opacity(0.9)),
        ButtonKind::Secondary | ButtonKind::Danger => button
            .border_1()
            .border_color(match (kind, big) {
                (ButtonKind::Danger, _) => t.red_border,
                (_, true) => t.border_strong,
                _ => t.border,
            })
            .hover(|style| style.bg(gpui::white().opacity(0.04))),
    }
}

/// A round 44 px button holding an icon: the phone's send and stop.
/// Filled in `color` when `filled`, else outlined in `border`.
pub fn round_button(
    id: &'static str,
    glyph: Icon,
    color: Hsla,
    border: Option<Hsla>,
    t: &Theme,
) -> Stateful<Div> {
    let button = div()
        .id(id)
        .size(control::LARGE)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::FULL)
        .cursor_pointer();
    match border {
        Some(border) => button.border_1().border_color(border).child(icon(
            glyph,
            IconSize::BASE,
            color,
        )),
        None => button.bg(color).child(icon(glyph, IconSize::XLARGE, t.bg)),
    }
}

/// A square button holding an icon.
pub fn icon_button(
    id: &'static str,
    glyph: Icon,
    size: f32,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::BOX)
        .cursor_pointer()
        .hover(|style| style.bg(gpui::white().opacity(0.05)))
        .child(icon(glyph, IconSize::XLARGE, t.text_soft))
}

// Marks.

/// tau's mark: τ on the accent.
pub fn logo(t: &Theme, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size / 4.))
        .bg(t.accent)
        .text_color(t.bg)
        .font_family(MONO)
        .text_size(px(size * 0.6))
        .font_weight(weight::EMPHASIS)
        .child("τ")
}

/// A repository's mark: its letter on its color, 20 px in the sidebar
/// and 26 px on a phone.
pub fn repo_mark(repo: &Repo, size: f32, t: &Theme) -> Div {
    mono(
        repo.letter(),
        if size > 22. { Type::SMALL } else { Type::MICRO },
        t.bg,
    )
    .size(px(size))
    .flex_shrink_0()
    .flex()
    .items_center()
    .justify_center()
    .rounded(if size > 22. {
        radius::CONTROL
    } else {
        radius::TAG
    })
    .bg(t.mark(&repo.name))
    .font_weight(weight::EMPHASIS)
}

/// A count on a pill: a conversation's unread replies.
pub fn count_pill(count: usize, t: &Theme) -> Div {
    mono(count.to_string(), Type::MICRO, t.bg)
        .flex_shrink_0()
        .min_w(px(18.))
        .h(px(18.))
        .px(sp(1.5))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::FULL)
        .bg(t.accent)
        .font_weight(weight::EMPHASIS)
}

pub fn dot(color: Hsla, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(radius::FULL)
        .bg(color)
}

/// A status: a dot and words, on a tinted pill.
pub fn pill(text: impl Into<SharedString>, color: Hsla, bg: Hsla) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(1.5))
        .px(sp(2.))
        .py(sp(0.75))
        .rounded(radius::LARGE)
        .bg(bg)
        .text_color(color)
        .typeset(Type::CAPTION)
        .child(dot(color, 6.))
        .child(text.into())
}

/// Words in an outlined pill: a state (`Draft`) or a tag
/// (`Recommended`).
pub fn badge(text: impl Into<SharedString>, color: Hsla, border: Hsla) -> Div {
    div()
        .flex_shrink_0()
        .px(sp(2.))
        .py(sp(0.5))
        .rounded(radius::LARGE)
        .border_1()
        .border_color(border)
        .typeset(Type::MICRO)
        .text_color(color)
        .child(text.into())
}

/// A name in mono on a raised chip, with an optional leading icon: a
/// branch, a repository, a model.
pub fn chip(
    glyph: Option<Icon>,
    text: impl Into<SharedString>,
    style: Type,
    color: Hsla,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(1.5))
        .flex_shrink_0()
        .px(sp(2.5))
        .py(sp(1.25))
        .rounded(radius::CONTROL)
        .bg(t.raised)
        .children(glyph.map(|glyph| icon(glyph, IconSize::COMPACT, color)))
        .child(mono(text, style, color))
}

/// A short value in mono on a small raised tag: a field name, a path, a
/// permission level.
pub fn tag(
    text: impl Into<SharedString>,
    style: Type,
    color: Hsla,
    t: &Theme,
) -> Div {
    mono(text, style, color)
        .flex_shrink_0()
        .px(sp(1.5))
        .py(sp(0.5))
        .rounded(radius::SMALL)
        .bg(t.raised)
}

/// What the user wrote, in a raised bubble.
pub fn bubble(t: &Theme) -> Div {
    div()
        .px(sp(3.5))
        .py(sp(3.))
        .bg(t.raised)
        .border_1()
        .border_color(t.border_soft)
        .rounded(radius::LARGE)
        .leading(1.55)
}

/// A horizontal meter: `share` of the track filled.
pub fn bar(share: f32, height: f32, fill: Hsla, track: Hsla) -> Div {
    div()
        .relative()
        .h(px(height))
        .w_full()
        .rounded(radius::FULL)
        .bg(track)
        .child(
            div()
                .h(px(height))
                .w(relative(share.clamp(0.0, 1.0)))
                .rounded(radius::FULL)
                .bg(fill),
        )
}

// Surfaces.

/// A bordered box, clipping what it holds.
pub fn card(t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.border)
        .rounded(radius::BOX)
        .overflow_hidden()
}

/// A panel-colored box with big rounded corners, padded by `steps`.
pub fn panel(steps: f32, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .p(sp(steps))
        .bg(t.panel)
        .border_1()
        .border_color(t.border)
        .rounded(radius::CARD)
}

/// A small labelled number.
pub fn stat(label: &str, value: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(1.))
        .px(sp(3.))
        .py(sp(2.5))
        .rounded(radius::CONTROL)
        .bg(t.panel)
        .child(mono(label.to_owned(), Type::CAPTION, t.dim))
        .child(mono(value, Type::LEAD, t.text))
}

/// Two columns of name and value: names in a fixed column, values
/// left-aligned beside them.
pub fn key_values(
    rows: impl IntoIterator<Item = (SharedString, Div)>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .children(rows.into_iter().map(|(key, value)| {
            div()
                .flex()
                .items_start()
                .gap(sp(3.))
                .child(
                    mono(key, Type::CAPTION, t.muted)
                        .w(px(128.))
                        .flex_shrink_0(),
                )
                .child(div().flex_1().min_w(px(0.)).child(value))
        }))
}

/// A line of news in a tinted box: waiting, or a failure.
pub fn notice(
    glyph: Icon,
    text: impl Into<SharedString>,
    tone: Hsla,
    style: Type,
    t: &Theme,
) -> Div {
    let danger = tone == t.red;
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .px(sp(3.5))
        .py(sp(3.))
        .rounded(radius::BOX)
        .border_1()
        .bg(if danger { t.red_soft } else { t.blue_soft })
        .border_color(if danger { t.red_border } else { t.blue_border })
        .typeset(style)
        .text_color(if danger { t.red } else { t.text_soft })
        .child(icon(glyph, IconSize::LARGE, tone))
        .child(div().flex_1().min_w(px(0.)).child(text.into()))
}

/// A modal dialog over a dimmed backdrop: a title, a message and what
/// to do about it. Callers put their buttons in `actions` and handle
/// dismissal.
pub fn dialog(
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
    actions: impl IntoElement,
    t: &Theme,
) -> Div {
    modal(
        icon(Icon::Warning, IconSize::LARGE, t.red),
        title,
        message,
        None,
        actions,
        t,
    )
}

/// A modal over a dimmed backdrop: a title, a message, then anything
/// the message asks for (a field, a choice) above the actions.
pub fn modal(
    glyph: Svg,
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
    body: Option<AnyElement>,
    actions: impl IntoElement,
    t: &Theme,
) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .p(sp(4.))
        .bg(t.scrim)
        .child(
            // A set width, not a share of the backdrop, so wrapped text
            // is measured at the width it is drawn at.
            div()
                .w(px(440.))
                .max_w_full()
                .flex()
                .flex_col()
                .gap(sp(3.))
                .p(sp(5.))
                .bg(t.panel)
                .border_1()
                .border_color(t.border_strong)
                .rounded(radius::CARD)
                .shadow_lg()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.5))
                        .child(glyph)
                        .child(
                            div()
                                .typeset(Type::SUBTITLE)
                                .font_weight(weight::STRONG)
                                .child(title.into()),
                        ),
                )
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .leading(1.55)
                        .text_color(t.text_soft)
                        .child(message.into()),
                )
                .children(body)
                .child(div().flex().justify_end().gap(sp(2.)).child(actions)),
        )
}

/// An empty state.
pub fn empty(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .justify_center()
        .py(sp(12.))
        .text_color(t.muted)
        .child(text.into())
}

// Forms.

/// A text field in a 44 px box.
pub fn field(input: &Entity<TextInput>, mono: bool, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .h(control::LARGE)
        .px(sp(3.))
        .rounded(radius::BOX)
        .border_1()
        .border_color(t.border_strong)
        .bg(t.bg)
        .typeset(if mono { Type::SMALL.mono() } else { Type::BODY })
        .child(input.clone())
}

/// An on/off switch; callers add the id and the click.
pub fn switch(on: bool, t: &Theme) -> Div {
    div()
        .w(px(36.))
        .h(px(20.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .p(sp(0.5))
        .rounded(radius::FULL)
        .bg(if on { t.accent } else { t.border_strong })
        .when(on, |track| track.justify_end())
        .cursor_pointer()
        .child(div().size(px(16.)).rounded(radius::FULL).bg(if on {
            t.bg
        } else {
            t.muted
        }))
}

/// A checkbox: 18 px, or 20 px for a phone's touch rows.
pub fn checkbox(checked: bool, large: bool, t: &Theme) -> Div {
    let size = if large { 20. } else { 18. };
    let checkbox = div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SMALL);
    if checked {
        checkbox.bg(t.accent).child(icon(
            Icon::Check,
            if large {
                IconSize::BASE
            } else {
                IconSize::SMALL
            },
            t.bg,
        ))
    } else {
        checkbox.border(px(1.5)).border_color(t.border_strong)
    }
}

// Layout.

/// A scrolling screen body with the layout's padding.
pub fn screen(
    id: &'static str,
    compact: bool,
    content: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(4.5))
                .px(sp(if compact { 4. } else { 8. }))
                .py(sp(if compact { 4. } else { 6. }))
                .child(content),
        )
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
        .gap(sp(2.5))
        .px(sp(2.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .when(ws.can_go_back(), |bar| {
            bar.child(
                icon_button("task-back", Icon::Back, 44., t)
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .when(!ws.can_go_back(), |bar| bar.pl(sp(4.)))
        .child(
            div()
                .typeset(Type::SUBTITLE)
                .font_weight(weight::STRONG)
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
        .gap(sp(4.5))
        .px(sp(4.))
        .py(sp(5.))
        .typeset(Type::LEAD)
}

/// Anything, as an element.
pub fn any(element: impl IntoElement) -> AnyElement {
    element.into_any_element()
}
