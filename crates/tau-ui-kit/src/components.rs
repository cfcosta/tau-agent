//! The components screens are built from: text, buttons, badges, chips,
//! surfaces, fields and notices. Each takes its look from the design
//! tokens in [`crate::theme`], so a component looks the same wherever it
//! is used, and changing it here changes it everywhere.
//!
//! Components return plain elements; callers add ids, clicks and layout.

use gpui::{
    AnyElement,
    BoxShadow,
    Div,
    Entity,
    Hsla,
    IntoElement,
    SharedString,
    Stateful,
    Svg,
    div,
    linear_color_stop,
    linear_gradient,
    prelude::*,
    px,
    relative,
    svg,
};

use crate::{
    assets::Icon,
    input::TextInput,
    prose::prose,
    theme::{Design, IconSize, MONO, Theme, Type, control, radius, sp, weight},
};

// Depth.

/// Which side of the window a piece of chrome sits on: the side its
/// shadow falls away from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The title bar: its shadow falls down onto the content.
    Top,
    /// The status bar: its shadow falls up.
    Bottom,
    /// The sidebar: its shadow falls right.
    Left,
    /// The inspector: its shadow falls left.
    Right,
}

/// A shadow `x` and `y` px off, blurred by `blur`.
pub fn shade(x: f32, y: f32, blur: f32, color: Hsla) -> BoxShadow {
    BoxShadow::new(px(x), px(y), color).blur_radius(px(blur))
}

/// Top to bottom, from `top` to `bottom`.
fn fall(top: Hsla, bottom: Hsla) -> gpui::Background {
    linear_gradient(
        180.,
        linear_color_stop(top, 0.),
        linear_color_stop(bottom, 1.),
    )
}

/// The milled look (see [`crate::theme::Depth`]) for any element: how
/// it rises out of the ground or sinks into it.
pub trait Material: Styled + Sized {
    /// A raised panel: cards, menus, notes. Lit along its top edge,
    /// shaded along its bottom, and casting a soft shadow.
    fn raised(self, t: &Theme) -> Self {
        let d = &t.depth;
        self.bg(fall(d.panel_top, t.card)).shadow(vec![
            shade(0., 1., 0., d.highlight).inset(),
            shade(0., -1., 0., d.shade).inset(),
            shade(0., 2., 4., d.drop),
            shade(0., 10., 24., d.drop.opacity(0.6)),
        ])
    }

    /// A well sunk into its surface: fields, meters, the terminal.
    fn well(self, t: &Theme) -> Self {
        let d = &t.depth;
        self.bg(d.well).shadow(vec![
            shade(0., 2., 6., d.inner).inset(),
            BoxShadow::new(px(0.), px(0.), d.shade)
                .spread_radius(px(1.))
                .inset(),
            shade(0., 1., 0., d.highlight.opacity(0.8)),
        ])
    }

    /// A key: buttons, chips, the user's bubble. Rounded over from a
    /// lit top to a darker bottom, on a small shadow.
    fn key(self, t: &Theme) -> Self {
        let d = &t.depth;
        self.bg(fall(d.key_top, d.key_bottom))
            .border_1()
            .border_color(d.key_border)
            .shadow(key_shadows(t))
    }

    /// The accent key: the one thing to do. Its light spills around it.
    fn accent_key(self, t: &Theme) -> Self {
        let d = &t.depth;
        self.bg(fall(d.accent_top, d.accent_bottom)).shadow(vec![
            shade(0., 1., 0., gpui::white().opacity(0.45)).inset(),
            shade(0., -2., 0., d.accent_edge).inset(),
            shade(0., 4., 12., d.accent_glow),
        ])
    }

    /// A danger key: stopping or throwing away.
    fn danger_key(self, t: &Theme) -> Self {
        let d = &t.depth;
        self.bg(fall(d.danger_top, d.danger_bottom))
            .border_1()
            .border_color(t.red_border)
            .shadow(key_shadows(t))
    }

    /// Pressed in: the selected row of a list.
    fn pressed(self, t: &Theme) -> Self {
        let d = &t.depth;
        self.bg(t.selected).shadow(vec![
            shade(0., 2., 5., d.inner.opacity(0.85)).inset(),
            shade(0., 1., 0., d.highlight.opacity(0.8)),
        ])
    }

    /// Chrome on one `edge` of the window: lit from above, with a seam
    /// and a shadow toward what it frames.
    fn chrome(self, edge: Edge, t: &Theme) -> Self {
        let d = &t.depth;
        let (x, y) = match edge {
            Edge::Top => (0., 1.),
            Edge::Bottom => (0., -1.),
            Edge::Left => (1., 0.),
            Edge::Right => (-1., 0.),
        };
        self.bg(fall(d.chrome_top, t.panel)).shadow(vec![
            shade(0., 1., 0., d.highlight.opacity(0.7)).inset(),
            shade(x, y, 0., d.seam),
            shade(x * 6., y * 4., 20., d.drop.opacity(0.75)),
        ])
    }

    /// A tinted band, `fill` at its top fading down, lit along its top
    /// edge: a banner.
    fn lit(self, fill: Hsla, t: &Theme) -> Self {
        let d = &t.depth;
        self.bg(fall(fill, fill.opacity(0.55))).shadow(vec![
            shade(0., 1., 0., d.highlight.opacity(0.7)).inset(),
            shade(0., 4., 12., d.drop.opacity(0.6)),
        ])
    }

    /// A key in any colors: falling from `top` to `bottom`, lit along
    /// its top edge, on a small shadow. For looks with their own palette,
    /// such as onboarding's.
    fn bevel(self, top: Hsla, bottom: Hsla, t: &Theme) -> Self {
        self.bg(fall(top, bottom)).shadow(key_shadows(t))
    }

    /// A well with its own `floor`.
    fn sunk(self, floor: Hsla, t: &Theme) -> Self {
        self.well(t).bg(floor)
    }

    /// Light and shadow only, keeping the fill: a see-through card
    /// rises off what shows through it.
    fn lifted(self, t: &Theme) -> Self {
        let d = &t.depth;
        self.shadow(vec![
            shade(0., 1., 0., d.highlight).inset(),
            shade(0., 2., 4., d.drop),
            shade(0., 16., 40., d.drop.opacity(0.8)),
        ])
    }

    /// A light around a small shape in `color`: a live dot.
    fn glow(self, color: Hsla) -> Self {
        self.shadow(vec![shade(0., 0., 6., color)])
    }
}

impl<E: Styled> Material for E {}

/// The inner shadow of a meter's `track`.
fn track_shade(track: Hsla) -> Hsla {
    gpui::black().opacity(0.55 * track.a.max(0.6))
}

/// The shadows under a key.
fn key_shadows(t: &Theme) -> Vec<BoxShadow> {
    let d = &t.depth;
    vec![
        shade(0., 1., 0., d.highlight.opacity(1.5)).inset(),
        shade(0., 1., 2., d.inner.opacity(0.85)),
        shade(0., 3., 8., d.drop.opacity(0.75)),
    ]
}

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
            .accent_key(t)
            .font_weight(weight::STRONG)
            .hover(|style| style.opacity(0.92)),
        ButtonKind::Secondary => button
            .key(t)
            .hover(|style| style.border_color(t.border_strong)),
        ButtonKind::Danger => button
            .danger_key(t)
            .hover(|style| style.border_color(t.red)),
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
        Some(border) => button.key(t).border_color(border).child(icon(
            glyph,
            IconSize::BASE,
            color,
        )),
        None if color == t.accent => {
            button
                .accent_key(t)
                .child(icon(glyph, IconSize::XLARGE, t.bg))
        }
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
        .accent_key(t)
        .text_color(t.bg)
        .font_family(MONO)
        .text_size(px(size * 0.6))
        .font_weight(weight::EMPHASIS)
        .child("τ")
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

/// A dot that glows: something running now.
pub fn live_dot(color: Hsla, size: f32) -> Div {
    dot(color, size).glow(color)
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
        .child(live_dot(color, 6.))
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
        .key(t)
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
        .key(t)
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
        .shadow(vec![shade(0., 1., 2., track_shade(track)).inset()])
        .child(
            div()
                .h(px(height))
                .w(relative(share.clamp(0.0, 1.0)))
                .rounded(radius::FULL)
                .bg(fill)
                .shadow(vec![
                    shade(0., 1., 0., gpui::white().opacity(0.3)).inset(),
                ]),
        )
}

// Surfaces.

/// A raised, bordered box, clipping what it holds.
pub fn card(t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .raised(t)
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
        .raised(t)
        .border_1()
        .border_color(t.border)
        .rounded(radius::CARD)
}

/// A 56 px glyph in a tinted tile, above a phone screen's title: done,
/// or what went wrong.
pub fn tile(glyph: Icon, color: Hsla, bg: Hsla, border: Hsla) -> Div {
    div()
        .size(px(56.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::TILE)
        .bg(bg)
        .border_1()
        .border_color(border)
        .child(icon(glyph, IconSize(28.), color))
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
        // What is under it neither scrolls nor takes clicks.
        .occlude()
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
                            // Shrinks to the box, so a long title wraps
                            // instead of running out of it.
                            div()
                                .flex_1()
                                .min_w(px(0.))
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
        .well(t)
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
        .when(on, |track| track.accent_key(t).justify_end())
        .when(!on, |track| track.well(t))
        .cursor_pointer()
        .child(div().size(px(16.)).rounded(radius::FULL).key(t))
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
        checkbox.accent_key(t).child(icon(
            Icon::Check,
            if large {
                IconSize::BASE
            } else {
                IconSize::SMALL
            },
            t.bg,
        ))
    } else {
        checkbox.well(t)
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

/// A rule's strictness: the probability line from 0 to 1, split where
/// the rule flags a call and where it blocks one, with a mark at
/// `score` when there is one. `handles` draws the thresholds as knobs,
/// for the editor.
pub fn strictness(
    review: f64,
    block: f64,
    score: Option<f64>,
    width: Option<f32>,
    handles: bool,
    t: &Theme,
) -> Div {
    let height = if handles { 6. } else { 5. };
    let zone = |share: f64, color: Hsla| {
        div()
            .h(px(height))
            .w(relative(share.clamp(0.0, 1.0) as f32))
            .bg(color)
    };
    let knob = |at: f64, color: Hsla| {
        div()
            .absolute()
            .left(relative(at as f32))
            .top(px(-6.))
            .ml(px(-9.))
            .size(px(18.))
            .rounded(radius::FULL)
            .bg(t.text)
            .border_3()
            .border_color(color)
    };
    div()
        .relative()
        .h(px(height))
        .flex_shrink_0()
        .map(|line| match width {
            Some(width) => line.w(px(width)),
            None => line.w_full(),
        })
        .child(
            div()
                .flex()
                .size_full()
                .rounded(radius::HAIRLINE)
                .overflow_hidden()
                .child(zone(review, t.border_strong))
                .child(zone(block - review, t.accent.opacity(0.55)))
                .child(div().flex_1().h(px(height)).bg(t.red.opacity(0.6))),
        )
        .when_some(score, |line, score| {
            let color = if score >= block {
                t.red
            } else if score >= review {
                t.accent
            } else {
                t.text_soft
            };
            line.child(
                div()
                    .absolute()
                    .left(relative(score.clamp(0.0, 1.0) as f32))
                    .top(px(-4.))
                    .ml(px(-1.))
                    .w(px(2.))
                    .h(px(height + 8.))
                    .rounded(radius::HAIRLINE)
                    .bg(color),
            )
        })
        .when(handles, |line| {
            line.child(knob(review, t.accent)).child(knob(block, t.red))
        })
}

/// A table from a model's reply: a header row on a raised band, rows
/// split by hairlines, each cell lined up as its column says. Columns
/// share the width; long cells wrap.
pub fn table(
    align: &[crate::markdown::Align],
    head: Vec<AnyElement>,
    rows: Vec<Vec<AnyElement>>,
    t: &Theme,
) -> Div {
    use crate::markdown::Align;
    let row = |cells: Vec<AnyElement>, header: bool| {
        div()
            .flex()
            .w_full()
            .when(!header, |row| row.border_t_1().border_color(t.border))
            .when(header, |row| row.bg(t.raised).text_color(t.text))
            .children(cells.into_iter().enumerate().map(|(n, content)| {
                let cell = div()
                    .flex_1()
                    .min_w(px(0.))
                    .px(sp(3.))
                    .py(sp(2.))
                    .when(n > 0, |cell| {
                        cell.border_l_1().border_color(t.border)
                    });
                let cell = match align.get(n) {
                    Some(Align::Right) => cell.text_right(),
                    Some(Align::Center) => cell.text_center(),
                    _ => cell,
                };
                cell.child(content)
            }))
    };
    div()
        .flex()
        .flex_col()
        .w_full()
        .border_1()
        .border_color(t.border_strong)
        .rounded(radius::BOX)
        .overflow_hidden()
        .typeset(Type::SMALL)
        .child(row(head, true))
        .children(rows.into_iter().map(|cells| row(cells, false)))
}

/// A fenced block of code from a reply, in the monospace face, with its
/// language in the corner.
pub fn code_block(lang: Option<&str>, text: &str, t: &Theme) -> Div {
    div()
        .relative()
        .w_full()
        .px(sp(3.5))
        .py(sp(3.))
        .rounded(radius::BOX)
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .child(mono(text.to_owned(), Type::SMALL, t.text_soft))
        .children(lang.map(|lang| {
            div().absolute().top(sp(1.5)).right(sp(2.5)).child(mono(
                lang.to_owned(),
                Type::MICRO,
                t.dim,
            ))
        }))
}

/// Quoted blocks, set off by a bar on the left.
pub fn quote(children: Vec<AnyElement>, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .pl(sp(3.5))
        .border_l_2()
        .border_color(t.border_strong)
        .text_color(t.muted)
        .children(children)
}

/// A list item: its bullet or number, then its blocks.
pub fn list_item(marker: String, children: Vec<AnyElement>, t: &Theme) -> Div {
    div()
        .flex()
        .gap(sp(2.))
        .child(
            div()
                .flex_shrink_0()
                .min_w(sp(4.))
                .text_color(t.dim)
                .child(marker),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .children(children),
        )
}

/// A heading in a reply: 1 is largest; from 3 on, body size in bold.
pub fn md_heading(level: u8, content: AnyElement, t: &Theme) -> Div {
    div()
        .pt(sp(1.))
        .text_color(t.text)
        .font_weight(weight::STRONG)
        .typeset(match level {
            1 => Type::HEADING,
            2 => Type::LEAD,
            _ => Type::BODY,
        })
        .child(content)
}

/// A thematic break in a reply.
pub fn rule(t: &Theme) -> Div {
    div().w_full().h(px(1.)).my(sp(1.)).bg(t.border)
}

/// `text` as a QR code, `size` px square: black modules on a white
/// plate with a quiet zone, whatever the theme, so any camera reads it.
/// `None` if the text does not fit a QR code.
pub fn qr_code(text: &str, size: f32) -> Option<Div> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let dark: Vec<bool> = code
        .to_colors()
        .into_iter()
        .map(|color| color == qrcode::Color::Dark)
        .collect();
    // Four modules of quiet zone on each side, as the standard asks.
    let module = size / (width + 8) as f32;
    let modules = gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let black = gpui::rgb(0x000000);
            for (n, _) in dark.iter().enumerate().filter(|(_, dark)| **dark) {
                let (x, y) = ((n % width) as f32, (n / width) as f32);
                let origin = gpui::point(
                    bounds.left() + px(x * module),
                    bounds.top() + px(y * module),
                );
                // A hair wider, so neighbours meet without seams.
                let side = gpui::size(px(module + 0.3), px(module + 0.3));
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(origin, side),
                    black,
                ));
            }
        },
    )
    .size(px(module * width as f32));
    Some(
        div()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::BOX)
            .bg(gpui::rgb(0xffffff))
            .child(modules),
    )
}

/// What a plugin's note in a transcript says in its header.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteHead {
    /// The plugin's name, beside its icon.
    pub plugin: String,
    pub icon: Icon,
    pub tone: crate::theme::Tone,
    /// What it says, in marked-up prose.
    pub text: String,
    /// Cost, latency or confidence, in small print.
    pub detail: Option<String>,
}

/// What a click on a note's header does.
pub type OnClick =
    Box<dyn Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static>;

/// A plugin's note in a transcript: its header, and a body under it.
///
/// - `folds`: when the body folds, whether it is open; a click on the
///   header (`on_toggle`) opens or closes it. On a phone nothing folds.
/// - `link`: what sits at the header's end, such as "Details".
#[allow(clippy::too_many_arguments)]
pub fn note(
    id: impl Into<gpui::ElementId>,
    head: NoteHead,
    folds: Option<bool>,
    on_toggle: Option<OnClick>,
    link: Option<AnyElement>,
    body: Option<AnyElement>,
    compact: bool,
    t: &Theme,
) -> Div {
    let folds = folds.filter(|_| !compact);
    let open = folds.unwrap_or(true);
    let header = div()
        .id(id)
        .flex()
        .items_center()
        .gap(sp(2.))
        .when(folds.is_some(), |row| {
            row.cursor_pointer()
                .child(icon(
                    if open { Icon::Down } else { Icon::Chevron },
                    IconSize::SMALL,
                    t.dim,
                ))
                .when_some(on_toggle, |row, toggle| row.on_click(toggle))
        })
        .child(
            div()
                .size(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::TAG)
                .bg(t.info_surface)
                .child(icon(head.icon, IconSize::COMPACT, t.tone(head.tone))),
        )
        .child(
            mono(head.plugin.clone(), Type::CAPTION, t.tone(head.tone))
                .flex_shrink_0(),
        )
        .when(!compact, |row| {
            row.child(div().child(crate::prose::rich(
                &head.text,
                t.text_soft,
                t,
            )))
        })
        .child(div().flex_1())
        .when_some(head.detail.clone().filter(|_| !compact), |row, detail| {
            row.child(mono(detail, Type::MICRO, t.dim).flex_shrink_0())
        })
        .children(link);
    let indent = match (compact, folds.is_some()) {
        (true, _) => 0.,
        (false, true) => 12.,
        (false, false) => 7.,
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .px(sp(3.))
        .py(sp(2.))
        .rounded(radius::BOX)
        .bg(t.blue_soft)
        .border_1()
        .border_dashed()
        .border_color(t.blue_border)
        .child(header)
        .when(compact, |card| {
            card.child(
                div()
                    .text_color(t.text_soft)
                    .line_height(relative(1.45))
                    .child(crate::prose::rich(&head.text, t.text_soft, t)),
            )
        })
        .when_some(body.filter(|_| open), |card, body| {
            card.child(div().pl(sp(indent)).child(body))
        })
}
