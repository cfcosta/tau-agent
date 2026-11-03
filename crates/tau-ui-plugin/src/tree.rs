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
        heading,
        icon,
        mono,
        pill,
        rule,
        table,
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
    Table {
        #[serde(default)]
        head: Vec<String>,
        #[serde(default, deserialize_with = "table_rows")]
        rows: Vec<Vec<Node>>,
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
    /// What shows where there is nothing yet.
    Empty {
        text: String,
    },
    /// A button that asks the plugin to carry out `action` with `args`.
    Button {
        label: String,
        action: String,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        kind: Kind,
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
}

impl<'a> Draw<'a> {
    pub fn new(theme: &'a Theme, id: impl Into<SharedString>) -> Self {
        Self {
            theme,
            id: id.into(),
            on_act: None,
            setting: None,
        }
    }

    pub fn on_act(mut self, on_act: OnAct) -> Self {
        self.on_act = Some(on_act);
        self
    }

    pub fn setting(mut self, setting: DrawSetting<'a>) -> Self {
        self.setting = Some(setting);
        self
    }

    /// Draws `node`.
    pub fn draw(&self, node: &Node) -> AnyElement {
        self.at(node, 0, &self.id)
    }

    fn children(
        &self,
        nodes: &[Node],
        depth: usize,
        path: &str,
    ) -> Vec<AnyElement> {
        nodes
            .iter()
            .take(MAX_CHILDREN)
            .enumerate()
            .map(|(n, child)| self.at(child, depth + 1, &format!("{path}.{n}")))
            .collect()
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

    fn at(&self, node: &Node, depth: usize, path: &str) -> AnyElement {
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
            Node::Table { head, rows } => table(
                &[],
                head.iter()
                    .map(|cell| cell.clone().into_any_element())
                    .collect(),
                rows.iter()
                    .take(MAX_CHILDREN)
                    .enumerate()
                    .map(|(n, row)| {
                        self.children(row, depth, &format!("{path}.{n}"))
                    })
                    .collect(),
                t,
            )
            .into_any_element(),
            Node::List { items } => div()
                .flex()
                .flex_col()
                .gap(sp(1.))
                .children(self.children(items, depth, path).into_iter().map(
                    |item| {
                        div()
                            .flex()
                            .gap(sp(2.))
                            .child(div().text_color(t.dim).child("•"))
                            .child(div().flex_1().min_w(rems(0.)).child(item))
                    },
                ))
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
                .children(self.children(children, depth, path))
                .into_any_element(),
            Node::Row { children } => div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .children(self.children(children, depth, path))
                .into_any_element(),
            Node::Scroll { height, children } => div()
                .id(SharedString::from(format!("{path}:scroll")))
                .flex()
                .flex_col()
                .gap(sp(2.))
                .max_h(rems(height.unwrap_or(24.).clamp(2., 80.)))
                .overflow_y_scroll()
                .children(self.children(children, depth, path))
                .into_any_element(),
            Node::Rule => rule(t).into_any_element(),
            Node::Empty { text } => empty(text.clone(), t).into_any_element(),
            Node::Button {
                label,
                action,
                args,
                kind,
            } => match self.click(Act::Plugin {
                action: action.clone(),
                args: args.clone(),
            }) {
                Some(click) => button(label.clone(), (*kind).into(), t)
                    .id(SharedString::from(format!("{path}:button")))
                    .on_click(click)
                    .into_any_element(),
                None => badge(label.clone(), t.muted, t.border_strong)
                    .into_any_element(),
            },
            Node::Link { label, to } => {
                match self.click(Act::Navigate(to.clone())) {
                    Some(click) => text_link(label.clone(), Type::SMALL, t)
                        .id(SharedString::from(format!("{path}:link")))
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
            { "piece": "button", "label": "Reset", "action": "reset", "args": { "n": 1 }, "kind": "danger" },
            { "piece": "row", "children": [{ "piece": "badge", "text": "x", "tone": "warn" }] },
            { "piece": "list", "items": [{ "piece": "mono", "text": "m" }] },
            { "piece": "table", "head": ["a"], "rows": [[{ "piece": "text", "text": "1" }]] },
            { "piece": "scroll", "height": 10.0, "children": [{ "piece": "rule" }] },
            { "piece": "link", "label": "Notes", "to": { "plugin": null, "page": "notes", "params": {} } },
            { "piece": "icon", "name": "memory", "tone": "info" },
        ]});
        let node = Node::read(&wire);
        assert!(
            matches!(&node, Node::Stack { children } if children.len() == 11)
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

    /// Every icon a tree can name is found by its file's name.
    #[test]
    fn icons_by_name() {
        assert_eq!(Icon::named("sub-agent"), Some(Icon::SubAgent));
        assert_eq!(Icon::named("memory"), Some(Icon::Memory));
        assert_eq!(Icon::named("no-such-icon"), None);
    }
}
