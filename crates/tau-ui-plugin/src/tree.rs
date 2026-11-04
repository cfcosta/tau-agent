//! The view tree (ADR 0045): what every plugin draws with, Rust or Luau.
//! A tree is plain data, the same on the computer and the phone, and the
//! interface draws it with tau-ui-kit's pieces.
//!
//! On the wire a piece is a JSON object named by its `piece` key, as
//! tau-luau-plugins' `tau.ui` builds it; a bare string is a `text`. A
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
use tau_ui_kit::{
    assets::Icon,
    components::{
        ButtonKind,
        badge,
        button,
        code_block,
        dot,
        empty,
        field,
        heading,
        icon,
        mono,
        pill,
        rule,
        text_link,
    },
    prose::{markdown, rich},
    theme::{self, Design as _, IconSize, Theme, Type, radius, sp},
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
}

impl From<Kind> for ButtonKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Primary => ButtonKind::Primary,
            Kind::Secondary => ButtonKind::Secondary,
            Kind::Danger => ButtonKind::Danger,
        }
    }
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
    Heading {
        text: String,
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
    /// A filled label: a count, a state.
    Pill {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    /// A dot, a line and small print: a plugin's status.
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
    },
    List {
        #[serde(default, deserialize_with = "nodes")]
        items: Vec<Node>,
    },
    /// A bar `share` of the way full, from 0 to 1.
    Progress {
        #[serde(default)]
        share: f64,
        #[serde(default)]
        label: Option<String>,
    },
    Code {
        #[serde(default)]
        lang: Option<String>,
        text: String,
    },
    /// Pieces one under the other.
    Stack {
        #[serde(default, deserialize_with = "nodes")]
        children: Vec<Node>,
    },
    /// Pieces side by side, wrapping.
    Row {
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
    /// Pieces in a card: a framed group.
    Card {
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
    /// the person clicks, `open` says.
    Section {
        id: String,
        title: String,
        #[serde(default)]
        open: bool,
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
    /// A unified diff, its lines colored as `path`'s language when known.
    Diff {
        text: String,
        #[serde(default)]
        path: Option<String>,
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
    Button {
        label: String,
        action: String,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        kind: Kind,
        #[serde(default)]
        icon: Option<String>,
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
    /// A piece that did not read, and why.
    #[serde(skip_deserializing)]
    Invalid {
        why: String,
    },
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
}

/// What a ticker is drawn from: its key, and the time it counts from and,
/// once it stops, to, in milliseconds since 1970.
pub struct TickerSpec {
    pub key: String,
    pub count: tau_ui_kit::ticker::Count,
}

/// A piece the interface draws with a view it keeps from one draw to
/// the next.
pub enum WidgetSpec {
    Terminal(TerminalSpec),
    Ticker(TickerSpec),
}

/// Draws a widget, kept by the interface from one draw to the next.
pub type Widgets<'a> = &'a dyn Fn(WidgetSpec, &mut App) -> AnyElement;

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
    pub on_submit: Option<OnSubmit>,
}

/// Carries out an input's Enter with what it holds.
pub type OnSubmit = Rc<dyn Fn(String, &mut App)>;

/// Gives the editor of an input, kept by the interface: made from its
/// draft, and sending each edit back as the draft (ADR 0045).
pub type Inputs<'a> = &'a dyn Fn(
    InputSpec,
    &mut App,
) -> gpui::Entity<tau_ui_kit::input::TextInput>;

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

/// Draws a bound settings piece, given as JSON.
pub type DrawSetting<'a> = &'a dyn Fn(&Value, &Theme) -> AnyElement;

/// What a tree is drawn with.
pub struct Draw<'a> {
    pub theme: &'a Theme,
    /// Names the tree's elements apart from other trees on screen.
    pub id: SharedString,
    /// What clicks do; without it buttons and links draw but do nothing.
    pub on_act: Option<OnAct>,
    /// How bound settings pieces draw; without it they say they belong
    /// on a settings page.
    pub setting: Option<DrawSetting<'a>>,
    /// The plugin's UI state, which sections and tabs read; without it
    /// each shows as it starts.
    pub ui: Option<&'a std::collections::BTreeMap<String, Value>>,
    /// What the tree's UI state keys start with: the plugin's own part,
    /// for a plugin that draws trees for several (tau-luau-plugins).
    pub prefix: Option<String>,
    /// Where inputs' editors come from; without it inputs show their
    /// draft and take no keys.
    pub inputs: Option<Inputs<'a>>,
    /// Where widgets' views come from (terminals, tickers); without it a
    /// widget shows that it is drawn where one is kept.
    pub widgets: Option<Widgets<'a>>,
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
            setting: None,
            ui: None,
            prefix: None,
            inputs: None,
            widgets: None,
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

    pub fn setting(mut self, setting: DrawSetting<'a>) -> Self {
        self.setting = Some(setting);
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

    /// A graph: its edges painted under its nodes, each node a dot with
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
                                .rounded(rems((size / 2.) / 16.))
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
            | Node::Button { .. }
            | Node::Link { .. } => div().flex().child(drawn).into_any_element(),
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
            Node::Heading { text } => heading(text, t).into_any_element(),
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
                let color = t.tone((*tone).into());
                badge(text.clone(), color, color.opacity(0.4))
                    .into_any_element()
            }
            Node::Pill { text, tone } => {
                let color = match tone {
                    Tone::Neutral => t.text_soft,
                    tone => t.tone((*tone).into()),
                };
                let bg = match tone {
                    Tone::Neutral => t.raised,
                    _ => color.opacity(0.12),
                };
                pill(text.clone(), color, bg).into_any_element()
            }
            Node::Status { text, detail, tone } => div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(dot(t.tone((*tone).into()), 7.))
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
            Node::Title { text, subtitle } => match subtitle {
                Some(subtitle) => tau_ui_kit::components::screen_title(
                    text.clone(),
                    subtitle.clone(),
                    t,
                )
                .into_any_element(),
                None => {
                    tau_ui_kit::components::title(text.clone(), Type::TITLE)
                        .text_color(t.text)
                        .into_any_element()
                }
            },
            Node::Table { head, rows, widths } => {
                tau_ui_kit::components::weighted_table(
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
                let share = share.clamp(0., 1.) as f32;
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(1.))
                    .when_some(label.clone(), |column, label| {
                        column.child(mono(label, Type::CAPTION, t.dim))
                    })
                    .child(
                        div()
                            .w_full()
                            .h(rems(0.375))
                            .rounded(radius::SMALL)
                            .bg(t.raised)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(share))
                                    .rounded(radius::SMALL)
                                    .bg(t.accent),
                            ),
                    )
                    .into_any_element()
            }
            Node::Code { lang, text } => {
                code_block(lang.as_deref(), text, t).into_any_element()
            }
            Node::Stack { children } => div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .children(self.children(children, depth, path, cx))
                .into_any_element(),
            Node::Row { children } => div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .children(self.children(children, depth, path, cx))
                .into_any_element(),
            Node::Scroll { height, children } => div()
                .id(SharedString::from(format!("{path}:scroll")))
                .flex()
                .flex_col()
                .gap(sp(2.))
                .max_h(rems(height.unwrap_or(24.).clamp(2., 80.)))
                .overflow_y_scroll()
                .children(self.children(children, depth, path, cx))
                .into_any_element(),
            Node::Rule => rule(t).into_any_element(),
            Node::Card { children } => tau_ui_kit::components::card(t)
                .flex()
                .flex_col()
                .gap(sp(2.))
                .px(sp(3.5))
                .py(sp(3.))
                .children(self.children(children, depth, path, cx))
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
                            column.child(tau_ui_kit::components::text(
                                detail,
                                Type::CAPTION,
                                t.muted,
                            ))
                        }),
                )
                .child(tau_ui_kit::components::switch(*on, t))
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
                children,
            } => {
                let key = self.key(open_key(id));
                let open = self
                    .ui
                    .and_then(|ui| ui.get(&key))
                    .and_then(Value::as_bool)
                    .unwrap_or(*open);
                let head = div()
                    .id(SharedString::from(format!("{path}:section")))
                    .debug_selector(selector("section", title))
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .cursor_pointer()
                    .child(icon(
                        if open { Icon::Down } else { Icon::Chevron },
                        IconSize::TINY,
                        t.dim,
                    ))
                    // Plain small text, not a heading: a title is often
                    // data, which capitals make hard to read.
                    .child(
                        div()
                            .min_w(rems(0.))
                            .typeset(Type::SMALL)
                            .text_color(t.text_soft)
                            .child(title.clone()),
                    )
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
                    .gap(sp(2.))
                    .child(head)
                    .when(open, |column| {
                        column
                            .children(self.children(children, depth, path, cx))
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
                let bar = div().flex().flex_wrap().gap(sp(0.5)).children(
                    tabs.iter().take(MAX_CHILDREN).enumerate().map(
                        |(n, tab)| {
                            let on = n == chosen;
                            div()
                                .id(SharedString::from(format!(
                                    "{path}:tab{n}"
                                )))
                                .debug_selector(selector("tab", &tab.label))
                                .px(sp(3.))
                                .py(sp(1.5))
                                .rounded(radius::SMALL)
                                .cursor_pointer()
                                .typeset(Type::CAPTION)
                                .text_color(if on { t.text } else { t.dim })
                                .when(on, |tab| tab.bg(t.raised))
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
                            self.children(
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
            Node::Diff { text, path } => tau_ui_kit::diff::view(
                &tau_ui_kit::diff::parse(text),
                path.as_deref().and_then(tau_ui_kit::syntax::Lang::of_path),
                t,
            )
            .into_any_element(),
            Node::Terminal {
                id,
                text,
                cols,
                rows,
                live,
            } => match self.widgets {
                Some(widgets) => widgets(
                    WidgetSpec::Terminal(TerminalSpec {
                        key: self.key(format!("{id}.terminal")),
                        bytes: text.as_bytes().to_vec(),
                        cols: (*cols).clamp(10, 400),
                        rows: (*rows).clamp(2, 200),
                        live: *live,
                    }),
                    cx,
                ),
                None => invalid(
                    "a terminal, drawn where the interface keeps one",
                    t,
                ),
            },
            Node::Ticker { id, since, until } => match self.widgets {
                Some(widgets) => widgets(
                    WidgetSpec::Ticker(TickerSpec {
                        key: self.key(format!("{id}.ticker")),
                        count: tau_ui_kit::ticker::Count::Since {
                            since: *since,
                            until: *until,
                        },
                    }),
                    cx,
                ),
                // Without a clock, what it read when it stopped, if it did.
                None => mono(
                    until.map_or_else(String::new, |until| {
                        tau_ui_kit::ticker::reading(*since, until)
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
                        count: tau_ui_kit::ticker::Count::Due { at: *at },
                    }),
                    cx,
                ),
                None => invalid(
                    "a countdown, drawn where the interface keeps one",
                    t,
                ),
            },
            Node::Spacer => div().flex_1().into_any_element(),
            Node::Graph {
                height,
                nodes,
                edges,
            } => self.graph(*height, nodes, edges, path),
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
                    .border_1()
                    .border_color(t.border_strong)
                    .rounded(radius::CARD)
                    .shadow_lg()
                    // A click on it stays in it.
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation()
                    })
                    .child(
                        div()
                            .typeset(Type::SUBTITLE)
                            .font_weight(tau_ui_kit::theme::weight::STRONG)
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
                    .rounded(radius::SMALL)
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
                ..
            } => match self.click(Act::Plugin {
                action: action.clone(),
                args: args.clone(),
            }) {
                Some(click) => button(label.clone(), (*kind).into(), t)
                    .id(SharedString::from(format!("{path}:button")))
                    .debug_selector(selector("button", label))
                    .on_click(click)
                    .into_any_element(),
                None => badge(label.clone(), t.muted, t.border_strong)
                    .into_any_element(),
            },
            Node::Link { label, to } => {
                match self.click(Act::Navigate(to.clone())) {
                    Some(click) => text_link(label.clone(), Type::SMALL, t)
                        .id(SharedString::from(format!("{path}:link")))
                        .debug_selector(selector("link", label))
                        .on_click(click)
                        .into_any_element(),
                    None => div()
                        .typeset(Type::SMALL)
                        .text_color(t.accent)
                        .child(label.clone())
                        .into_any_element(),
                }
            }
            Node::Toggle { .. } | Node::Choice { .. } | Node::Field { .. } => {
                match self.setting {
                    Some(setting) => setting(&node.value(), t),
                    None => {
                        invalid("a setting, drawn on a settings page only", t)
                    }
                }
            }
            Node::Invalid { why } => invalid(why, t),
        }
    }
}

/// An input drawn without an editor: its draft, as text.
fn field_text(draft: &str, t: &Theme) -> AnyElement {
    div()
        .px(sp(3.))
        .py(sp(2.))
        .rounded(radius::CONTROL)
        .border_1()
        .border_color(t.border)
        .typeset(Type::SMALL)
        .text_color(t.text_soft)
        .child(draft.to_owned())
        .into_any_element()
}

/// A clickable piece's name for tests (`debug_bounds`): its kind and
/// what it says, `button:Save`, `link:Memory`, `switch:Compact idle
/// chats`, `section:Details`, `tab:Rules`.
fn selector(kind: &str, label: &str) -> impl Fn() -> String + 'static {
    let name = format!("{kind}:{label}");
    move || name.clone()
}

/// What a piece that does not draw shows instead.
fn invalid(why: &str, t: &Theme) -> AnyElement {
    div()
        .px(sp(2.))
        .py(sp(1.))
        .rounded(radius::SMALL)
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

    /// Trees read in the shape `tau.ui` builds them, and write back the
    /// same.
    #[test]
    fn reads_and_writes_the_wire_shape() {
        let wire = json!({ "piece": "stack", "children": [
            { "piece": "status", "text": "on", "detail": "2 turns", "tone": "good" },
            { "piece": "rows", "rows": [{ "key": "a", "value": "1" }] },
            { "piece": "progress", "share": 0.5, "label": "half" },
            { "piece": "code", "lang": "rust", "text": "fn f() {}" },
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
                text: "x".into()
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
              "cols": 40, "rows": 4, "live": false },
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
                           "multiline": false, "submit": null, "keep": true, "secret": false }],
            "dismiss": "cancel" });
        assert_eq!(Node::read(&modal).value(), modal);
        assert_eq!(
            Node::read(&json!({ "piece": "terminal", "id": "t" })),
            Node::Terminal {
                id: "t".into(),
                text: String::new(),
                cols: 80,
                rows: 24,
                live: false
            }
        );
        let input = json!({ "piece": "input", "id": "note", "placeholder": null,
                            "multiline": true, "submit": "save", "keep": false, "secret": false });
        assert_eq!(Node::read(&input).value(), input);
        assert_eq!(tab_key("views"), "views.tab");
    }

    /// Every icon a tree can name is found by its file's name.
    #[test]
    fn icons_by_name() {
        assert_eq!(Icon::named("sub-agent"), Some(Icon::SubAgent));
        assert_eq!(Icon::named("memory"), Some(Icon::Memory));
        assert_eq!(Icon::named("no-such-icon"), None);
    }
}
