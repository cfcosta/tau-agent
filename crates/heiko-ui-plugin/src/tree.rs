//! The view tree (ADR 0045): what every plugin draws with, Rust or Luau.
//! A tree is plain data, the same on the computer and the phone, and the
//! interface draws it with heiko-ui-kit's pieces.
//!
//! On the wire a piece is a JSON object named by its `piece` key, as
//! heiko-luau-plugins' `heiko.ui` builds it; a bare string is a `text`. A
//! piece that does not read draws as a red box that says why, so one
//! bad piece never hides the rest.

use std::rc::Rc;

use gpui::{
    AnyElement,
    App,
    IntoElement,
    ParentElement,
    SharedString,
    Styled,
    div,
    prelude::*,
    relative,
    rems,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use heiko_ui_kit::{
    assets::Icon,
    components::{
        BlockSize,
        ButtonKind,
        SquareStyle,
        State,
        button,
        empty,
        field,
        heading,
        icon,
        link,
        mono,
        printed_qr_code,
        qr_code,
        rule,
        section_header,
        stamp,
        text_link,
    },
    prose::{markdown, rich},
    theme::{self, Design as _, IconSize, Theme, Type, sp},
};

use crate::Link;

/// How deep a tree may nest, and how many pieces a list or a row may
/// hold, before the rest is left out.
pub const MAX_DEPTH: usize = 12;
pub const MAX_CHILDREN: usize = 200;

/// How a piece reads, by name on the wire.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    #[default]
    Neutral,
    Good,
    Warn,
    Bad,
    Info,
}

impl From<Tone> for theme::Tone {
    fn from(tone: Tone) -> Self {
        match tone {
            Tone::Neutral => theme::Tone::Quiet,
            Tone::Good => theme::Tone::Good,
            Tone::Warn => theme::Tone::Warn,
            Tone::Bad => theme::Tone::Danger,
            Tone::Info => theme::Tone::Info,
        }
    }
}

/// What a button is for, as its look says.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Primary,
    #[default]
    Secondary,
    Danger,
    /// One that needs the person: a small block in the offset print.
    Hot,
}

impl From<Kind> for ButtonKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Primary | Kind::Hot => ButtonKind::Primary,
            Kind::Secondary => ButtonKind::Secondary,
            Kind::Danger => ButtonKind::Danger,
        }
    }
}

/// How big a primary [`Node::Button`] is, by name on the wire.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ButtonSize {
    /// In a header or a list.
    #[default]
    Small,
    /// Beside a composer.
    Medium,
    /// A screen's main action.
    Large,
}

impl From<ButtonSize> for BlockSize {
    fn from(size: ButtonSize) -> Self {
        match size {
            ButtonSize::Small => BlockSize::Small,
            ButtonSize::Medium => BlockSize::Medium,
            ButtonSize::Large => BlockSize::Large,
        }
    }
}

/// Whether a value is what a builder leaves out, and so is not written.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// The color a mark, a stamp, a square or a slip is printed in, by name
/// on the wire: one of the tones, or one of the palette's other colors.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Ink {
    #[default]
    Neutral,
    Good,
    Warn,
    Bad,
    Info,
    Teal,
    Cyan,
    Purple,
    Yellow,
    Bright,
}

impl Ink {
    /// The tone it is, when it is one.
    fn tone(self) -> Option<theme::Tone> {
        Some(match self {
            Ink::Neutral => theme::Tone::Quiet,
            Ink::Good => theme::Tone::Good,
            Ink::Warn => theme::Tone::Warn,
            Ink::Bad => theme::Tone::Danger,
            Ink::Info => theme::Tone::Info,
            Ink::Teal | Ink::Cyan | Ink::Purple | Ink::Yellow | Ink::Bright => {
                return None;
            }
        })
    }

    /// The color a stamp or a slip is outlined in: its tone's, or its
    /// own.
    fn outline(self, t: &Theme) -> gpui::Hsla {
        self.tone().map_or_else(|| self.color(t), |tone| t.tone(tone))
    }

    /// Its color; neutral is the dim of small print.
    fn color(self, t: &Theme) -> gpui::Hsla {
        match self {
            Ink::Neutral => t.dim,
            Ink::Teal => t.roles.memory,
            Ink::Cyan => t.roles.agent,
            Ink::Purple => t.change,
            Ink::Yellow => t.roles.cost,
            Ink::Bright => t.bright,
            Ink::Good => t.tone(theme::Tone::Good),
            Ink::Warn => t.tone(theme::Tone::Warn),
            Ink::Bad => t.tone(theme::Tone::Danger),
            Ink::Info => t.tone(theme::Tone::Info),
        }
    }
}

/// The state a [`Node::Mark`] shows, by name on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkState {
    Working,
    Asks,
    Failing,
    Met,
    Limit,
    Cap,
    Idle,
}

impl From<MarkState> for State {
    fn from(state: MarkState) -> Self {
        match state {
            MarkState::Working => State::Working,
            MarkState::Asks => State::Asks,
            MarkState::Failing => State::Failing,
            MarkState::Met => State::Met,
            MarkState::Limit => State::Limit,
            MarkState::Cap => State::Cap,
            MarkState::Idle => State::Idle,
        }
    }
}

/// How a [`Node::Square`] is drawn, by name on the wire: not reached yet,
/// outlined in its color, or filled with it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SquareLook {
    #[default]
    Quiet,
    Outlined,
    Filled,
}

/// How a [`Node::Step`] stands, by name on the wire.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    #[default]
    Done,
    Running,
    Failed,
    /// It waits on the person.
    Waits,
    /// Cancelled, or stopped before it ended.
    Stopped,
}

/// How much room a [`Node::Gap`] leaves, by name on the wire.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum GapSize {
    Small,
    #[default]
    Medium,
    Large,
}

/// A label under a tick of [`Node::Ticks`]: the tick's index, from 0,
/// and what it says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickLabel {
    pub at: u32,
    pub text: String,
}

/// One of the choices of [`Node::Segments`]: what it says, and the
/// value a click on it sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub label: String,
    pub value: Value,
}

/// A line of [`Node::Rows`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// One piece of a view tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "piece", rename_all = "lowercase")]
pub enum Node {
    Text {
        text: String,
    },
    /// Text with `code` and `**bold**`.
    Rich {
        text: String,
    },
    Mono {
        text: String,
    },
    /// A small label over a group, in `tone` when given.
    Heading {
        text: String,
        #[serde(default, skip_serializing_if = "is_default")]
        tone: Ink,
    },
    /// A section's head, which does not fold: its title over the heavy
    /// rule, `meta` in small print at the right, then `actions`.
    Header {
        title: String,
        #[serde(default)]
        meta: Option<String>,
        #[serde(default, deserialize_with = "nodes")]
        actions: Vec<Node>,
    },
    /// Markdown, as a model's reply draws: paragraphs, lists, code,
    /// tables, links that open in the browser.
    Markdown {
        text: String,
    },
    /// One of the interface's icons, by its file's name: `sub-agent`.
    Icon {
        name: String,
        #[serde(default)]
        tone: Tone,
    },
    Badge {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    /// A word or two in a tone, with no frame: a count, a state.
    Pill {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    /// A mark, a line and small print: a plugin's status.
    Status {
        text: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        tone: Tone,
    },
    Rows {
        #[serde(default)]
        rows: Vec<KeyValue>,
    },
    /// A header row over rows of pieces.
    /// `widths` are the columns' shares of the width; 1 each when not
    /// given.
    Table {
        #[serde(default)]
        head: Vec<String>,
        #[serde(default, deserialize_with = "table_rows")]
        rows: Vec<Vec<Node>>,
        #[serde(default)]
        widths: Vec<f32>,
    },
    /// A page's title, with what it is about under it.
    Title {
        text: String,
        #[serde(default)]
        subtitle: Option<String>,
        /// The title's ink; a note's title takes its tone.
        #[serde(default, skip_serializing_if = "is_default")]
        tone: Ink,
    },
    List {
        #[serde(default, deserialize_with = "nodes")]
        items: Vec<Node>,
    },
    /// A strip of ticks `share` of the way lit, from 0 to 1.
    Progress {
        #[serde(default)]
        share: f64,
        #[serde(default)]
        label: Option<String>,
    },
    /// `first_line`, when given, numbers its lines from it.
    Code {
        #[serde(default)]
        lang: Option<String>,
        text: String,
        #[serde(default)]
        first_line: Option<u32>,
    },
    /// A line in a tone: a passed test in green, a line that matters
    /// less dimmed (neutral). `mono` sets it in the monospace face.
    Toned {
        text: String,
        tone: Tone,
        #[serde(default)]
        mono: bool,
    },
    /// Pieces one under the other.
    Stack {
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// Pieces side by side, wrapping. After a [`Node::Kanji`] they do
    /// not wrap under it: the glyph keeps its width and the rest takes
    /// what is left.
    Row {
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// Pieces on one line that never wraps: the last takes what is left
    /// of the width, and what does not fit of it is cut.
    Line {
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// Pieces one under the other that scroll past `height` rems.
    Scroll {
        #[serde(default)]
        height: Option<f32>,
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// A line across.
    Rule,
    /// Room between two pieces one under the other.
    Gap {
        #[serde(default)]
        size: GapSize,
    },
    /// Pieces in a group of their own, under a hairline.
    Card {
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// Pieces one under the other on the flat darker plane, padded: a
    /// result.
    Band {
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// Pieces one under the other, a line down the left joining the
    /// squares they start with: the steps of a change, a queue.
    Rail {
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// A setting's row: its label and small print, and a switch showing
    /// `on`. A click carries out `action`, the plugin's own.
    Switch {
        label: String,
        #[serde(default)]
        detail: Option<String>,
        on: bool,
        action: String,
        #[serde(default)]
        args: Value,
    },
    /// A heading that opens and closes the pieces under it. Whether it
    /// is open is the interface's state, under the plugin's `id`; until
    /// the person clicks, `open` says. A `small` one is a line inside a
    /// note: its title in the mono face, over a hairline once open,
    /// with the pieces of `lead` before the title.
    Section {
        id: String,
        title: String,
        #[serde(default)]
        open: bool,
        #[serde(default)]
        small: bool,
        #[serde(default, deserialize_with = "nodes")]
        lead: Vec<Node>,
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// A text input whose draft is the interface's state, under the
    /// plugin's `id`. Enter carries out `submit`, when given, with
    /// `{ "text": draft }`, and clears the draft.
    Input {
        id: String,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        multiline: bool,
        #[serde(default)]
        submit: Option<String>,
        /// Whether Enter keeps the draft: a value to keep, not a message
        /// to send.
        #[serde(default)]
        keep: bool,
        /// A secret: drawn as dots, and its draft stays in the window that
        /// edits it, never in the state the interfaces share; only Enter
        /// gives it to the plugin.
        #[serde(default)]
        secret: bool,
        /// Raised to give it the keys: each new number moves them here
        /// once, so drawing again never takes them back.
        #[serde(default)]
        focus: Option<u64>,
    },
    /// An area that takes the keys (ADR 0045): while it has them, each of
    /// its `keys` (`up`, `enter`, `n`, ...) carries out `action` with
    /// `{ "key": k, "field": false }`; in a field inside it, `through`
    /// keys still come to it with `"field": true`. A click outside its
    /// fields gives it the keys, and a new `focus` number does too.
    Keys {
        id: String,
        #[serde(default)]
        keys: Vec<String>,
        #[serde(default)]
        through: Vec<String>,
        action: String,
        #[serde(default)]
        focus: Option<u64>,
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// A panel over the page, for a choice or a form that stops the rest
    /// until it is done. The tree holds it only while it is open; a
    /// click outside it carries out `dismiss`, the plugin's own action.
    Modal {
        title: String,
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
        #[serde(default)]
        dismiss: Option<String>,
    },
    /// A picture the interface fetches and keeps (ADR 0045): `key` names
    /// it, such as its blob's digest; `fetch` is the plugin's own action
    /// that asks its host half for it, answered with `{ "data": base64 }`
    /// or `{ "error": why }`; `mime` is its type. It fits `height` rems,
    /// keeping its shape.
    Image {
        key: String,
        mime: String,
        fetch: Value,
        #[serde(default)]
        height: Option<f32>,
    },
    /// `text` as a QR code `size` pixels wide, for a phone's camera: a
    /// pairing code. With `offset`, printed over the hard offset.
    Qr {
        text: String,
        size: f32,
        #[serde(default, skip_serializing_if = "is_default")]
        offset: bool,
    },
    /// A tree the interface fetches and keeps (ADR 0045), as a picture:
    /// `key` names it, and changes when what it shows does; `fetch` is the
    /// plugin's own action that asks its host half for it, answered with
    /// `{ "tree": node }` or `{ "error": why }`. Its actions are the
    /// plugin's, as the rest of the tree's.
    Fetched {
        key: String,
        fetch: Value,
    },
    /// A unified diff, each line after its number in its version of the
    /// file, colored as `path`'s language when known.
    Diff {
        text: String,
        #[serde(default)]
        path: Option<String>,
    },
    /// One line of code, the `spans` of it on a flat blue ground: a line
    /// a search found, and where. A span is the first and last byte it
    /// covers, counted from 1.
    Marked {
        text: String,
        #[serde(default)]
        spans: Vec<(u32, u32)>,
    },
    /// What a program drew on a terminal of `cols` by `rows`, as the bytes
    /// it wrote (or a replay of them); `live` while it is still writing.
    /// The interface keeps its screen under the plugin's `id`, with its
    /// scrollback and selection.
    Terminal {
        id: String,
        #[serde(default)]
        text: String,
        #[serde(default = "default_cols")]
        cols: u16,
        #[serde(default = "default_rows")]
        rows: u16,
        #[serde(default)]
        live: bool,
        /// Whether it folds to a few rows, with a button that shows them
        /// all; the interface keeps which.
        #[serde(default)]
        fold: bool,
    },
    /// The time since `since`, counting while `until` is not given, in
    /// milliseconds since 1970: the state says when, the interface counts.
    Ticker {
        id: String,
        since: i64,
        #[serde(default)]
        until: Option<i64>,
    },
    /// The time left until `at`, in milliseconds since 1970: `in 5m`,
    /// then `now`. The state says when, the interface counts down.
    Due {
        id: String,
        at: i64,
    },
    /// How long ago `at` was, in milliseconds since 1970: `now`, then
    /// `4m ago`. The state says when, the interface counts.
    Ago {
        id: String,
        at: i64,
    },
    /// Room that pushes what follows in a row to its end.
    Spacer,
    /// A small map: nodes at fractions of a box `height` rems tall,
    /// joined by `edges` of node ids.
    Graph {
        #[serde(default = "default_graph_height")]
        height: f32,
        #[serde(default)]
        nodes: Vec<GraphNode>,
        #[serde(default)]
        edges: Vec<(String, String)>,
    },
    /// Columns of cards. Dragging a card to another column carries out
    /// `action` with `{ card, from, to }`, the card's id and the two
    /// columns': what the move stands for is the plugin's to do. Too
    /// narrow for its columns, as on a phone, it shows one at a time, as
    /// tabs.
    Board {
        action: String,
        #[serde(default)]
        columns: Vec<BoardColumn>,
    },
    /// Tabs, one shown at a time. Which is chosen is the interface's
    /// state, under the plugin's `id`; until the person picks, the first.
    Tabs {
        id: String,
        #[serde(default)]
        tabs: Vec<Tab>,
    },
    /// What shows where there is nothing yet.
    Empty {
        text: String,
    },
    /// A button that asks the plugin to carry out `action` with `args`.
    /// With no label and an icon, a small square around the icon.
    /// `size` is a primary button's. A `wide` one takes the whole
    /// width, its label at the left; a wide primary one may have
    /// `detail`, a second line in smaller text inside the block.
    Button {
        label: String,
        action: String,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        kind: Kind,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default, skip_serializing_if = "is_default")]
        size: ButtonSize,
        #[serde(default, skip_serializing_if = "is_default")]
        wide: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A labelled choice: its `choices` side by side, the one whose
    /// value is `picked` outlined in blue over the offset, the others
    /// quiet flat blocks. A click carries out `action`, the plugin's
    /// own, with `args` and the choice's `value`.
    Segments {
        #[serde(default)]
        choices: Vec<Segment>,
        #[serde(default)]
        picked: Value,
        action: String,
        #[serde(default)]
        args: Value,
    },
    /// Text that opens a page.
    Link {
        label: String,
        to: Link,
    },
    /// Pieces bound to a key of the plugin's settings, drawn on a
    /// settings page only; kept as they came, for the page that binds
    /// them.
    Toggle {
        #[serde(flatten)]
        rest: Map<String, Value>,
    },
    Choice {
        #[serde(flatten)]
        rest: Map<String, Value>,
    },
    Field {
        #[serde(flatten)]
        rest: Map<String, Value>,
    },
    /// A quantity: `total` square ticks, the first `lit` of them in
    /// `tone`, those at the indexes in `marked` (from 0) white, as
    /// thresholds on the strip. `labels` are small print under it, each
    /// centred under its tick.
    Ticks {
        #[serde(default)]
        lit: u32,
        total: u32,
        #[serde(default)]
        tone: Ink,
        #[serde(default)]
        marked: Vec<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        labels: Vec<TickLabel>,
    },
    /// One small square for each of a few things, each in its ink: the
    /// calls a script made, by how they went. No wider than its squares.
    Pips {
        #[serde(default)]
        tones: Vec<Ink>,
    },
    /// The small mark of a state; with a `count` above one, the count
    /// before it.
    Mark {
        state: MarkState,
        #[serde(default)]
        count: Option<u32>,
    },
    /// A word or two in mono, outlined in `tone`: `exit 0`, `queued`.
    Stamp {
        label: String,
        #[serde(default)]
        tone: Ink,
    },
    /// A small square holding a number or a letter: a step, a check, a
    /// place in a queue. With an `action`, the plugin's own, a click
    /// carries it out with `args`: a segment, a toggle.
    Square {
        label: String,
        #[serde(default)]
        style: SquareLook,
        #[serde(default)]
        tone: Ink,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        args: Value,
    },
    /// A call made inside another (ADR 0060): a lettered step, its tool
    /// in its kind's color, what it is about, and what came back.
    Step {
        label: String,
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<theme::ToolKind>,
        #[serde(default)]
        subject: String,
        #[serde(default)]
        receipt: String,
        #[serde(default)]
        state: StepState,
    },
    /// A note or an id on a label outlined in `tone`; `dashed` for one
    /// proposed, not kept yet. With `to`, a click opens that page.
    Slip {
        text: String,
        #[serde(default)]
        tone: Ink,
        #[serde(default)]
        dashed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<Link>,
    },
    /// A big number or time, in the display face.
    Figure {
        text: String,
    },
    /// A large glyph in the display face, in `tone`: the mark beside a
    /// note.
    Kanji {
        text: String,
        #[serde(default)]
        tone: Ink,
    },
    /// A piece that did not read, and why.
    #[serde(skip_deserializing)]
    Invalid {
        why: String,
    },
}

/// A column of [`Node::Board`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardColumn {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cards: Vec<BoardCard>,
}

/// A card of a [`BoardColumn`]: its id, and what it shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardCard {
    pub id: String,
    #[serde(default, deserialize_with = "nodes")]
    pub children: Vec<Node>,
}

/// One tab of [`Node::Tabs`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub label: String,
    #[serde(default, deserialize_with = "nodes")]
    pub children: Vec<Node>,
}

/// The UI state key under which a section's `id` keeps whether it is
/// open, and tabs' `id` the tab chosen.
pub fn open_key(id: &str) -> String {
    format!("{id}.open")
}

pub fn tab_key(id: &str) -> String {
    format!("{id}.tab")
}

fn default_graph_height() -> f32 {
    12.5
}

/// The narrowest a board's column is, in pixels: a board too narrow for
/// all of them shows them as tabs.
const BOARD_COLUMN: f32 = 200.;

/// A board's card while it is dragged, drawn under the pointer: which
/// board and column it came from, and what it is called.
#[derive(Clone)]
struct DraggedCard {
    board: String,
    card: String,
    from: String,
    title: String,
    /// Its background, border and text.
    colors: (gpui::Hsla, gpui::Hsla, gpui::Hsla),
}

impl gpui::Render for DraggedCard {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let (bg, border, text) = self.colors;
        div()
            .w(gpui::px(BOARD_COLUMN))
            .px(sp(3.))
            .py(sp(2.5))
            .bg(bg)
            .border_2()
            .border_color(border)
            .text_color(text)
            .typeset(Type::SMALL)
            .truncate()
            .child(self.title.clone())
    }
}

/// The first text a card shows, to name it while it is dragged.
fn card_title(children: &[Node]) -> String {
    children
        .iter()
        .find_map(|node| match node {
            Node::Text { text }
            | Node::Rich { text }
            | Node::Mono { text }
            | Node::Heading { text, .. }
            | Node::Title { text, .. } => Some(text.clone()),
            Node::Row { children } | Node::Stack { children } => {
                Some(card_title(children)).filter(|title| !title.is_empty())
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// A node of [`Node::Graph`]: where it is, from 0 to 1 across and down,
/// how it reads, and the page it opens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub tone: Tone,
    /// The one the map is about: drawn larger, in the accent.
    #[serde(default)]
    pub focus: bool,
    #[serde(default)]
    pub to: Option<Link>,
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

/// What a terminal is drawn from: its key, what was written to it, its
/// size, and whether more is coming.
pub struct TerminalSpec {
    pub key: String,
    pub bytes: Vec<u8>,
    pub cols: u16,
    pub rows: u16,
    pub live: bool,
    /// How many rows it shows; all of them when `None`.
    pub shown: Option<u16>,
}

/// What a ticker is drawn from: its key, and the time it counts from and,
/// once it stops, to, in milliseconds since 1970.
pub struct TickerSpec {
    pub key: String,
    pub count: heiko_ui_kit::ticker::Count,
}

/// A piece the interface draws with a view it keeps from one draw to
/// the next.
pub enum WidgetSpec {
    Terminal(TerminalSpec),
    Ticker(TickerSpec),
    Image(ImageSpec),
    Keys(KeysSpec),
}

/// Carries out a key an area took: its name, and whether it came from a
/// field inside the area.
pub type OnKey = Rc<dyn Fn(&str, bool, &mut App)>;

/// What an area that takes the keys is drawn from: its key, the keys it
/// takes and those its fields let through, its focus number, what is in
/// it, and what a key does.
pub struct KeysSpec {
    pub key: String,
    pub keys: Vec<String>,
    pub through: Vec<String>,
    pub focus: Option<u64>,
    pub children: Vec<AnyElement>,
    pub on_key: Option<OnKey>,
}

/// What an image is drawn from: its key, its type, the plugin's action
/// that fetches it, and the height it fits.
pub struct ImageSpec {
    pub key: String,
    pub mime: String,
    pub fetch: Value,
    pub height: Option<f32>,
}

/// Draws a widget, kept by the interface from one draw to the next.
pub type Widgets<'a> = &'a dyn Fn(WidgetSpec, &mut App) -> AnyElement;

/// How many rows a folded terminal shows.
pub const FOLDED_ROWS: u16 = 12;

/// The UI state key under which an input's `id` keeps its draft.
pub fn draft_key(id: &str) -> String {
    format!("{id}.draft")
}

/// What an input is drawn from: the key of its draft, the draft, how it
/// looks, and what its Enter does.
pub struct InputSpec {
    pub key: String,
    pub draft: String,
    pub placeholder: String,
    pub multiline: bool,
    pub keep: bool,
    pub secret: bool,
    /// The focus number: a new one gives the input the keys.
    pub focus: Option<u64>,
    pub on_submit: Option<OnSubmit>,
}

/// Carries out an input's Enter with what it holds.
pub type OnSubmit = Rc<dyn Fn(String, &mut App)>;

/// Gives the editor of an input, kept by the interface: made from its
/// draft, and sending each edit back as the draft (ADR 0045).
pub type Inputs<'a> = &'a dyn Fn(
    InputSpec,
    &mut App,
) -> gpui::Entity<heiko_ui_kit::input::TextInput>;

/// Children that read one by one, each failing alone.
fn nodes<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<Node>, D::Error> {
    let values = Option::<Vec<Value>>::deserialize(de)?.unwrap_or_default();
    Ok(values.iter().map(Node::read).collect())
}

fn table_rows<'de, D: Deserializer<'de>>(
    de: D,
) -> Result<Vec<Vec<Node>>, D::Error> {
    let rows = Option::<Vec<Vec<Value>>>::deserialize(de)?.unwrap_or_default();
    Ok(rows
        .iter()
        .map(|row| row.iter().map(Node::read).collect())
        .collect())
}

impl Node {
    /// Reads a tree, never failing: what does not read becomes
    /// [`Node::Invalid`] where it stands.
    pub fn read(value: &Value) -> Self {
        if let Some(text) = value.as_str() {
            return Self::text(text);
        }
        // One that did not read before, sent on as it was written: it
        // keeps why.
        if value.get("piece").and_then(Value::as_str) == Some("invalid") {
            return Self::Invalid {
                why: value["why"].as_str().unwrap_or_default().to_owned(),
            };
        }
        match Self::deserialize(value) {
            Ok(node) => node,
            Err(error) => Self::Invalid {
                why: match value.get("piece").and_then(Value::as_str) {
                    Some(piece) => format!("piece `{piece}`: {error}"),
                    None => "a piece without `piece`".to_owned(),
                },
            },
        }
    }

    /// The tree as JSON, the shape [`Node::read`] takes.
    pub fn value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn stack(children: impl IntoIterator<Item = Node>) -> Self {
        Self::Stack {
            children: children.into_iter().collect(),
        }
    }

    pub fn row(children: impl IntoIterator<Item = Node>) -> Self {
        Self::Row {
            children: children.into_iter().collect(),
        }
    }

    /// Whether it is one of the pieces a settings page binds.
    pub fn is_setting(&self) -> bool {
        matches!(
            self,
            Self::Toggle { .. } | Self::Choice { .. } | Self::Field { .. }
        )
    }
}

/// What a click in a tree asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// The plugin's own action, with its arguments.
    Plugin { action: String, args: Value },
    /// Opens a page.
    Navigate(Link),
    /// Sets a key of the plugin's UI state, or forgets it: a section
    /// opened, a tab chosen (ADR 0045).
    Ui { key: String, value: Option<Value> },
}

/// Carries out what a click asks.
pub type OnAct = Rc<dyn Fn(Act, &mut App)>;

/// What the interface has of a fetched tree: given its key and the
/// action that asks for it, the answer once there is one, and why it
/// failed when it did. The interface asks for it the first time.
pub type Fetches<'a> =
    &'a dyn Fn(&str, &Value, &mut App) -> Option<Result<Value, String>>;

/// What a tree is drawn with.
pub struct Draw<'a> {
    pub theme: &'a Theme,
    /// Names the tree's elements apart from other trees on screen.
    pub id: SharedString,
    /// What clicks do; without it buttons and links draw but do nothing.
    pub on_act: Option<OnAct>,
    /// The plugin's UI state, which sections and tabs read; without it
    /// each shows as it starts.
    pub ui: Option<&'a std::collections::BTreeMap<String, Value>>,
    /// What the tree's UI state keys start with: the plugin's own part,
    /// for a plugin that draws trees for several (heiko-luau-plugins).
    pub prefix: Option<String>,
    /// Where inputs' editors come from; without it inputs show their
    /// draft and take no keys.
    pub inputs: Option<Inputs<'a>>,
    /// Where widgets' views come from (terminals, tickers); without it a
    /// widget shows that it is drawn where one is kept.
    pub widgets: Option<Widgets<'a>>,
    /// Where fetched trees come from; without one a fetched tree shows
    /// that it is drawn where they are kept.
    pub fetches: Option<Fetches<'a>>,
    /// The width the tree is laid out in, in pixels.
    pub width: f32,
    /// The window's size, in pixels, where a modal lays its backdrop.
    pub window: (f32, f32),
}

impl<'a> Draw<'a> {
    pub fn new(theme: &'a Theme, id: impl Into<SharedString>) -> Self {
        Self {
            theme,
            id: id.into(),
            on_act: None,
            ui: None,
            prefix: None,
            inputs: None,
            widgets: None,
            fetches: None,
            width: 960.,
            window: (960., 720.),
        }
    }

    pub fn ui(
        mut self,
        ui: &'a std::collections::BTreeMap<String, Value>,
    ) -> Self {
        self.ui = Some(ui);
        self
    }

    pub fn on_act(mut self, on_act: OnAct) -> Self {
        self.on_act = Some(on_act);
        self
    }

    pub fn fetches(mut self, fetches: Fetches<'a>) -> Self {
        self.fetches = Some(fetches);
        self
    }

    pub fn window(mut self, window: (f32, f32)) -> Self {
        self.window = window;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn widgets(mut self, widgets: Widgets<'a>) -> Self {
        self.widgets = Some(widgets);
        self
    }

    pub fn inputs(mut self, inputs: Inputs<'a>) -> Self {
        self.inputs = Some(inputs);
        self
    }

    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// `key` of the tree's UI state, under its prefix.
    fn key(&self, key: String) -> String {
        match &self.prefix {
            Some(prefix) => format!("{prefix}/{key}"),
            None => key,
        }
    }

    /// Draws `node`.
    pub fn draw(&self, node: &Node, cx: &mut App) -> AnyElement {
        self.at(node, 0, &self.id, cx)
    }

    fn children(
        &self,
        nodes: &[Node],
        depth: usize,
        path: &str,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let mut drawn = Vec::new();
        for (n, child) in nodes.iter().take(MAX_CHILDREN).enumerate() {
            drawn.push(self.at(child, depth + 1, &format!("{path}.{n}"), cx));
        }
        drawn
    }

    /// Pieces one under the other, as a stack draws them: a header that
    /// is not the first has room over it, so sections stand apart.
    fn stacked(
        &self,
        nodes: &[Node],
        depth: usize,
        path: &str,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        self.children(nodes, depth, path, cx)
            .into_iter()
            .zip(nodes)
            .enumerate()
            .map(|(n, (drawn, node))| match node {
                Node::Header { .. } if n > 0 => {
                    div().pt(sp(4.)).child(drawn).into_any_element()
                }
                _ => drawn,
            })
            .collect()
    }

    /// A board: its columns side by side, each card draggable to another
    /// column, which carries out `action`; too narrow for them, its
    /// columns as tabs.
    fn board(
        &self,
        action: &str,
        columns: &[BoardColumn],
        depth: usize,
        path: &str,
        cx: &mut App,
    ) -> AnyElement {
        let t = self.theme;
        let columns = &columns[..columns.len().min(MAX_CHILDREN)];
        if self.width < BOARD_COLUMN * columns.len().max(1) as f32 {
            let tabs = Node::Tabs {
                id: format!("{path}:board"),
                tabs: columns
                    .iter()
                    .map(|column| Tab {
                        label: format!(
                            "{} {}",
                            column.title,
                            column.cards.len()
                        ),
                        children: column
                            .cards
                            .iter()
                            .map(|card| Node::Card {
                                children: card.children.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            };
            return self.piece(&tabs, depth, path, cx);
        }
        let drawn: Vec<AnyElement> = columns
            .iter()
            .enumerate()
            .map(|(c, column)| {
                let cards: Vec<AnyElement> = column
                    .cards
                    .iter()
                    .take(MAX_CHILDREN)
                    .enumerate()
                    .map(|(n, card)| {
                        let at = format!("{path}.{c}.{n}");
                        let dragged = DraggedCard {
                            board: path.to_owned(),
                            card: card.id.clone(),
                            from: column.id.clone(),
                            title: card_title(&card.children),
                            colors: (t.panel, t.border_strong, t.text),
                        };
                        div()
                            .id(SharedString::from(format!("{at}:card")))
                            .debug_selector(selector("board-card", &card.id))
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(sp(1.5))
                            .py(sp(2.5))
                            .border_b_1()
                            .border_color(t.hairline)
                            .overflow_hidden()
                            .children(self.children(
                                &card.children,
                                depth + 1,
                                &at,
                                cx,
                            ))
                            .when(self.on_act.is_some(), |card| {
                                card.cursor_grab().on_drag(
                                    dragged,
                                    |dragged, _, _, cx| {
                                        cx.new(|_| dragged.clone())
                                    },
                                )
                            })
                            .into_any_element()
                    })
                    .collect();
                let dropped = self.on_act.clone().map(|on_act| {
                    let (board, action, to) =
                        (path.to_owned(), action.to_owned(), column.id.clone());
                    move |dragged: &DraggedCard,
                          _: &mut gpui::Window,
                          cx: &mut App| {
                        if dragged.board != board || dragged.from == to {
                            return;
                        }
                        on_act(
                            Act::Plugin {
                                action: action.clone(),
                                args: serde_json::json!({
                                    "card": dragged.card,
                                    "from": dragged.from,
                                    "to": to,
                                }),
                            },
                            cx,
                        );
                    }
                });
                let accent = t.accent;
                div()
                    .id(SharedString::from(format!("{path}.{c}:column")))
                    .debug_selector(selector("board-column", &column.id))
                    .flex_1()
                    .min_w(gpui::px(BOARD_COLUMN))
                    .flex()
                    .flex_col()
                    .px(sp(2.))
                    // Seen only while a card is dragged over it.
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(sp(2.))
                            .py(sp(2.))
                            .border_b_2()
                            .border_color(t.rule)
                            .text_color(t.bright)
                            .child(
                                div()
                                    .typeset(Type::SMALL)
                                    .font_weight(
                                        heiko_ui_kit::theme::weight::STRONG,
                                    )
                                    .child(column.title.clone()),
                            )
                            .child(
                                div()
                                    .typeset(Type::MICRO)
                                    .text_color(t.dim)
                                    .child(column.cards.len().to_string()),
                            ),
                    )
                    .children(cards)
                    .drag_over::<DraggedCard>(move |style, _, _, _| {
                        style.border_color(accent)
                    })
                    .when_some(dropped, |column, dropped| {
                        column.on_drop(dropped)
                    })
                    .into_any_element()
            })
            .collect();
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(sp(4.))
            .children(drawn)
            .into_any_element()
    }

    /// A graph: its edges painted under its nodes, each node a square with
    /// its label, opening its page when it has one.
    fn graph(
        &self,
        height: f32,
        nodes: &[GraphNode],
        edges: &[(String, String)],
        path: &str,
    ) -> AnyElement {
        let t = self.theme;
        let spot = |id: &str| {
            nodes
                .iter()
                .find(|node| node.id == id)
                .map(|node| (node.x.clamp(0., 1.), node.y.clamp(0., 1.)))
        };
        let lines: Vec<((f32, f32), (f32, f32))> = edges
            .iter()
            .take(MAX_CHILDREN)
            .filter_map(|(from, to)| Some((spot(from)?, spot(to)?)))
            .collect();
        let edge = t.border_strong;
        let painted = gpui::canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let at = |(x, y): (f32, f32)| {
                    gpui::point(
                        bounds.left() + bounds.size.width * x,
                        bounds.top() + bounds.size.height * y,
                    )
                };
                for (from, to) in &lines {
                    let mut line = gpui::PathBuilder::stroke(gpui::px(1.5));
                    line.move_to(at(*from));
                    line.line_to(at(*to));
                    if let Ok(line) = line.build() {
                        window.paint_path(line, edge);
                    }
                }
            },
        )
        .absolute()
        .size_full();
        let drawn =
            nodes
                .iter()
                .take(MAX_CHILDREN)
                .enumerate()
                .map(|(n, node)| {
                    let (size, color) = if node.focus {
                        (18., t.accent)
                    } else {
                        let color = match node.tone {
                            Tone::Neutral => t.slate,
                            tone => t.tone(tone.into()),
                        };
                        (12., color)
                    };
                    div()
                        .id(SharedString::from(format!("{path}:node{n}")))
                        .absolute()
                        .left(relative(node.x.clamp(0., 1.)))
                        .top(relative(node.y.clamp(0., 1.)))
                        .w(rems(7.5))
                        .ml(sp(-15.))
                        .mt(rems((-size / 2.) / 16.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            div()
                                .size(rems(size / 16.))
                                .bg(color)
                                .border_2()
                                .border_color(t.panel),
                        )
                        .children(node.label.clone().map(|label| {
                            div()
                                .mt(sp(1.))
                                .max_w(rems(7.5))
                                .truncate()
                                .typeset(Type::MICRO)
                                .text_color(t.muted)
                                .child(label)
                        }))
                        .when_some(
                            node.to
                                .clone()
                                .and_then(|to| self.click(Act::Navigate(to))),
                            |node, click| node.cursor_pointer().on_click(click),
                        )
                });
        div()
            .relative()
            .h(rems(height.clamp(4., 40.)))
            .w_full()
            .child(painted)
            .children(drawn)
            .into_any_element()
    }

    /// A clickable element's handler, when clicks do something.
    fn click(
        &self,
        act: Act,
    ) -> Option<impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut App) + 'static>
    {
        let on_act = self.on_act.clone()?;
        Some(
            move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| {
                on_act(act.clone(), cx)
            },
        )
    }

    fn at(
        &self,
        node: &Node,
        depth: usize,
        path: &str,
        cx: &mut App,
    ) -> AnyElement {
        let drawn = self.piece(node, depth, path, cx);
        // Pieces that size to their content keep it in a column or a
        // table's cell, which would stretch them across.
        match node {
            Node::Icon { .. }
            | Node::Badge { .. }
            | Node::Pill { .. }
            | Node::Button { wide: false, .. }
            | Node::Link { .. }
            | Node::Mark { .. }
            | Node::Stamp { .. }
            | Node::Square { .. } => {
                div().flex().child(drawn).into_any_element()
            }
            _ => drawn,
        }
    }

    fn piece(
        &self,
        node: &Node,
        depth: usize,
        path: &str,
        cx: &mut App,
    ) -> AnyElement {
        let t = self.theme;
        if depth > MAX_DEPTH {
            return invalid("the view nests too deep", t);
        }
        match node {
            Node::Text { text } => div()
                .typeset(Type::SMALL)
                .text_color(t.text_soft)
                .child(text.clone())
                .into_any_element(),
            Node::Rich { text } => div()
                .typeset(Type::SMALL)
                .child(rich(text, t.text_soft, t))
                .into_any_element(),
            Node::Mono { text } => {
                mono(text.clone(), Type::CAPTION, t.text_soft)
                    .into_any_element()
            }
            Node::Heading { text, tone } => heading(text, t)
                .text_color(tone.color(t))
                .into_any_element(),
            Node::Header {
                title,
                meta,
                actions,
            } => {
                let actions = self.children(actions, depth, path, cx);
                section_header(
                    title.clone(),
                    meta.clone().map(SharedString::from),
                    t,
                )
                .when(!actions.is_empty(), |head| {
                    head.child(
                        div()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .gap(sp(3.))
                            .children(actions),
                    )
                })
                .into_any_element()
            }
            Node::Markdown { text } => div()
                .typeset(Type::SMALL)
                .child(markdown(path, text, t.text_soft, t))
                .into_any_element(),
            Node::Icon { name, tone } => match Icon::named(name) {
                Some(glyph) => {
                    let color = match tone {
                        Tone::Neutral => t.muted,
                        tone => t.tone((*tone).into()),
                    };
                    icon(glyph, IconSize::COMPACT, color).into_any_element()
                }
                None => invalid(&format!("no icon `{name}`"), t),
            },
            Node::Badge { text, tone } => {
                stamp(text.clone(), (*tone).into(), t).into_any_element()
            }
            Node::Pill { text, tone } => {
                let color = match tone {
                    Tone::Neutral => t.text_soft,
                    tone => t.tone((*tone).into()),
                };
                mono(text.clone(), Type::CAPTION, color)
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .into_any_element()
            }
            Node::Status { text, detail, tone } => div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(match tone {
                    Tone::Good => status_state(State::Met, t),
                    Tone::Warn => status_state(State::Asks, t),
                    Tone::Bad => status_state(State::Failing, t),
                    Tone::Neutral => status_state(State::Idle, t),
                    // Nothing to act on: a flat square in the tone.
                    Tone::Info => div()
                        .size(sp(2.))
                        .flex_shrink_0()
                        .bg(t.tone((*tone).into()))
                        .into_any_element(),
                })
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.text)
                        .child(text.clone()),
                )
                .when_some(detail.clone(), |row, detail| {
                    row.child(mono(detail, Type::CAPTION, t.dim))
                })
                .into_any_element(),
            Node::Rows { rows } => div()
                .flex()
                .flex_col()
                .gap(sp(1.))
                .children(rows.iter().take(MAX_CHILDREN).map(|row| {
                    div()
                        .flex()
                        .gap(sp(3.))
                        .child(
                            mono(row.key.clone(), Type::CAPTION, t.dim)
                                .w(rems(8.))
                                .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .typeset(Type::SMALL)
                                .text_color(t.text_soft)
                                .child(row.value.clone()),
                        )
                }))
                .into_any_element(),
            Node::Title {
                text,
                subtitle,
                tone,
            } => match subtitle {
                Some(subtitle) => heiko_ui_kit::components::screen_title(
                    text.clone(),
                    subtitle.clone(),
                    t,
                )
                .into_any_element(),
                None => {
                    heiko_ui_kit::components::title(text.clone(), Type::TITLE)
                        .text_color(if *tone == Ink::default() {
                            t.text
                        } else {
                            tone.color(t)
                        })
                        .into_any_element()
                }
            },
            Node::Table { head, rows, widths } => {
                heiko_ui_kit::components::weighted_table(
                    &[],
                    widths,
                    head.iter()
                        .map(|cell| cell.clone().into_any_element())
                        .collect(),
                    rows.iter()
                        .take(MAX_CHILDREN)
                        .enumerate()
                        .map(|(n, row)| {
                            self.children(
                                row,
                                depth,
                                &format!("{path}.{n}"),
                                cx,
                            )
                        })
                        .collect(),
                    t,
                )
                .into_any_element()
            }
            Node::List { items } => div()
                .flex()
                .flex_col()
                .gap(sp(1.))
                .children(
                    self.children(items, depth, path, cx).into_iter().map(
                        |item| {
                            div()
                                .flex()
                                .gap(sp(2.))
                                .child(div().text_color(t.dim).child("•"))
                                .child(
                                    div().flex_1().min_w(rems(0.)).child(item),
                                )
                        },
                    ),
                )
                .into_any_element(),
            Node::Progress { share, label } => {
                let share = share.clamp(0., 1.);
                let lit = (share * PROGRESS_TICKS as f64).round() as usize;
                // Any share above none lights a tick.
                let lit = if share > 0. { lit.max(1) } else { 0 };
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(1.))
                    .when_some(label.clone(), |column, label| {
                        column.child(mono(label, Type::CAPTION, t.dim))
                    })
                    .child(heiko_ui_kit::components::ticks(
                        lit,
                        PROGRESS_TICKS,
                        t.accent,
                        t,
                    ))
                    .into_any_element()
            }
            Node::Code {
                lang,
                text,
                first_line,
            } => heiko_ui_kit::components::numbered_code_block(
                lang.as_deref(),
                text,
                *first_line,
                t,
            )
            .into_any_element(),
            Node::Toned { text, tone, mono } => {
                let color = match tone {
                    Tone::Neutral => t.dim,
                    tone => t.tone((*tone).into()),
                };
                let line = div().typeset(if *mono {
                    Type::CAPTION.mono()
                } else {
                    Type::SMALL
                });
                line.text_color(color)
                    .child(text.clone())
                    .into_any_element()
            }
            Node::Stack { children } => div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .children(self.stacked(children, depth, path, cx))
                .into_any_element(),
            Node::Row { children } => {
                let mut drawn = self.children(children, depth, path, cx);
                let row = div().flex().flex_wrap().items_center().gap(sp(2.));
                if !matches!(children.first(), Some(Node::Kanji { .. }))
                    || drawn.len() < 2
                {
                    return row.children(drawn).into_any_element();
                }
                // The glyph keeps its width; the rest takes what is
                // left, and wraps there, never under the glyph.
                let kanji = drawn.remove(0);
                let rest = div().flex_1().min_w(rems(0.));
                let rest = if drawn.len() == 1 {
                    rest.children(drawn)
                } else {
                    rest.child(row.children(drawn))
                };
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .child(kanji)
                    .child(rest)
                    .into_any_element()
            }
            Node::Line { children } => {
                let mut drawn = self.children(children, depth, path, cx);
                let last = drawn.pop();
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .children(
                        drawn
                            .into_iter()
                            .map(|piece| div().flex_shrink_0().child(piece)),
                    )
                    .children(last.map(|piece| {
                        div().flex_1().min_w(rems(0.)).truncate().child(piece)
                    }))
                    .into_any_element()
            }
            Node::Scroll { height, children } => div()
                .id(SharedString::from(format!("{path}:scroll")))
                .flex()
                .flex_col()
                .gap(sp(2.))
                .max_h(rems(height.unwrap_or(24.).clamp(2., 80.)))
                .overflow_y_scroll()
                .children(self.stacked(children, depth, path, cx))
                .into_any_element(),
            Node::Rule => rule(t).into_any_element(),
            Node::Gap { size } => div()
                .flex_shrink_0()
                .h(sp(match size {
                    GapSize::Small => 2.,
                    GapSize::Medium => 4.,
                    GapSize::Large => 8.,
                }))
                .into_any_element(),
            Node::Card { children } => heiko_ui_kit::components::card(t)
                .gap(sp(2.))
                .py(sp(3.))
                .children(self.children(children, depth, path, cx))
                .into_any_element(),
            Node::Band { children } => heiko_ui_kit::components::band(t)
                .children(self.children(children, depth, path, cx))
                .into_any_element(),
            Node::Rail { children } => heiko_ui_kit::components::rail(
                self.children(children, depth, path, cx),
                t,
            )
            .into_any_element(),
            Node::Switch {
                label,
                detail,
                on,
                action,
                args,
            } => div()
                .id(SharedString::from(format!("{path}:switch")))
                .debug_selector(selector("switch", label))
                .flex()
                .items_center()
                .gap(sp(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.75))
                        .child(label.clone())
                        .when_some(detail.clone(), |column, detail| {
                            column.child(heiko_ui_kit::components::text(
                                detail,
                                Type::CAPTION,
                                t.muted,
                            ))
                        }),
                )
                .child(heiko_ui_kit::components::switch(*on, t))
                .when_some(
                    self.click(Act::Plugin {
                        action: action.clone(),
                        args: args.clone(),
                    }),
                    |row, click| row.cursor_pointer().on_click(click),
                )
                .into_any_element(),
            Node::Section {
                id,
                title,
                open,
                small,
                lead,
                children,
            } => {
                let key = self.key(open_key(id));
                let open = self
                    .ui
                    .and_then(|ui| ui.get(&key))
                    .and_then(Value::as_bool)
                    .unwrap_or(*open);
                let turn: &'static str = if open { "hide" } else { "show" };
                let head = if *small {
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(3.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(open, |head| {
                            head.pb(sp(1.5))
                                .border_b_1()
                                .border_color(t.hairline)
                        })
                        .children(self.children(
                            lead,
                            depth,
                            &format!("{path}:lead"),
                            cx,
                        ))
                        .child(
                            heiko_ui_kit::components::mono(
                                title.clone(),
                                heiko_ui_kit::theme::Type::SMALL,
                                t.dim,
                            )
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate(),
                        )
                        .child(heiko_ui_kit::components::link_action(turn, t))
                } else {
                    section_header(title.clone(), Some(turn.into()), t)
                };
                let head = head
                    .id(SharedString::from(format!("{path}:section")))
                    .debug_selector(selector("section", title))
                    .cursor_pointer()
                    .when_some(
                        self.click(Act::Ui {
                            key,
                            value: Some(Value::Bool(!open)),
                        }),
                        |head, click| head.on_click(click),
                    );
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(3.))
                    .child(head)
                    .when(open, |column| {
                        column.children(self.stacked(children, depth, path, cx))
                    })
                    .into_any_element()
            }
            Node::Tabs { id, tabs } => {
                let key = self.key(tab_key(id));
                let chosen = self
                    .ui
                    .and_then(|ui| ui.get(&key))
                    .and_then(Value::as_u64)
                    .map_or(0, |n| n as usize)
                    .min(tabs.len().saturating_sub(1));
                let bar = div().flex().flex_wrap().gap(sp(5.)).children(
                    tabs.iter().take(MAX_CHILDREN).enumerate().map(
                        |(n, tab)| {
                            let on = n == chosen;
                            div()
                                .id(SharedString::from(format!(
                                    "{path}:tab{n}"
                                )))
                                .debug_selector(selector("tab", &tab.label))
                                .pb(sp(1.))
                                .border_b_2()
                                .border_color(if on {
                                    t.rule
                                } else {
                                    gpui::transparent_black()
                                })
                                .cursor_pointer()
                                .typeset(Type::SMALL)
                                .text_color(if on { t.bright } else { t.dim })
                                .child(tab.label.clone())
                                .when_some(
                                    self.click(Act::Ui {
                                        key: key.clone(),
                                        value: Some(Value::from(n)),
                                    }),
                                    |tab, click| tab.on_click(click),
                                )
                        },
                    ),
                );
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(3.))
                    .child(bar)
                    .children(tabs.get(chosen).map(|tab| {
                        div().flex().flex_col().gap(sp(2.)).children(
                            self.stacked(
                                &tab.children,
                                depth,
                                &format!("{path}:{chosen}"),
                                cx,
                            ),
                        )
                    }))
                    .into_any_element()
            }
            Node::Input {
                id,
                placeholder,
                multiline,
                submit,
                keep,
                secret,
                focus,
            } => {
                let key = self.key(draft_key(id));
                let draft = if *secret {
                    String::new()
                } else {
                    self.ui
                        .and_then(|ui| ui.get(&key))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                let Some(inputs) = self.inputs else {
                    return field_text(&draft, t);
                };
                let on_submit = match (submit, &self.on_act) {
                    (Some(action), Some(on_act)) => {
                        let (action, on_act) = (action.clone(), on_act.clone());
                        Some(Rc::new(move |text: String, cx: &mut App| {
                            on_act(
                                Act::Plugin {
                                    action: action.clone(),
                                    args: serde_json::json!({ "text": text }),
                                },
                                cx,
                            )
                        }) as OnSubmit)
                    }
                    _ => None,
                };
                let input = inputs(
                    InputSpec {
                        key,
                        draft,
                        placeholder: placeholder.clone().unwrap_or_default(),
                        multiline: *multiline,
                        keep: *keep,
                        secret: *secret,
                        focus: *focus,
                        on_submit,
                    },
                    cx,
                );
                // It takes the room a row gives it, and never less than
                // a few words.
                div()
                    .flex_1()
                    .min_w(rems(12.))
                    .child(field(&input, false, t))
                    .into_any_element()
            }
            Node::Image {
                key,
                mime,
                fetch,
                height,
            } => match self.widgets {
                Some(widgets) => widgets(
                    WidgetSpec::Image(ImageSpec {
                        key: key.clone(),
                        mime: mime.clone(),
                        fetch: fetch.clone(),
                        height: *height,
                    }),
                    cx,
                ),
                None => {
                    invalid("an image, drawn where the interface keeps one", t)
                }
            },
            Node::Qr { text, size, offset } => {
                let code = if *offset {
                    printed_qr_code(text, *size, t)
                } else {
                    qr_code(text, *size)
                };
                code.map_or_else(
                    || invalid("text too long for a QR code", t),
                    IntoElement::into_any_element,
                )
            }
            Node::Fetched { key, fetch } => match self.fetches {
                Some(fetches) => match fetches(key, fetch, cx) {
                    Some(Ok(answer)) => {
                        let tree = Node::read(&answer["tree"]);
                        self.at(&tree, depth + 1, &format!("{path}.0"), cx)
                    }
                    Some(Err(why)) => invalid(&why, t),
                    None => div()
                        .typeset(Type::SMALL)
                        .text_color(t.dim)
                        .child("Drawing…")
                        .into_any_element(),
                },
                None => invalid("a fetched tree, drawn where one is kept", t),
            },
            Node::Diff { text, path } => heiko_ui_kit::diff::numbered(
                &heiko_ui_kit::diff::parse(text),
                Some(&heiko_ui_kit::diff::line_numbers(text)),
                path.as_deref().and_then(heiko_ui_kit::syntax::Lang::of_path),
                t,
            )
            .into_any_element(),
            Node::Marked { text, spans } => {
                let marked: Vec<std::ops::Range<usize>> = spans
                    .iter()
                    .take(MAX_CHILDREN)
                    .map(|(from, to)| {
                        from.saturating_sub(1) as usize..*to as usize
                    })
                    .collect();
                heiko_ui_kit::components::marked_line(text, &marked, t)
                    .into_any_element()
            }
            Node::Terminal {
                id,
                text,
                cols,
                rows,
                live,
                fold,
            } => match self.widgets {
                Some(widgets) => {
                    // Folded, it shows a few rows; the person opens it.
                    let key = self.key(format!("{id}.expanded"));
                    let expanded = self
                        .ui
                        .and_then(|ui| ui.get(&key))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let screen = widgets(
                        WidgetSpec::Terminal(TerminalSpec {
                            key: self.key(format!("{id}.terminal")),
                            bytes: text.as_bytes().to_vec(),
                            cols: (*cols).clamp(10, 400),
                            rows: (*rows).clamp(2, 200),
                            live: *live,
                            shown: (*fold && !expanded).then_some(FOLDED_ROWS),
                        }),
                        cx,
                    );
                    if !*fold {
                        return screen;
                    }
                    let toggle = link(
                        if expanded { "show less" } else { "show all" },
                        t,
                    )
                    .id(SharedString::from(format!("{path}:expand")))
                    .debug_selector(selector(
                        "button",
                        if expanded { "show less" } else { "show all" },
                    ))
                    .when_some(
                        self.click(Act::Ui {
                            key,
                            value: Some(Value::Bool(!expanded)),
                        }),
                        |toggle, click| toggle.on_click(click),
                    );
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(1.))
                        .child(screen)
                        .child(div().flex().justify_end().child(toggle))
                        .into_any_element()
                }
                None => invalid(
                    "a terminal, drawn where the interface keeps one",
                    t,
                ),
            },
            Node::Ticker { id, since, until } => match self.widgets {
                Some(widgets) => widgets(
                    WidgetSpec::Ticker(TickerSpec {
                        key: self.key(format!("{id}.ticker")),
                        count: heiko_ui_kit::ticker::Count::Since {
                            since: *since,
                            until: *until,
                        },
                    }),
                    cx,
                ),
                // Without a clock, what it read when it stopped, if it did.
                None => mono(
                    until.map_or_else(String::new, |until| {
                        heiko_ui_kit::ticker::reading(*since, until)
                    }),
                    Type::CAPTION,
                    t.text_soft,
                )
                .into_any_element(),
            },
            Node::Due { id, at } => match self.widgets {
                Some(widgets) => widgets(
                    WidgetSpec::Ticker(TickerSpec {
                        key: self.key(format!("{id}.due")),
                        count: heiko_ui_kit::ticker::Count::Due { at: *at },
                    }),
                    cx,
                ),
                None => invalid(
                    "a countdown, drawn where the interface keeps one",
                    t,
                ),
            },
            Node::Ago { id, at } => match self.widgets {
                Some(widgets) => widgets(
                    WidgetSpec::Ticker(TickerSpec {
                        key: self.key(format!("{id}.ago")),
                        count: heiko_ui_kit::ticker::Count::Ago { at: *at },
                    }),
                    cx,
                ),
                None => invalid(
                    "a time ago, drawn where the interface keeps one",
                    t,
                ),
            },
            Node::Spacer => div().flex_1().into_any_element(),
            Node::Graph {
                height,
                nodes,
                edges,
            } => self.graph(*height, nodes, edges, path),
            Node::Board { action, columns } => {
                self.board(action, columns, depth, path, cx)
            }
            Node::Modal {
                title,
                children,
                dismiss,
            } => {
                const WIDE: f32 = 440.;
                let dismissed = self.on_act.clone().zip(dismiss.clone()).map(
                    |(on_act, action)| {
                        move |_: &gpui::MouseDownEvent,
                              _: &mut gpui::Window,
                              cx: &mut App| {
                            on_act(
                                Act::Plugin {
                                    action: action.clone(),
                                    args: Value::Null,
                                },
                                cx,
                            )
                        }
                    },
                );
                let panel = div()
                    .id(SharedString::from(format!("{path}:modal")))
                    .mt(gpui::px(72.))
                    .w(gpui::px(WIDE))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .gap(sp(3.))
                    .p(sp(5.))
                    .bg(t.panel)
                    .border_2()
                    .border_color(t.border_strong)
                    // A click on it stays in it.
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation()
                    })
                    .child(
                        div()
                            .typeset(Type::SECTION.sized(17.))
                            .text_color(t.bright)
                            .child(title.clone()),
                    )
                    .children(self.children(children, depth, path, cx));
                // The backdrop: the window's width, and taller than any
                // window; what is under it takes no clicks.
                let backdrop = div()
                    .id(SharedString::from(format!("{path}:backdrop")))
                    .w(gpui::px(self.window.0))
                    .h(gpui::px(self.window.1))
                    .px(sp(2.))
                    .flex()
                    .justify_center()
                    .items_start()
                    .bg(t.scrim)
                    .occlude()
                    .when_some(dismissed, |backdrop, dismissed| {
                        backdrop
                            .on_mouse_down(gpui::MouseButton::Left, dismissed)
                    })
                    .child(panel);
                gpui::deferred(
                    gpui::anchored()
                        .position_mode(gpui::AnchoredPositionMode::Window)
                        .position(gpui::point(gpui::px(0.), gpui::px(0.)))
                        .child(backdrop),
                )
                .with_priority(2)
                .into_any_element()
            }
            Node::Keys {
                id,
                keys,
                through,
                action,
                focus,
                children,
            } => {
                let children = self.children(children, depth, path, cx);
                let Some(widgets) = self.widgets else {
                    return div()
                        .flex()
                        .flex_col()
                        .children(children)
                        .into_any_element();
                };
                let on_key = self.on_act.clone().map(|on_act| {
                    let action = action.clone();
                    Rc::new(move |key: &str, field: bool, cx: &mut App| {
                        on_act(
                            Act::Plugin {
                                action: action.clone(),
                                args: serde_json::json!({ "key": key, "field": field }),
                            },
                            cx,
                        )
                    }) as OnKey
                });
                widgets(
                    WidgetSpec::Keys(KeysSpec {
                        key: self.key(format!("{id}.keys")),
                        keys: keys.clone(),
                        through: through.clone(),
                        focus: *focus,
                        children,
                        on_key,
                    }),
                    cx,
                )
            }
            Node::Empty { text } => empty(text.clone(), t).into_any_element(),
            Node::Button {
                label,
                icon: Some(glyph),
                action,
                args,
                ..
            } if label.is_empty() => {
                let Some(glyph) = Icon::named(glyph) else {
                    return invalid(&format!("no icon `{glyph}`"), t);
                };
                div()
                    .id(SharedString::from(format!("{path}:button")))
                    .debug_selector(selector(
                        "button",
                        &format!("{glyph:?}").to_lowercase(),
                    ))
                    .size(rems(1.25))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|style| style.bg(t.raised))
                    .child(icon(glyph, IconSize::SMALL, t.dim))
                    .when_some(
                        self.click(Act::Plugin {
                            action: action.clone(),
                            args: args.clone(),
                        }),
                        |square, click| square.cursor_pointer().on_click(click),
                    )
                    .into_any_element()
            }
            Node::Button {
                label,
                action,
                args,
                kind,
                size,
                wide,
                detail,
                ..
            } => match self.click(Act::Plugin {
                action: action.clone(),
                args: args.clone(),
            }) {
                Some(click) => {
                    let drawn = match kind {
                        Kind::Primary if *wide => {
                            heiko_ui_kit::components::wide_block(
                                label.clone(),
                                detail.clone().map(SharedString::from),
                                (*size).into(),
                                t,
                            )
                        }
                        Kind::Primary => {
                            heiko_ui_kit::components::primary_block(
                                label.clone(),
                                (*size).into(),
                                t,
                            )
                        }
                        Kind::Hot => heiko_ui_kit::components::hot_action(
                            label.clone(),
                            t,
                        )
                        .typeset(Type::SMALL),
                        Kind::Secondary | Kind::Danger => {
                            button(label.clone(), (*kind).into(), t)
                        }
                    };
                    // A wide one that is a word takes the row's width,
                    // the word at its left.
                    let drawn = if *wide && *kind != Kind::Primary {
                        div()
                            .w_full()
                            .flex()
                            .justify_start()
                            .cursor_pointer()
                            .child(drawn)
                    } else {
                        drawn
                    };
                    drawn
                        .id(SharedString::from(format!("{path}:button")))
                        .debug_selector(selector("button", label))
                        .on_click(click)
                        .into_any_element()
                }
                None => div()
                    .typeset(Type::SMALL)
                    .text_color(t.dim)
                    .child(label.clone())
                    .into_any_element(),
            },
            Node::Segments {
                choices,
                picked,
                action,
                args,
            } => div()
                .flex()
                .flex_wrap()
                .gap(sp(2.))
                .children(choices.iter().take(MAX_CHILDREN).enumerate().map(
                    |(n, choice)| {
                        // What the click sends: the piece's `args`, and
                        // the choice's value.
                        let mut sent = match args {
                            Value::Object(args) => args.clone(),
                            _ => Map::new(),
                        };
                        sent.insert("value".to_owned(), choice.value.clone());
                        heiko_ui_kit::components::segment(
                            choice.label.clone(),
                            !picked.is_null() && *picked == choice.value,
                            t,
                        )
                        .id(SharedString::from(format!("{path}:segment{n}")))
                        .debug_selector(selector("segment", &choice.label))
                        .when_some(
                            self.click(Act::Plugin {
                                action: action.clone(),
                                args: Value::Object(sent),
                            }),
                            |segment, click| segment.on_click(click),
                        )
                    },
                ))
                .into_any_element(),
            Node::Link { label, to } => {
                match self.click(Act::Navigate(to.clone())) {
                    Some(click) => text_link(label.clone(), Type::SMALL, t)
                        .id(SharedString::from(format!("{path}:link")))
                        .debug_selector(selector("link", label))
                        .on_click(click)
                        .into_any_element(),
                    None => div()
                        .typeset(Type::SMALL)
                        .text_color(t.text)
                        .child(label.clone())
                        .into_any_element(),
                }
            }
            Node::Toggle { .. } | Node::Choice { .. } | Node::Field { .. } => {
                invalid("a setting, drawn on a settings page only", t)
            }
            Node::Ticks {
                lit,
                total,
                tone,
                marked,
                labels,
            } => {
                let total = (*total as usize).clamp(1, MAX_TICKS);
                let marked: Vec<usize> =
                    marked.iter().map(|at| *at as usize).collect();
                let labels: Vec<(usize, SharedString)> = labels
                    .iter()
                    .take(MAX_TICKS)
                    .map(|label| (label.at as usize, label.text.clone().into()))
                    .collect();
                // It takes the room a row gives it, and never less than
                // its ticks need.
                div()
                    .flex_1()
                    .min_w(rems(6.))
                    .child(heiko_ui_kit::components::labelled_ticks(
                        (*lit as usize).min(total),
                        total,
                        tone.color(t),
                        &marked,
                        &labels,
                        t,
                    ))
                    .into_any_element()
            }
            Node::Pips { tones } => div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(rems(0.1875))
                .children(tones.iter().take(MAX_TICKS).map(|tone| {
                    div().size(rems(0.5625)).bg(match tone {
                        Ink::Neutral => t.hairline,
                        tone => tone.color(t),
                    })
                }))
                .into_any_element(),
            Node::Mark { state, count } => match count {
                Some(count) => heiko_ui_kit::components::counted_mark(
                    *count as usize,
                    (*state).into(),
                    t,
                )
                .into_any_element(),
                None => heiko_ui_kit::components::mark((*state).into(), t)
                    .into_any_element(),
            },
            Node::Stamp { label, tone } => {
                heiko_ui_kit::components::stamp_in(
                    label.clone(),
                    tone.outline(t),
                    t,
                )
                .into_any_element()
            }
            Node::Square {
                label,
                style,
                tone,
                action,
                args,
            } => {
                let color = match tone {
                    Ink::Neutral => t.slate,
                    tone => tone.color(t),
                };
                let square = heiko_ui_kit::components::square(
                    label.clone(),
                    match style {
                        SquareLook::Quiet => SquareStyle::Quiet,
                        SquareLook::Outlined => SquareStyle::Outlined(color),
                        SquareLook::Filled => SquareStyle::Filled(color),
                    },
                    t,
                );
                let click = action.clone().and_then(|action| {
                    self.click(Act::Plugin {
                        action,
                        args: args.clone(),
                    })
                });
                match click {
                    Some(click) => square
                        .id(SharedString::from(format!("{path}:square")))
                        .debug_selector(selector("square", label))
                        .cursor_pointer()
                        .on_click(click)
                        .into_any_element(),
                    None => square.into_any_element(),
                }
            }
            Node::Step {
                label,
                tool,
                kind,
                subject,
                receipt,
                state,
            } => {
                use heiko_ui_kit::{
                    components::{mono, sub_step_square},
                    theme::weight,
                };
                let style = match state {
                    StepState::Done | StepState::Stopped => SquareStyle::Quiet,
                    StepState::Running => SquareStyle::Outlined(t.green),
                    StepState::Failed => SquareStyle::Filled(t.red),
                    StepState::Waits => SquareStyle::Outlined(t.roles.live),
                };
                let verb = match (state, kind) {
                    (StepState::Failed, _) => t.red,
                    (_, Some(kind)) => t.roles.kind(*kind),
                    (_, None) => t.text,
                };
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .min_h(rems(1.75))
                    .child(sub_step_square(label.clone(), style, t))
                    .child(
                        mono(tool.clone(), Type::SMALL, verb)
                            .flex_shrink_0()
                            .min_w(rems(6.))
                            .font_weight(weight::EMPHASIS),
                    )
                    .child(
                        mono(subject.clone(), Type::SMALL, t.text_soft)
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate(),
                    )
                    .when(!receipt.is_empty(), |row| {
                        row.child(
                            mono(receipt.clone(), Type::SMALL, t.dim)
                                .flex_shrink_0(),
                        )
                    })
                    .into_any_element()
            }
            Node::Slip {
                text,
                tone,
                dashed,
                to,
            } => {
                let slip = heiko_ui_kit::components::slip_in(
                    text.clone(),
                    tone.outline(t),
                    *dashed,
                    t,
                );
                let click =
                    to.clone().and_then(|to| self.click(Act::Navigate(to)));
                match click {
                    // In a row, so only the slip takes the click.
                    Some(click) => div()
                        .flex()
                        .child(
                            slip.id(SharedString::from(format!("{path}:slip")))
                                .debug_selector(selector("slip", text))
                                .cursor_pointer()
                                .on_click(click),
                        )
                        .into_any_element(),
                    None => slip.into_any_element(),
                }
            }
            Node::Figure { text } => {
                heiko_ui_kit::components::big_figure(text.clone(), t)
                    .into_any_element()
            }
            Node::Kanji { text, tone } => div()
                .flex_shrink_0()
                .typeset(Type::FIGURE.sized(46.))
                .text_color(match tone {
                    Ink::Neutral => t.bright,
                    tone => tone.color(t),
                })
                .child(text.clone())
                .into_any_element(),
            Node::Invalid { why } => invalid(why, t),
        }
    }
}

/// How many ticks a [`Node::Progress`] is drawn with, and the most a
/// [`Node::Ticks`] may have.
const PROGRESS_TICKS: usize = 20;
const MAX_TICKS: usize = 60;

/// A state's mark, as the piece before a status's line.
fn status_state(state: State, t: &Theme) -> AnyElement {
    heiko_ui_kit::components::mark(state, t).into_any_element()
}

/// An input drawn without an editor: its draft, as text.
fn field_text(draft: &str, t: &Theme) -> AnyElement {
    div()
        .px(sp(3.))
        .py(sp(2.))
        .border_1()
        .border_color(t.border)
        .typeset(Type::SMALL)
        .text_color(t.text_soft)
        .child(draft.to_owned())
        .into_any_element()
}

/// A clickable piece's name for tests (`debug_bounds`): its kind and
/// what it says, `button:Save`, `link:Memory`, `switch:Compact idle
/// chats`, `section:Details`, `tab:Rules`, `square:1`, `slip:a note`,
/// `segment:Weekly`.
fn selector(kind: &str, label: &str) -> impl Fn() -> String + 'static {
    let name = format!("{kind}:{label}");
    move || name.clone()
}

/// What a piece that does not draw shows instead.
fn invalid(why: &str, t: &Theme) -> AnyElement {
    div()
        .px(sp(2.))
        .py(sp(1.))
        .border_1()
        .border_color(t.red_border)
        .typeset(Type::CAPTION)
        .text_color(t.red)
        .child(why.to_owned())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Trees read in the shape `heiko.ui` builds them, and write back the
    /// same.
    #[test]
    fn reads_and_writes_the_wire_shape() {
        let wire = json!({ "piece": "stack", "children": [
            { "piece": "status", "text": "on", "detail": "2 turns", "tone": "good" },
            { "piece": "rows", "rows": [{ "key": "a", "value": "1" }] },
            { "piece": "progress", "share": 0.5, "label": "half" },
            { "piece": "code", "lang": "rust", "text": "fn f() {}", "first_line": 3 },
            { "piece": "button", "label": "Reset", "action": "reset", "args": { "n": 1 }, "kind": "danger", "icon": null },
            { "piece": "row", "children": [{ "piece": "badge", "text": "x", "tone": "warn" }] },
            { "piece": "list", "items": [{ "piece": "mono", "text": "m" }] },
            { "piece": "table", "head": ["a"], "rows": [[{ "piece": "text", "text": "1" }]], "widths": [2.0] },
            { "piece": "title", "text": "Ledger", "subtitle": "What was pruned" },
            { "piece": "scroll", "height": 10.0, "children": [{ "piece": "rule" }] },
            { "piece": "link", "label": "Notes", "to": { "plugin": null, "page": "notes", "params": {} } },
            { "piece": "icon", "name": "memory", "tone": "info" },
        ]});
        let node = Node::read(&wire);
        assert!(
            matches!(&node, Node::Stack { children } if children.len() == 12)
        );
        assert_eq!(Node::read(&node.value()), node);
        assert_eq!(node.value(), wire);
    }

    /// What Luau's builders leave out reads as missing: a bare string is
    /// text, `null` is no value, a button without a kind is secondary.
    #[test]
    fn strings_nulls_and_defaults() {
        assert_eq!(Node::read(&json!("hi")), Node::text("hi"));
        assert_eq!(
            Node::read(&json!({ "piece": "code", "lang": null, "text": "x" })),
            Node::Code {
                lang: None,
                text: "x".into(),
                first_line: None,
            }
        );
        assert_eq!(
            Node::read(
                &json!({ "piece": "button", "label": "Go", "action": "go", "args": {} })
            ),
            Node::Button {
                label: "Go".into(),
                action: "go".into(),
                args: json!({}),
                kind: Kind::Secondary,
                icon: None,
                size: ButtonSize::Small,
                wide: false,
                detail: None,
            }
        );
    }

    /// A piece that does not read is invalid where it stands; its
    /// siblings, and the cells of a table, still read.
    #[test]
    fn bad_pieces_stay_local() {
        let node = Node::read(&json!({ "piece": "row", "children": [
            { "piece": "sparkle" },
            { "piece": "badge" },
            { "text": "no piece" },
            { "piece": "table", "rows": [[{ "piece": "nope" }, "ok"]] },
            { "piece": "text", "text": "fine" },
        ]}));
        let Node::Row { children } = node else {
            panic!("a row: {node:?}")
        };
        let why = |n: usize| match &children[n] {
            Node::Invalid { why } => why.clone(),
            other => panic!("invalid: {other:?}"),
        };
        assert!(why(0).contains("sparkle"));
        assert!(why(1).contains("badge"));
        assert!(why(2).contains("without"));
        let Node::Table { rows, .. } = &children[3] else {
            panic!("a table: {:?}", children[3])
        };
        assert!(matches!(rows[0][0], Node::Invalid { .. }));
        assert_eq!(rows[0][1], Node::text("ok"));
        assert_eq!(children[4], Node::text("fine"));
    }

    /// Settings pieces are kept as they came, for the page that binds
    /// them.
    #[test]
    fn settings_pieces_pass_through() {
        let piece = json!({ "piece": "toggle", "key": "on", "label": null });
        let node = Node::read(&piece);
        assert!(node.is_setting());
        assert_eq!(node.value(), piece);
    }

    /// Sections and tabs read and write their wire shape; where they
    /// were left is UI state under their `id`, apart from the tree.
    #[test]
    fn sections_and_tabs() {
        let wire = json!({ "piece": "stack", "children": [
            { "piece": "section", "id": "more", "title": "More", "open": true,
              "children": [{ "piece": "text", "text": "in" }] },
            { "piece": "tabs", "id": "views", "tabs": [
                { "label": "One", "children": [{ "piece": "rule" }] },
                { "label": "Two", "children": [{ "piece": "nope" }] },
            ] },
        ]});
        let node = Node::read(&wire);
        let Node::Stack { children } = &node else {
            panic!("a stack: {node:?}")
        };
        let Node::Tabs { tabs, .. } = &children[1] else {
            panic!("tabs: {:?}", children[1])
        };
        assert!(matches!(tabs[1].children[0], Node::Invalid { .. }));
        assert_eq!(Node::read(&node.value()), node);
        assert_eq!(open_key("more"), "more.open");
        assert_eq!(draft_key("note"), "note.draft");
        let widgets = json!({ "piece": "stack", "children": [
            { "piece": "diff", "text": "@@ -1 +1 @@\n-a\n+b\n", "path": "src/lib.rs" },
            { "piece": "terminal", "id": "out", "text": "\u{1b}[32mok\u{1b}[0m\r\n",
              "cols": 40, "rows": 4, "live": false, "fold": true },
        ]});
        assert_eq!(Node::read(&widgets).value(), widgets);
        let more = json!({ "piece": "stack", "children": [
            { "piece": "ticker", "id": "load", "since": 1000, "until": null },
            { "piece": "graph", "height": 10.0,
              "nodes": [
                { "id": "a", "label": "A", "x": 0.5, "y": 0.5, "tone": "neutral", "focus": true, "to": null },
                { "id": "b", "label": null, "x": 0.25, "y": 0.25, "tone": "info", "focus": false,
                  "to": { "plugin": null, "page": "note", "params": { "id": "b" } } },
              ],
              "edges": [["a", "b"]] },
        ]});
        assert_eq!(Node::read(&more).value(), more);
        let setting = json!({ "piece": "card", "children": [
            { "piece": "switch", "label": "Compact idle chats", "detail": null,
              "on": true, "action": "idle", "args": null },
        ]});
        assert_eq!(Node::read(&setting).value(), setting);
        let modal = json!({ "piece": "modal", "title": "Add server",
            "children": [{ "piece": "input", "id": "name", "placeholder": null,
                           "multiline": false, "submit": null, "keep": true, "secret": false, "focus": null }],
            "dismiss": "cancel" });
        assert_eq!(Node::read(&modal).value(), modal);
        assert_eq!(
            Node::read(&json!({ "piece": "terminal", "id": "t" })),
            Node::Terminal {
                id: "t".into(),
                text: String::new(),
                cols: 80,
                rows: 24,
                live: false,
                fold: false,
            }
        );
        let input = json!({ "piece": "input", "id": "note", "placeholder": null,
                            "multiline": true, "submit": "save", "keep": false, "secret": false, "focus": null });
        assert_eq!(Node::read(&input).value(), input);
        assert_eq!(tab_key("views"), "views.tab");
    }

    /// The pieces of the printed language read and write their wire
    /// shape; what a builder leaves out is neutral, quiet and unmarked,
    /// and a color or a state with no such name does not read.
    #[test]
    fn marks_stamps_and_ticks() {
        let wire = json!({ "piece": "row", "children": [
            { "piece": "kanji", "text": "憶", "tone": "teal" },
            { "piece": "ticks", "lit": 6, "total": 21, "tone": "bad", "marked": [6, 16] },
            { "piece": "mark", "state": "asks", "count": 3 },
            { "piece": "stamp", "label": "exit 0", "tone": "good" },
            { "piece": "square", "label": "1", "style": "outlined", "tone": "yellow" },
            { "piece": "slip", "text": "Cap applies to server hints too", "tone": "teal", "dashed": true },
            { "piece": "figure", "text": "14:00" },
        ]});
        let node = Node::read(&wire);
        assert!(
            matches!(&node, Node::Row { children } if children.iter().all(|child| !matches!(child, Node::Invalid { .. }))),
            "{node:?}"
        );
        assert_eq!(node.value(), wire);
        assert_eq!(
            Node::read(&json!({ "piece": "ticks", "total": 10 })),
            Node::Ticks {
                lit: 0,
                total: 10,
                tone: Ink::Neutral,
                marked: Vec::new(),
                labels: Vec::new(),
            }
        );
        assert_eq!(
            Node::read(&json!({ "piece": "square", "label": "2" })),
            Node::Square {
                label: "2".into(),
                style: SquareLook::Quiet,
                tone: Ink::Neutral,
                action: None,
                args: Value::Null,
            }
        );
        assert_eq!(
            Node::read(&json!({ "piece": "mark", "state": "met" })),
            Node::Mark {
                state: MarkState::Met,
                count: None,
            }
        );
        for bad in [
            json!({ "piece": "mark", "state": "happy" }),
            json!({ "piece": "stamp", "label": "x", "tone": "mauve" }),
            json!({ "piece": "ticks", "lit": 2 }),
        ] {
            assert!(matches!(Node::read(&bad), Node::Invalid { .. }), "{bad}");
        }
    }

    /// The pieces results and change lists are made of read and write
    /// their wire shape; what a builder leaves out is not written, so a
    /// piece from before them reads and writes as it did.
    #[test]
    fn headers_bands_rails_and_marked_lines() {
        let wire = json!({ "piece": "stack", "children": [
            { "piece": "header", "title": "Changes", "meta": "3 files", "actions": [
                { "piece": "button", "label": "Land", "action": "land", "args": {},
                  "kind": "hot", "icon": null },
            ] },
            { "piece": "heading", "text": "Recalled", "tone": "teal" },
            { "piece": "band", "children": [
                { "piece": "marked", "text": "let retry = after;", "spans": [[5, 9], [13, 17]] },
            ] },
            { "piece": "rail", "children": [
                { "piece": "row", "children": [
                    { "piece": "square", "label": "1", "style": "filled", "tone": "info",
                      "action": "pick", "args": { "n": 1 } },
                    { "piece": "text", "text": "first" },
                ] },
            ] },
            { "piece": "slip", "text": "Cap applies to hints", "tone": "teal", "dashed": false,
              "to": { "plugin": null, "page": "note", "params": { "id": "cap" } } },
            { "piece": "button", "label": "Start", "action": "start", "args": {},
              "kind": "primary", "icon": null, "size": "large" },
        ]});
        let node = Node::read(&wire);
        let Node::Stack { children } = &node else {
            panic!("a stack: {node:?}")
        };
        assert!(
            children
                .iter()
                .all(|child| !matches!(child, Node::Invalid { .. })),
            "{node:?}"
        );
        assert_eq!(node.value(), wire);
        assert_eq!(
            children[5],
            Node::Button {
                label: "Start".into(),
                action: "start".into(),
                args: json!({}),
                kind: Kind::Primary,
                icon: None,
                size: ButtonSize::Large,
                wide: false,
                detail: None,
            }
        );
        assert_eq!(
            Node::read(&json!({ "piece": "header", "title": "Files" })),
            Node::Header {
                title: "Files".into(),
                meta: None,
                actions: Vec::new(),
            }
        );
        assert_eq!(
            Node::read(&json!({ "piece": "marked", "text": "plain" })),
            Node::Marked {
                text: "plain".into(),
                spans: Vec::new(),
            }
        );
        // Pieces from before these: nothing new is written for them.
        for old in [
            json!({ "piece": "heading", "text": "Rules" }),
            json!({ "piece": "square", "label": "2", "style": "quiet", "tone": "neutral" }),
            json!({ "piece": "slip", "text": "kept", "tone": "good", "dashed": false }),
            json!({ "piece": "button", "label": "Go", "action": "go", "args": {},
                    "kind": "secondary", "icon": null }),
        ] {
            let node = Node::read(&old);
            assert!(!matches!(node, Node::Invalid { .. }), "{node:?}");
            assert_eq!(node.value(), old);
        }
        assert_eq!(
            Node::read(&json!({ "piece": "heading", "text": "Rules" })),
            Node::Heading {
                text: "Rules".into(),
                tone: Ink::Neutral,
            }
        );
        for bad in [
            json!({ "piece": "button", "label": "Go", "action": "go", "size": "huge" }),
            json!({ "piece": "heading", "text": "x", "tone": "mauve" }),
            json!({ "piece": "marked", "text": "x", "spans": [[1]] }),
            json!({ "piece": "header" }),
        ] {
            assert!(matches!(Node::read(&bad), Node::Invalid { .. }), "{bad}");
        }
    }

    /// Wide buttons, gaps, tick labels, the printed QR code and segments
    /// read and write their wire shape; what a builder leaves out is not
    /// written, so a piece from before them reads and writes as it did.
    #[test]
    fn wide_buttons_gaps_labels_and_segments() {
        let wire = json!({ "piece": "stack", "children": [
            { "piece": "button", "label": "Start a run", "action": "start", "args": {},
              "kind": "primary", "icon": null, "size": "large", "wide": true,
              "detail": "on main, with the plan" },
            { "piece": "button", "label": "Skip", "action": "skip", "args": {},
              "kind": "secondary", "icon": null, "wide": true },
            { "piece": "gap", "size": "large" },
            { "piece": "ticks", "lit": 6, "total": 21, "tone": "info", "marked": [16],
              "labels": [{ "at": 0, "text": "0" }, { "at": 16, "text": "compacts" }] },
            { "piece": "qr", "text": "heiko://pair", "size": 200.0, "offset": true },
            { "piece": "segments",
              "choices": [
                { "label": "Daily", "value": "daily" },
                { "label": "Weekly", "value": "weekly" },
                { "label": "3", "value": 3 },
              ],
              "picked": "weekly", "action": "every", "args": { "agent": "tidy" } },
        ]});
        let node = Node::read(&wire);
        let Node::Stack { children } = &node else {
            panic!("a stack: {node:?}")
        };
        assert!(
            children
                .iter()
                .all(|child| !matches!(child, Node::Invalid { .. })),
            "{node:?}"
        );
        assert_eq!(node.value(), wire);
        assert_eq!(Node::read(&node.value()), node);
        assert_eq!(
            children[0],
            Node::Button {
                label: "Start a run".into(),
                action: "start".into(),
                args: json!({}),
                kind: Kind::Primary,
                icon: None,
                size: ButtonSize::Large,
                wide: true,
                detail: Some("on main, with the plan".into()),
            }
        );
        assert_eq!(
            children[3],
            Node::Ticks {
                lit: 6,
                total: 21,
                tone: Ink::Info,
                marked: vec![16],
                labels: vec![
                    TickLabel {
                        at: 0,
                        text: "0".into(),
                    },
                    TickLabel {
                        at: 16,
                        text: "compacts".into(),
                    },
                ],
            }
        );
        // What a builder leaves out: a medium gap, a plain QR code,
        // nothing picked.
        assert_eq!(
            Node::read(&json!({ "piece": "gap" })),
            Node::Gap {
                size: GapSize::Medium,
            }
        );
        assert_eq!(
            Node::read(&json!({ "piece": "qr", "text": "x", "size": 120.0 })),
            Node::Qr {
                text: "x".into(),
                size: 120.,
                offset: false,
            }
        );
        assert_eq!(
            Node::read(&json!({ "piece": "segments", "action": "pick" })),
            Node::Segments {
                choices: Vec::new(),
                picked: Value::Null,
                action: "pick".into(),
                args: Value::Null,
            }
        );
        // Pieces from before these: nothing new is written for them.
        for old in [
            json!({ "piece": "button", "label": "Go", "action": "go", "args": {},
                    "kind": "primary", "icon": null, "size": "medium" }),
            json!({ "piece": "ticks", "lit": 2, "total": 10, "tone": "good", "marked": [] }),
            json!({ "piece": "qr", "text": "x", "size": 200.0 }),
        ] {
            let node = Node::read(&old);
            assert!(!matches!(node, Node::Invalid { .. }), "{node:?}");
            assert_eq!(node.value(), old);
        }
        for bad in [
            json!({ "piece": "gap", "size": "huge" }),
            json!({ "piece": "ticks", "total": 4, "labels": [{ "at": 1 }] }),
            json!({ "piece": "segments", "choices": [{ "label": "x", "value": 1 }] }),
            json!({ "piece": "segments", "action": "pick", "choices": [{ "value": 1 }] }),
            json!({ "piece": "button", "label": "Go", "action": "go", "wide": "yes" }),
        ] {
            assert!(matches!(Node::read(&bad), Node::Invalid { .. }), "{bad}");
        }
    }

    /// Every icon a tree can name is found by its file's name.
    #[test]
    fn icons_by_name() {
        assert_eq!(Icon::named("sub-agent"), Some(Icon::SubAgent));
        assert_eq!(Icon::named("memory"), Some(Icon::Memory));
        assert_eq!(Icon::named("no-such-icon"), None);
    }
}
