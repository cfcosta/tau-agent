//! The components screens are built from: text, rules, actions, marks,
//! stamps, ticks, surfaces, fields and notices. Each takes its look from
//! the design tokens in [`crate::theme`], so a component looks the same
//! wherever it is used, and changing it here changes it everywhere.
//!
//! The look is flat and printed. Structure comes from rules: a section
//! is a title over a heavy rule ([`section_header`]), rows are split by
//! hairlines ([`rule`]), and nothing is boxed. The one decoration is the
//! hard offset: the primary action is a flat block printed over an
//! offset in another color ([`primary_block`]). Other actions are
//! underlined words ([`link_action`]). Quantities are square ticks
//! ([`ticks`]), and state is a small mark ([`mark`]), a stamp
//! ([`stamp`]), a square ([`square`]) or a slip ([`slip`]).
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
    prelude::*,
    px,
    relative,
    rems,
    svg,
};

pub use crate::assets::State;
use crate::{
    assets::Icon,
    input::TextInput,
    prose::prose,
    theme::{Design, IconSize, Theme, Tone, Type, control, radius, sp, weight},
};

// Surfaces.

/// Which side of the window a piece of chrome sits on: the side its
/// border faces away from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The title bar: its border is under it.
    Top,
    /// The status bar: its border is over it.
    Bottom,
    /// The sidebar: its border is on its right.
    Left,
    /// The inspector: its border is on its left.
    Right,
}

/// A hard offset `by` px down and right in `color`, with no blur: what
/// a printed block sits on.
fn offset(by: f32, color: Hsla) -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(by), px(by), color)]
}

/// How surfaces are told apart, for any element: flat fills, thin
/// outlines and the hard offset. Nothing is rounded, lit or blurred.
pub trait Material: Styled + Sized {
    /// What floats over the rest: menus, popovers, dialogs. The
    /// sidebar's fill, outlined.
    fn raised(self, t: &Theme) -> Self {
        self.bg(t.card).border_1().border_color(t.border)
    }

    /// A field: text fields and the terminal. An outline, a little
    /// stronger than a floating surface's, with no fill.
    fn well(self, t: &Theme) -> Self {
        self.border_1().border_color(t.border_strong)
    }

    /// The selected one of a set of options: an outline with no fill.
    fn key(self, t: &Theme) -> Self {
        self.border_1().border_color(t.border_strong)
    }

    /// A flat block in the primary color: a checked box, a switch that
    /// is on.
    fn accent_key(self, t: &Theme) -> Self {
        self.bg(t.roles.primary)
    }

    /// The offset print: a flat block in the primary color with dark
    /// text, over a hard offset `by` px down and right. For the primary
    /// action and a selected option; once or twice a screen.
    fn printed(self, by: f32, t: &Theme) -> Self {
        self.bg(t.roles.primary)
            .text_color(t.roles.on_primary)
            .shadow(offset(by, t.offset))
    }

    /// The selected row of a list.
    fn pressed(self, t: &Theme) -> Self {
        self.bg(t.selected).rounded(radius::SELECTED)
    }

    /// Chrome on one `edge` of the window: the sidebar's fill, and a
    /// border toward what it frames.
    fn chrome(self, edge: Edge, t: &Theme) -> Self {
        let framed = self.bg(t.panel).border_color(t.border);
        match edge {
            Edge::Top => framed.border_b_1(),
            Edge::Bottom => framed.border_t_1(),
            Edge::Left => framed.border_r_1(),
            Edge::Right => framed.border_l_1(),
        }
    }

    /// A key in its own colors, for looks with their own palette, such
    /// as onboarding's: filled with `top`.
    fn bevel(self, top: Hsla, _bottom: Hsla, _t: &Theme) -> Self {
        self.bg(top)
    }

    /// A field with its own `floor`.
    fn sunk(self, floor: Hsla, t: &Theme) -> Self {
        self.well(t).bg(floor)
    }

    /// Floating over the rest, keeping its fill: a hard offset in the
    /// hairline's color.
    fn lifted(self, t: &Theme) -> Self {
        self.shadow(offset(4., t.hairline))
    }
}

impl<E: Styled> Material for E {}

// Rules.

/// A hairline: between rows, and a thematic break in a reply.
pub fn rule(t: &Theme) -> Div {
    div()
        .w_full()
        .h(rems(0.0625))
        .flex_shrink_0()
        .bg(t.hairline)
}

/// The heavy rule a section's title sits on.
pub fn heavy_rule(t: &Theme) -> Div {
    div().w_full().h(rems(0.125)).flex_shrink_0().bg(t.rule)
}

/// A section's head: its title in the display face, `meta` in small
/// muted print at the right when there is one, over the heavy rule.
pub fn section_header(
    title: impl Into<SharedString>,
    meta: Option<SharedString>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(sp(3.))
        .pb(sp(2.5))
        .border_b_2()
        .border_color(t.rule)
        .child(
            div()
                .typeset(Type::SECTION)
                .text_color(t.bright)
                .child(title.into()),
        )
        .child(div().flex_1())
        .children(
            meta.map(|meta| text(meta, Type::SMALL, t.dim).flex_shrink_0()),
        )
}

// Text.

pub fn icon(icon: Icon, size: IconSize, color: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .size(rems((size.0) / 16.))
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

/// A small uppercase label over a group inside a section.
pub fn heading(text: &str, t: &Theme) -> Div {
    div()
        .typeset(Type::MICRO)
        .text_color(t.dim)
        .child(text.to_uppercase())
}

/// A screen's big title, in the display face at `style`'s size.
pub fn title(text: impl Into<SharedString>, style: Type) -> Div {
    div()
        .typeset(style.display())
        .leading(1.25)
        .child(text.into())
}

/// A big number or time, in the display face.
pub fn big_figure(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .typeset(Type::FIGURE)
        .text_color(t.bright)
        .child(text.into())
}

/// The paragraph under a big title. `code` spans are set in mono.
pub fn lead(text: &str, style: Type, t: &Theme) -> Div {
    div()
        .typeset(style)
        .leading(1.6)
        .child(prose(text, t.muted, t))
}

/// A screen's title, in the display face, and the sentence under it.
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
                .typeset(Type::PAGE_TITLE)
                .text_color(t.bright)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(rems(47.5))
                .text_color(t.muted)
                .leading(1.5)
                .child(subtitle.into()),
        )
}

/// A field's label above it.
pub fn label(text: &'static str, t: &Theme) -> Div {
    div().typeset(Type::SMALL).text_color(t.muted).child(text)
}

// Actions.

/// A word over a 1 px line in `line`, with an optional leading icon.
fn underlined(
    label: SharedString,
    glyph: Option<(Icon, IconSize)>,
    line: Hsla,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(1.5))
        .flex_shrink_0()
        .border_b_1()
        .border_color(line)
        .text_color(t.text)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|style| style.text_color(t.bright))
        .children(glyph.map(|(glyph, size)| icon(glyph, size, t.text)))
        .child(label)
}

/// An action that is not the screen's main one: a word underlined in
/// blue, at the size of the text around it. Never a bordered button.
pub fn link_action(label: impl Into<SharedString>, t: &Theme) -> Div {
    underlined(label.into(), None, t.blue, t)
}

/// An action that needs the person: a small block in the offset print,
/// at the size of the text around it.
pub fn hot_action(label: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .px(sp(1.75))
        .py(rems(0.0625))
        .printed(2., t)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .child(label.into())
}

/// [`link_action`] at `style`.
pub fn text_link(text: impl Into<SharedString>, style: Type, t: &Theme) -> Div {
    link_action(text, t).typeset(style)
}

/// [`link_action`] in small print: a link that navigates somewhere.
pub fn link(label: impl Into<SharedString>, t: &Theme) -> Div {
    text_link(label, Type::CAPTION, t)
}

/// How big a [`primary_block`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockSize {
    /// In a header or a list: 30 px.
    Small,
    /// Beside a composer: 34 px.
    Medium,
    /// A screen's main action: 40 px, over a deeper offset.
    Large,
}

/// The screen's primary action: a flat blue block with dark text over
/// a hard pink offset. Use it once or twice a screen.
pub fn primary_block(
    label: impl Into<SharedString>,
    size: BlockSize,
    t: &Theme,
) -> Div {
    block(label.into(), None, size, t)
}

fn block(
    label: SharedString,
    glyph: Option<Icon>,
    size: BlockSize,
    t: &Theme,
) -> Div {
    let (height, pad, style, by, glyph_size) = match size {
        BlockSize::Small => (30., 3., Type::SMALL, 3., IconSize::COMPACT),
        BlockSize::Medium => {
            (34., 3.5, Type::SMALL.sized(13.5), 3., IconSize::BASE)
        }
        BlockSize::Large => (40., 4.5, Type::BODY, 4., IconSize::LARGE),
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(sp(2.))
        .flex_shrink_0()
        .h(rems(height / 16.))
        .px(sp(pad))
        .printed(by, t)
        .typeset(style.weighted(weight::STRONG))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .children(
            glyph.map(|glyph| icon(glyph, glyph_size, t.roles.on_primary)),
        )
        .child(label)
}

/// What a button does, and so how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The one thing to do on a screen: the offset print.
    Primary,
    /// Anything else: a word underlined in blue.
    Secondary,
    /// Stopping or throwing away: a word underlined in red.
    Danger,
}

/// A small button for headers, lists and toolbars.
pub fn button(
    label: impl Into<SharedString>,
    kind: ButtonKind,
    t: &Theme,
) -> Div {
    match kind {
        ButtonKind::Primary => block(label.into(), None, BlockSize::Small, t),
        ButtonKind::Secondary => {
            underlined(label.into(), None, t.blue, t).typeset(Type::SMALL)
        }
        ButtonKind::Danger => {
            underlined(label.into(), None, t.red, t).typeset(Type::SMALL)
        }
    }
}

/// A big button for task screens and phones: 44 px, easy to hit, with an
/// optional leading icon.
pub fn big_button(
    label: impl Into<SharedString>,
    glyph: Option<Icon>,
    kind: ButtonKind,
    t: &Theme,
) -> Div {
    let line = match kind {
        ButtonKind::Primary => {
            return block(label.into(), glyph, BlockSize::Large, t)
                .min_h(control::LARGE);
        }
        ButtonKind::Secondary => t.blue,
        ButtonKind::Danger => t.red,
    };
    // The word is underlined, not the whole 44 px a finger hits.
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .min_h(control::LARGE)
        .cursor_pointer()
        .typeset(Type::BODY)
        .child(underlined(
            label.into(),
            glyph.map(|glyph| (glyph, IconSize::LARGE)),
            line,
            t,
        ))
}

/// A square 44 px button holding an icon: the phone's send and stop.
/// Outlined in `border` when there is one; else filled in `color`, and
/// printed over the offset when `color` is the accent.
pub fn square_button(
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
        .cursor_pointer();
    match border {
        Some(border) => button.key(t).border_color(border).child(icon(
            glyph,
            IconSize::BASE,
            color,
        )),
        None if color == t.accent => button.printed(3., t).child(icon(
            glyph,
            IconSize::XLARGE,
            t.roles.on_primary,
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
        .size(rems((size) / 16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(gpui::white().opacity(0.05)))
        .child(icon(glyph, IconSize::XLARGE, t.text_soft))
}

// Marks.

/// `text` in the display face, `size` px tall, as heiko's name is
/// printed: light, over a blue copy a little up and left and a pink
/// copy a little down and right.
fn overprinted(text: &'static str, size: f32, t: &Theme) -> Div {
    let (x, y) = (size / 14. / 16., size / 21. / 16.);
    let copy = |color: Hsla, x: f32, y: f32| {
        div()
            .absolute()
            .left(rems(x))
            .top(rems(y))
            .text_color(color)
            .child(text)
    };
    div()
        .relative()
        .flex_shrink_0()
        .typeset(Type::SECTION.sized(size).leading(1.))
        .whitespace_nowrap()
        .child(copy(t.offset, x, y))
        .child(copy(t.blue, -x, -y))
        .child(div().relative().text_color(t.bright).child(text))
}

/// heiko's wordmark: 並行 and the name, overprinted in blue and pink.
/// `size` is the height of its letters in px.
pub fn wordmark(size: f32, t: &Theme) -> Div {
    div()
        .flex()
        .items_baseline()
        .flex_shrink_0()
        .gap(rems(size / 4. / 16.))
        .child(overprinted("並行", size, t))
        .child(overprinted("heiko", size * 0.86, t))
}

/// heiko's mark for small places: 並 alone, overprinted like the
/// [`wordmark`], in a square `size` px wide.
pub fn logo(t: &Theme, size: f32) -> Div {
    div()
        .size(rems((size) / 16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(overprinted("並", size * 0.85, t))
}

/// The small mark a state is shown with, in its color: working is a
/// ringed green dot, asking an orange diamond, failing a red cross, a
/// met goal a green tick, a limit a red square, a cap an amber square,
/// and idle a hollow ring.
pub fn mark(state: State, t: &Theme) -> Svg {
    let color = match state {
        State::Working | State::Met => t.green,
        State::Asks => t.roles.live,
        State::Failing | State::Limit => t.red,
        State::Cap => t.roles.cost,
        State::Idle => t.slate,
    };
    svg()
        .path(state.path())
        .size(rems(0.875))
        .flex_shrink_0()
        .text_color(color)
}

/// A state's mark with how many things are in it before it, when more
/// than one: `3` and the diamond for three questions.
pub fn counted_mark(count: usize, state: State, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .gap(rems(0.1875))
        .when(count > 1, |row| {
            row.child(
                mono(count.to_string(), Type::MICRO.sized(11.5), t.text_soft)
                    .font_weight(weight::EMPHASIS),
            )
        })
        .child(mark(state, t))
}

/// A count on a small filled square: a conversation's unread replies.
pub fn unread_count(count: usize, t: &Theme) -> Div {
    mono(count.to_string(), Type::MICRO, t.roles.on_primary)
        .flex_shrink_0()
        .min_w(rems(1.125))
        .h(rems(1.125))
        .px(sp(1.))
        .flex()
        .items_center()
        .justify_center()
        .bg(t.accent)
        .font_weight(weight::EMPHASIS)
}

pub fn dot(color: Hsla, size: f32) -> Div {
    div()
        .size(rems((size) / 16.))
        .flex_shrink_0()
        .rounded(radius::FULL)
        .bg(color)
}

/// A dot in a faint ring of its color: something running now.
pub fn live_dot(color: Hsla, size: f32) -> Div {
    div()
        .size(rems((size * 1.75) / 16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::FULL)
        .border_1()
        .border_color(color.opacity(0.4))
        .child(dot(color, size))
}

/// A status as a plain mark: a dot and words in `color`, with no fill.
pub fn status_mark(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .gap(sp(1.5))
        .text_color(color)
        .typeset(Type::CAPTION)
        .child(dot(color, 6.))
        .child(text.into())
}

/// A stamp: a word or two in mono, outlined in its tone over a hard
/// offset. `exit 0`, `queued`, `Draft`.
pub fn stamp(label: impl Into<SharedString>, tone: Tone, t: &Theme) -> Div {
    stamp_in(label, t.tone(tone), t)
}

/// A [`stamp`] in a `color` of its own.
pub fn stamp_in(label: impl Into<SharedString>, color: Hsla, t: &Theme) -> Div {
    mono(label, Type::CAPTION, color)
        .flex_shrink_0()
        .px(sp(2.))
        .py(rems(0.0625))
        .border(px(1.5))
        .border_color(color)
        .shadow(offset(3., t.hairline))
}

/// How a [`square`] is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SquareStyle {
    /// Not reached yet: a muted outline and number.
    Quiet,
    /// Outlined and numbered in a color.
    Outlined(Hsla),
    /// Filled with a color, its number dark. Never a warm color.
    Filled(Hsla),
}

/// A small square holding a number or a letter: a step, a check, a
/// place in a queue.
pub fn square(
    label: impl Into<SharedString>,
    style: SquareStyle,
    t: &Theme,
) -> Div {
    let (line, fill, ink) = match style {
        SquareStyle::Quiet => (t.slate, None, t.dim),
        SquareStyle::Outlined(color) => (color, None, color),
        SquareStyle::Filled(color) => (color, Some(color), t.roles.on_primary),
    };
    mono(label, Type::CAPTION, ink)
        .size(rems(1.5))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .border(px(1.5))
        .border_color(line)
        .when_some(fill, |square, fill| square.bg(fill))
        .font_weight(weight::EMPHASIS)
}

/// A [`slip`] in a `color` of its own; `dashed` for one proposed, not
/// kept yet.
pub fn slip_in(
    content: impl IntoElement,
    color: Hsla,
    dashed: bool,
    t: &Theme,
) -> Div {
    // A row, so the slip is as wide as what it holds and no wider.
    div().flex().child(
        div()
            .max_w_full()
            .px(sp(2.75))
            .py(sp(1.5))
            .bg(t.bg)
            .border(px(1.5))
            .border_color(color)
            .when(dashed, |slip| slip.border_dashed())
            .shadow(offset(4., t.under(color)))
            .typeset(Type::SMALL.sized(13.5).leading(1.5))
            .text_color(t.text)
            .child(content),
    )
}

/// A slip: a note or an id on a label outlined in its tone, over a
/// hard offset in a darker shade of the same color.
pub fn slip(content: impl IntoElement, tone: Tone, t: &Theme) -> Div {
    slip_in(content, t.tone(tone), false, t)
}

/// A [`slip`] with a dashed outline: one proposed, not kept yet.
pub fn dashed_slip(content: impl IntoElement, tone: Tone, t: &Theme) -> Div {
    slip_in(content, t.tone(tone), true, t)
}

/// A name in mono, underlined as a word the person can change, with an
/// optional leading icon: a branch, a repository, a model.
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
        .border_b_1()
        .border_color(t.slate)
        .children(glyph.map(|glyph| icon(glyph, IconSize::COMPACT, color)))
        .child(mono(text, style, color))
}

/// A short value in mono on a flat band: a field name, a path, a
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
        .bg(t.raised)
}

/// What the user wrote, between two hairlines.
pub fn bubble(t: &Theme) -> Div {
    div()
        .py(sp(3.5))
        .border_y_1()
        .border_color(t.hairline)
        .leading(1.55)
}

// Ticks.

/// How many ticks a [`bar`] has.
const BAR_TICKS: usize = 10;

fn tick_strip(
    lit: usize,
    total: usize,
    height: f32,
    color: Hsla,
    track: Hsla,
    marked: &[(usize, Hsla)],
) -> Div {
    let marked = |n: usize| {
        marked
            .iter()
            .find(|(at, _)| *at == n)
            .map(|(_, color)| *color)
    };
    div()
        .flex()
        .w_full()
        .gap(rems(0.1875))
        .children((0..total).map(|n| {
            div()
                .flex_1()
                .min_w(rems(0.))
                .h(rems(height / 16.))
                .bg(marked(n).unwrap_or(if n < lit { color } else { track }))
        }))
}

/// A quantity as a strip of `total` square ticks, the first `lit` of
/// them in `color`. Never a smooth bar.
pub fn ticks(lit: usize, total: usize, color: Hsla, t: &Theme) -> Div {
    marked_ticks(lit, total, color, &[], t)
}

/// [`ticks`], with the ticks at the indexes in `marked` drawn white:
/// thresholds on the strip.
pub fn marked_ticks(
    lit: usize,
    total: usize,
    color: Hsla,
    marked: &[usize],
    t: &Theme,
) -> Div {
    let marked: Vec<(usize, Hsla)> =
        marked.iter().map(|at| (*at, t.bright)).collect();
    tick_strip(lit, total, 10., color, t.hairline, &marked)
}

/// A strip of ticks, each in its own color: a whole in its parts.
pub fn colored_ticks(colors: &[Hsla], _: &Theme) -> Div {
    div().flex().w_full().gap(rems(0.1875)).children(
        colors.iter().map(|color| {
            div().flex_1().min_w(rems(0.)).h(rems(0.625)).bg(*color)
        }),
    )
}

/// A share as ticks `height` px tall: `share` of them in `fill`, the
/// rest in `track`. Any share above none lights a tick.
pub fn bar(share: f32, height: f32, fill: Hsla, track: Hsla) -> Div {
    let share = share.clamp(0.0, 1.0);
    let lit = (share * BAR_TICKS as f32).round() as usize;
    let lit = if share > 0. { lit.max(1) } else { 0 };
    tick_strip(lit, BAR_TICKS, height, fill, track, &[])
}

// Blocks.

/// A block in a column of them, under a hairline, clipping what it
/// holds. No fill and no outline.
pub fn card(t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(t.hairline)
        .overflow_hidden()
}

/// A block on a task screen, under a hairline, with `steps` of room
/// above and below what it holds. No fill and no outline.
pub fn panel(steps: f32, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .py(sp(steps))
        .border_t_1()
        .border_color(t.hairline)
}

/// The flat darker plane a result sits on: what it holds one under the
/// other, padded.
pub fn band(t: &Theme) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .px(sp(3.5))
        .py(sp(2.25))
        .bg(t.card)
}

/// Rows one under the other, a line down the left joining the squares
/// they start with: the steps of a change, a queue. Each row starts
/// with a [`square`] at its top.
pub fn rail(rows: Vec<AnyElement>, t: &Theme) -> Div {
    let last = rows.len().saturating_sub(1);
    div().flex().flex_col().gap(sp(3.)).children(
        rows.into_iter().enumerate().map(|(n, row)| {
            div()
                .relative()
                .when(n < last, |joined| {
                    // From under this row's square to the next row's.
                    joined.child(
                        div()
                            .absolute()
                            .top(rems(1.5))
                            .bottom(sp(-3.))
                            .left(rems(0.65625))
                            .w(rems(0.1875))
                            .bg(t.hairline),
                    )
                })
                .child(row)
        }),
    )
}

/// A 56 px glyph in a square outlined in its color over a hard offset,
/// above a phone screen's title: done, or what went wrong.
pub fn tile(glyph: Icon, color: Hsla, t: &Theme) -> Div {
    div()
        .size(rems(3.5))
        .flex()
        .items_center()
        .justify_center()
        .bg(t.bg)
        .border(px(1.5))
        .border_color(color)
        .shadow(offset(4., t.under(color)))
        .child(icon(glyph, IconSize(28.), color))
}

/// A small labelled number.
pub fn stat(label: &str, value: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(1.))
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
                        .w(rems(8.))
                        .flex_shrink_0(),
                )
                .child(div().flex_1().min_w(rems(0.)).child(value))
        }))
}

/// A line of news under a rule in its `tone`: waiting, or a failure.
/// No fill.
pub fn notice(
    glyph: Icon,
    text: impl Into<SharedString>,
    tone: Hsla,
    style: Type,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .pt(sp(2.5))
        .border_t_2()
        .border_color(tone)
        .typeset(style)
        .text_color(if tone == t.red { t.red } else { t.text_soft })
        .child(icon(glyph, IconSize::LARGE, tone))
        .child(div().flex_1().min_w(rems(0.)).child(text.into()))
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

/// A modal over a dimmed backdrop: a title in the display face, a
/// message, then anything the message asks for (a field, a choice)
/// above the actions.
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
                .w(rems(27.5))
                .max_w_full()
                .flex()
                .flex_col()
                .gap(sp(3.))
                .p(sp(5.))
                .raised(t)
                .border_color(t.border_strong)
                .lifted(t)
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
                                .min_w(rems(0.))
                                .typeset(Type::SECTION.sized(17.))
                                .text_color(t.bright)
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
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(sp(4.))
                        .child(actions),
                ),
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

/// A text field in a square 44 px outline.
pub fn field(input: &Entity<TextInput>, mono: bool, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .h(control::LARGE)
        .px(sp(3.))
        .well(t)
        .typeset(if mono { Type::SMALL.mono() } else { Type::BODY })
        .child(input.clone())
}

/// An on/off switch, as two squares: a block that sits left in an
/// outline when off, and right in a filled track when on. Callers add
/// the id and the click.
pub fn switch(on: bool, t: &Theme) -> Div {
    div()
        .w(rems(2.25))
        .h(rems(1.25))
        .flex_shrink_0()
        .flex()
        .items_center()
        .p(sp(0.5))
        .border_1()
        .when(on, |track| {
            track
                .accent_key(t)
                .border_color(t.roles.primary)
                .justify_end()
        })
        .when(!on, |track| track.border_color(t.border_strong))
        .cursor_pointer()
        .child(div().size(rems(0.875)).bg(if on {
            t.roles.on_primary
        } else {
            t.slate
        }))
}

/// A square checkbox: 18 px, or 20 px for a phone's touch rows.
pub fn checkbox(checked: bool, large: bool, t: &Theme) -> Div {
    let size = if large { 20. } else { 18. };
    let checkbox = div()
        .size(rems((size) / 16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    if checked {
        checkbox.accent_key(t).child(icon(
            Icon::Check,
            if large {
                IconSize::BASE
            } else {
                IconSize::SMALL
            },
            t.roles.on_primary,
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
        .min_h(rems(0.))
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
        .min_h(rems(0.))
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

/// A table from a model's reply: a header row in small muted print
/// over a rule (left out with its rule when `head` is empty), rows
/// split by hairlines, each cell lined up as its
/// column says. Columns share the width; long cells wrap.
pub fn table(
    align: &[crate::markdown::Align],
    head: Vec<AnyElement>,
    rows: Vec<Vec<AnyElement>>,
    t: &Theme,
) -> Div {
    weighted_table(align, &[], head, rows, t)
}

/// [`table`], each column taking its share of the width by `weights`;
/// a column without one takes a share of 1.
pub fn weighted_table(
    align: &[crate::markdown::Align],
    weights: &[f32],
    head: Vec<AnyElement>,
    rows: Vec<Vec<AnyElement>>,
    t: &Theme,
) -> Div {
    use crate::markdown::Align;
    let row = |cells: Vec<AnyElement>, header: bool| {
        div()
            .flex()
            .w_full()
            .border_color(t.hairline)
            .when(!header, |row| row.border_b_1())
            .when(header, |row| {
                row.border_b_2()
                    .border_color(t.slate)
                    .text_color(t.dim)
                    .typeset(Type::CAPTION)
            })
            .children(cells.into_iter().enumerate().map(|(n, content)| {
                let weight = weights.get(n).copied().unwrap_or(1.).max(0.1);
                let cell = div()
                    .flex_grow(weight)
                    .flex_shrink(1.)
                    .flex_basis(rems(0.))
                    .min_w(rems(0.))
                    .pr(sp(3.5))
                    .py(sp(2.));
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
        .typeset(Type::SMALL)
        .when(!head.is_empty(), |table| table.child(row(head, true)))
        .children(rows.into_iter().map(|cells| row(cells, false)))
}

/// A fenced block of code from a reply, in the monospace face on a
/// flat band, with its language in the corner.
pub fn code_block(lang: Option<&str>, text: &str, t: &Theme) -> Div {
    numbered_code_block(lang, text, None, t)
}

/// [`code_block`], with each line's number in a gutter from `first`
/// when given. Numbered lines do not wrap, so each number stays beside
/// its line.
pub fn numbered_code_block(
    lang: Option<&str>,
    text: &str,
    first: Option<u32>,
    t: &Theme,
) -> Div {
    let gutter = first.map(|first| {
        let count = text.lines().count().max(1) as u32;
        let numbers: Vec<String> =
            (first..first + count).map(|n| n.to_string()).collect();
        div()
            .flex_shrink_0()
            .pr(sp(3.))
            .text_right()
            .typeset(Type::SMALL.mono())
            .text_color(t.dim)
            .child(numbers.join("\n"))
    });
    div()
        .relative()
        .w_full()
        .px(sp(3.5))
        .py(sp(3.))
        .bg(t.card)
        .child(
            div().flex().children(gutter).child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .when(first.is_some(), |code| {
                        code.whitespace_nowrap().overflow_x_hidden()
                    })
                    .child(
                        match lang
                            .and_then(crate::syntax::Lang::of_name)
                            .filter(|_| !text.is_empty())
                        {
                            Some(lang) => {
                                let parts =
                                    crate::syntax::highlight_cached(lang, text);
                                div()
                                    .typeset(Type::SMALL.mono())
                                    .text_color(t.text_soft)
                                    .child(crate::select::selectable(
                                        gpui::StyledText::new(text.to_owned())
                                            .with_runs(crate::syntax::runs(
                                                text,
                                                &parts,
                                                t.text_soft,
                                                &t.syntax,
                                            )),
                                    ))
                            }
                            None => div()
                                .typeset(Type::SMALL.mono())
                                .text_color(t.text_soft)
                                .child(crate::select::selectable(
                                    gpui::StyledText::new(text.to_owned()),
                                )),
                        },
                    ),
            ),
        )
        .children(lang.map(|lang| {
            div().absolute().top(sp(1.5)).right(sp(2.5)).child(mono(
                lang.to_owned(),
                Type::MICRO,
                t.dim,
            ))
        }))
}

/// One line of code in the mono face, the byte ranges in `marked` on a
/// flat blue ground: a line a search found, and where. It does not
/// wrap.
pub fn marked_line(
    text: &str,
    marked: &[std::ops::Range<usize>],
    t: &Theme,
) -> Div {
    let run = |len: usize, on: bool| gpui::TextRun {
        len,
        font: gpui::font(crate::theme::MONO),
        color: if on { t.bright } else { t.text_soft },
        background_color: on.then_some(t.accent_border),
        underline: None,
        strikethrough: None,
    };
    // A range's end moved back to where a character starts.
    let floor = |mut at: usize| {
        at = at.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    let mut ranges: Vec<(usize, usize)> = marked
        .iter()
        .map(|range| (floor(range.start), floor(range.end)))
        .collect();
    ranges.sort_unstable();
    let mut runs = Vec::new();
    let mut at = 0;
    for (start, end) in ranges {
        let start = start.max(at);
        if start >= end {
            continue;
        }
        if start > at {
            runs.push(run(start - at, false));
        }
        runs.push(run(end - start, true));
        at = end;
    }
    if at < text.len() {
        runs.push(run(text.len() - at, false));
    }
    div()
        .typeset(Type::CAPTION.mono())
        .text_color(t.text_soft)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(crate::select::selectable(if runs.is_empty() {
            gpui::StyledText::new(text.to_owned())
        } else {
            gpui::StyledText::new(text.to_owned()).with_runs(runs)
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
                .min_w(rems(0.))
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
            // A module's side as drawn, at whatever the zoom is.
            let module = bounds.size.width / width as f32;
            for (n, _) in dark.iter().enumerate().filter(|(_, dark)| **dark) {
                let (x, y) = ((n % width) as f32, (n / width) as f32);
                let origin = gpui::point(
                    bounds.left() + module * x,
                    bounds.top() + module * y,
                );
                // A hair wider, so neighbours meet without seams.
                let side = gpui::size(module + px(0.3), module + px(0.3));
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(origin, side),
                    black,
                ));
            }
        },
    )
    .size(rems((module * width as f32) / 16.));
    Some(
        div()
            .size(rems((size) / 16.))
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgb(0xffffff))
            .child(modules),
    )
}

/// A repository's mark: its letter in mono, dark on a flat block of
/// its color, 20 px in the sidebar and 26 px on a phone.
pub fn repo_mark(name: &str, size: f32, t: &Theme) -> Div {
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or("?".into(), |c| c.to_lowercase().to_string());
    let mark = t.mark(name);
    div()
        .size(rems((size) / 16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SELECTED)
        .bg(mark)
        .text_color(t.roles.on_primary)
        .typeset(Type::MICRO.sized(size * 0.6).mono())
        .font_weight(weight::EMPHASIS)
        .child(letter)
}

/// What a plugin's note in a transcript says in its header.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteHead {
    /// The plugin's name, beside its icon.
    pub plugin: String,
    pub icon: Icon,
    pub tone: Tone,
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
    // A plugin speaks in its own color; a note that warns, in its tone.
    let warns = matches!(head.tone, Tone::Danger | Tone::Warn);
    let ink = match head.tone {
        Tone::Info | Tone::Quiet => {
            t.roles.plugin(&head.plugin).unwrap_or(t.tone(head.tone))
        }
        tone => t.tone(tone),
    };
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
        .child(icon(head.icon, IconSize::COMPACT, ink))
        .child(
            div()
                .flex_shrink_0()
                .text_color(ink)
                .child(head.plugin.clone()),
        )
        .when(!compact && !head.text.is_empty(), |row| {
            row.child(
                div()
                    .min_w(rems(0.))
                    .child(crate::prose::rich(&head.text, t.muted, t)),
            )
        })
        .child(div().flex_1())
        .when_some(head.detail.clone().filter(|_| !compact), |row, detail| {
            row.child(mono(detail, Type::MICRO, t.dim).flex_shrink_0())
        })
        .children(link);
    let indent = match (compact, folds.is_some()) {
        (true, _) => 0.,
        (false, true) => 9.,
        (false, false) => 5.,
    };
    // A note is a line of the transcript, quieter than what the agent
    // says; one that warns sits under a rule in its tone.
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .typeset(Type::CAPTION)
        .when(warns, |card| {
            card.pt(sp(2.5)).border_t_2().border_color(ink)
        })
        .child(header)
        .when(compact && !head.text.is_empty(), |card| {
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

/// A key, as a popover's hints show it: its name in small mono.
pub fn key_hint(label: &'static str, t: &Theme) -> Div {
    mono(label, Type::MICRO, t.dim)
}

/// The keys a popover takes, in a row at its foot.
pub fn hints(keys: &[(&'static str, &'static str)], t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(3.5))
        .px(sp(2.5))
        .pt(sp(2.))
        .pb(sp(1.))
        .mt(sp(1.))
        .border_t_1()
        .border_color(t.hairline)
        .children(keys.iter().map(|(label, does)| {
            div()
                .flex()
                .items_center()
                .gap(sp(1.5))
                .child(key_hint(label, t))
                .child(text(*does, Type::CAPTION, t.dim))
        }))
}
