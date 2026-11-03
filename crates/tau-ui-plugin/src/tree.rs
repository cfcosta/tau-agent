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
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use tau_ui_kit::{
    components::{ButtonKind, badge, button, code_block, dot, mono},
    prose::rich,
    theme::{self, Design as _, Theme, Type, radius, sp},
};

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

/// A line of [`Node::Rows`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub key: String,
    pub value: String,
}

/// One piece of a view tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Text(String),
    /// Text with `code` and `**bold**`.
    Rich(String),
    Mono(String),
    Badge {
        text: String,
        tone: Tone,
    },
    /// A dot, a line and small print: a plugin's status.
    Status {
        text: String,
        detail: Option<String>,
        tone: Tone,
    },
    Rows(Vec<Row>),
    List(Vec<Node>),
    /// A bar `share` of the way full, from 0 to 1.
    Progress {
        share: f64,
        label: Option<String>,
    },
    Code {
        lang: Option<String>,
        text: String,
    },
    /// Pieces one under the other.
    Stack(Vec<Node>),
    /// Pieces side by side, wrapping.
    Row(Vec<Node>),
    /// A button that asks the plugin to carry out `action` with `args`.
    Button {
        label: String,
        action: String,
        args: Value,
    },
    /// A piece bound to a key of the plugin's settings (`toggle`,
    /// `choice`, `field`), drawn on a settings page only; kept as it
    /// came, for the page that binds it.
    Setting(Value),
    /// A piece that did not read, and why.
    Invalid(String),
}

/// The pieces as the wire names them, for the derived parts of reading
/// and writing.
#[derive(Serialize, Deserialize)]
#[serde(tag = "piece", rename_all = "lowercase")]
enum Wire {
    Text {
        text: String,
    },
    Rich {
        text: String,
    },
    Mono {
        text: String,
    },
    Badge {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    Status {
        text: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        tone: Tone,
    },
    Rows {
        #[serde(default)]
        rows: Vec<Row>,
    },
    List {
        #[serde(default)]
        items: Vec<Node>,
    },
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
    Stack {
        #[serde(default)]
        children: Vec<Node>,
    },
    Row {
        #[serde(default)]
        children: Vec<Node>,
    },
    Button {
        label: String,
        action: String,
        #[serde(default)]
        args: Value,
    },
}

/// The pieces a settings page binds.
const SETTINGS: [&str; 3] = ["toggle", "choice", "field"];

impl Node {
    /// Reads a tree, never failing: what does not read becomes
    /// [`Node::Invalid`] where it stands.
    pub fn read(value: &Value) -> Self {
        if let Some(text) = value.as_str() {
            return Self::Text(text.to_owned());
        }
        let piece = value.get("piece").and_then(Value::as_str);
        if piece.is_some_and(|piece| SETTINGS.contains(&piece)) {
            return Self::Setting(value.clone());
        }
        match Wire::deserialize(value) {
            Ok(wire) => wire.into(),
            Err(error) => Self::Invalid(match piece {
                Some(piece) => format!("piece `{piece}`: {error}"),
                None => "a piece without `piece`".to_owned(),
            }),
        }
    }

    /// The tree as JSON, the shape [`Node::read`] takes.
    pub fn value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

impl From<Wire> for Node {
    fn from(wire: Wire) -> Self {
        match wire {
            Wire::Text { text } => Self::Text(text),
            Wire::Rich { text } => Self::Rich(text),
            Wire::Mono { text } => Self::Mono(text),
            Wire::Badge { text, tone } => Self::Badge { text, tone },
            Wire::Status { text, detail, tone } => {
                Self::Status { text, detail, tone }
            }
            Wire::Rows { rows } => Self::Rows(rows),
            Wire::List { items } => Self::List(items),
            Wire::Progress { share, label } => Self::Progress { share, label },
            Wire::Code { lang, text } => Self::Code { lang, text },
            Wire::Stack { children } => Self::Stack(children),
            Wire::Row { children } => Self::Row(children),
            Wire::Button {
                label,
                action,
                args,
            } => Self::Button {
                label,
                action,
                args,
            },
        }
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Value::deserialize(de).map(|value| Self::read(&value))
    }
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let wire = match self.clone() {
            Self::Setting(value) => return value.serialize(ser),
            Self::Invalid(why) => {
                return serde_json::json!({ "piece": "invalid", "why": why })
                    .serialize(ser);
            }
            Self::Text(text) => Wire::Text { text },
            Self::Rich(text) => Wire::Rich { text },
            Self::Mono(text) => Wire::Mono { text },
            Self::Badge { text, tone } => Wire::Badge { text, tone },
            Self::Status { text, detail, tone } => {
                Wire::Status { text, detail, tone }
            }
            Self::Rows(rows) => Wire::Rows { rows },
            Self::List(items) => Wire::List { items },
            Self::Progress { share, label } => Wire::Progress { share, label },
            Self::Code { lang, text } => Wire::Code { lang, text },
            Self::Stack(children) => Wire::Stack { children },
            Self::Row(children) => Wire::Row { children },
            Self::Button {
                label,
                action,
                args,
            } => Wire::Button {
                label,
                action,
                args,
            },
        };
        wire.serialize(ser)
    }
}

/// Carries out a button's action with its arguments.
pub type OnAction = Rc<dyn Fn(&str, &Value, &mut App)>;

/// Draws a bound settings piece.
pub type DrawSetting<'a> = &'a dyn Fn(&Value, &Theme) -> AnyElement;

/// What a tree is drawn with.
pub struct Draw<'a> {
    pub theme: &'a Theme,
    /// Names the tree's elements apart from other trees on screen.
    pub id: SharedString,
    /// What a button does; without it buttons draw but do nothing.
    pub on_action: Option<OnAction>,
    /// How bound settings pieces draw; without it they say they belong
    /// on a settings page.
    pub setting: Option<DrawSetting<'a>>,
}

impl<'a> Draw<'a> {
    pub fn new(theme: &'a Theme, id: impl Into<SharedString>) -> Self {
        Self {
            theme,
            id: id.into(),
            on_action: None,
            setting: None,
        }
    }

    pub fn on_action(mut self, on_action: OnAction) -> Self {
        self.on_action = Some(on_action);
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

    fn at(&self, node: &Node, depth: usize, path: &str) -> AnyElement {
        let t = self.theme;
        if depth > MAX_DEPTH {
            return invalid("the view nests too deep", t);
        }
        match node {
            Node::Text(text) => div()
                .typeset(Type::SMALL)
                .text_color(t.text_soft)
                .child(text.clone())
                .into_any_element(),
            Node::Rich(text) => div()
                .typeset(Type::SMALL)
                .child(rich(text, t.text_soft, t))
                .into_any_element(),
            Node::Mono(text) => mono(text.clone(), Type::CAPTION, t.text_soft)
                .into_any_element(),
            Node::Badge { text, tone } => {
                let color = t.tone((*tone).into());
                badge(text.clone(), color, color.opacity(0.4))
                    .into_any_element()
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
            Node::Rows(rows) => div()
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
            Node::List(items) => div()
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
            Node::Stack(children) => div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .children(self.children(children, depth, path))
                .into_any_element(),
            Node::Row(children) => div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .children(self.children(children, depth, path))
                .into_any_element(),
            Node::Button {
                label,
                action,
                args,
            } => match &self.on_action {
                Some(on_action) => {
                    let (on_action, action, args) =
                        (on_action.clone(), action.clone(), args.clone());
                    button(label.clone(), ButtonKind::Secondary, t)
                        .id(SharedString::from(format!("{path}:button")))
                        .on_click(move |_, _, cx| on_action(&action, &args, cx))
                        .into_any_element()
                }
                None => badge(label.clone(), t.muted, t.border_strong)
                    .into_any_element(),
            },
            Node::Setting(piece) => match self.setting {
                Some(setting) => setting(piece, t),
                None => invalid("a setting, drawn on a settings page only", t),
            },
            Node::Invalid(why) => invalid(why, t),
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
            { "piece": "button", "label": "Reset", "action": "reset", "args": { "n": 1 } },
            { "piece": "row", "children": [{ "piece": "badge", "text": "x", "tone": "warn" }] },
            { "piece": "list", "items": [{ "piece": "mono", "text": "m" }] },
        ]});
        let node = Node::read(&wire);
        assert!(matches!(&node, Node::Stack(children) if children.len() == 7));
        assert_eq!(Node::read(&node.value()), node);
        assert_eq!(node.value(), wire);
    }

    /// A bare string is text; nulls Luau writes for missing keys read as
    /// missing.
    #[test]
    fn strings_and_nulls() {
        assert_eq!(Node::read(&json!("hi")), Node::Text("hi".into()));
        assert_eq!(
            Node::read(&json!({ "piece": "code", "lang": null, "text": "x" })),
            Node::Code {
                lang: None,
                text: "x".into()
            }
        );
    }

    /// A piece that does not read is invalid where it stands; its
    /// siblings still read.
    #[test]
    fn bad_pieces_stay_local() {
        let node = Node::read(&json!({ "piece": "row", "children": [
            { "piece": "sparkle" },
            { "piece": "badge" },
            { "text": "no piece" },
            { "piece": "text", "text": "fine" },
        ]}));
        let Node::Row(children) = node else {
            panic!("a row: {node:?}")
        };
        assert!(
            matches!(&children[0], Node::Invalid(why) if why.contains("sparkle"))
        );
        assert!(
            matches!(&children[1], Node::Invalid(why) if why.contains("badge"))
        );
        assert!(matches!(&children[2], Node::Invalid(_)));
        assert_eq!(children[3], Node::Text("fine".into()));
    }

    /// Settings pieces are kept as they came, for the page that binds
    /// them.
    #[test]
    fn settings_pieces_pass_through() {
        let piece = json!({ "piece": "toggle", "key": "on", "label": null });
        assert_eq!(Node::read(&piece), Node::Setting(piece.clone()));
        assert_eq!(Node::read(&piece).value(), piece);
    }
}
