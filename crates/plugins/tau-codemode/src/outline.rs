//! A script's output on its card, as an outline: one line per item,
//! which opens on a click to what the item holds. The items come from
//! the result's details ([`crate::result::output_details`]), so a
//! string the script gave stays text and any other value stays JSON.

use std::collections::BTreeSet;

use gpui::{App, Div, Hsla, SharedString, div, prelude::*, rems};
use serde_json::{Map, Value};
use tau_ui_kit::{
    assets::Icon,
    components::{icon, mono},
    syntax::Kind,
    theme::{IconSize, Theme, Type, sp},
};

use crate::result::preview;

/// The most lines an open item shows of a text.
const TEXT_LINES: usize = 40;

/// The most entries an open array shows.
const ARRAY_ROWS: usize = 50;

/// A string longer than this, or with a line break, shows as a block
/// under an open object's fields, not on its row.
const SHORT_STRING: usize = 120;

/// One output item, as the details keep it.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Text(String),
    Json(Value),
    Image {
        mime: String,
        bytes: usize,
    },
    /// An item past the details' limit: its head, and its whole size.
    Cut {
        json: bool,
        head: String,
        bytes: usize,
    },
}

/// The items in a result's details, in order; none when it has no
/// `output`, as a call that failed before its script ran.
pub fn items(details: &Value) -> Vec<Shown> {
    let Some(output) = details.get("output").and_then(Value::as_array) else {
        return Vec::new();
    };
    output.iter().filter_map(shown).collect()
}

fn shown(item: &Value) -> Option<Shown> {
    let text = || item.get("text").and_then(Value::as_str).map(str::to_owned);
    let bytes = || {
        item.get("bytes")
            .and_then(Value::as_u64)
            .unwrap_or_default() as usize
    };
    let kind = item.get("kind").and_then(Value::as_str)?;
    if item.get("cut").and_then(Value::as_bool) == Some(true) {
        return Some(Shown::Cut {
            json: kind == "json",
            head: text()?,
            bytes: bytes(),
        });
    }
    Some(match kind {
        "text" => Shown::Text(text()?),
        "json" => Shown::Json(item.get("value")?.clone()),
        "image" => Shown::Image {
            mime: item.get("mime").and_then(Value::as_str)?.to_owned(),
            bytes: bytes(),
        },
        _ => return None,
    })
}

/// An item's line in the outline: what it is at a glance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub icon: Icon,
    pub title: String,
    /// Dimmed after the title.
    pub rest: String,
    /// On the right: its size, in lines, keys or entries.
    pub meta: String,
}

/// The line for `item`. An object with a `path` reads as a file, or as
/// a folder when it holds a list; any other object by its first plain
/// fields.
pub fn line(item: &Shown) -> Line {
    match item {
        Shown::Text(text) => Line {
            icon: Icon::Text,
            title: first_line(text),
            rest: String::new(),
            meta: lines_meta(text),
        },
        Shown::Json(Value::Object(fields)) => object_line(fields),
        Shown::Json(Value::Array(entries)) => Line {
            icon: Icon::Braces,
            title: count(entries.len(), "item", "items"),
            rest: entries
                .first()
                .map(|first| compact(first, 80))
                .unwrap_or_default(),
            meta: String::new(),
        },
        Shown::Json(value) => Line {
            icon: Icon::Braces,
            title: compact(value, 80),
            rest: String::new(),
            meta: String::new(),
        },
        Shown::Image { mime, bytes } => Line {
            icon: Icon::Camera,
            title: "image".into(),
            rest: mime.clone(),
            meta: size(*bytes),
        },
        Shown::Cut { json, head, bytes } => Line {
            icon: if *json { Icon::Braces } else { Icon::Text },
            title: first_line(head),
            rest: String::new(),
            meta: format!("{} · cut", size(*bytes)),
        },
    }
}

fn object_line(fields: &Map<String, Value>) -> Line {
    let path = fields.get("path").and_then(Value::as_str);
    let list = fields
        .iter()
        .find_map(|(key, value)| Some((key, value.as_array()?.len())));
    let meta = match (fields.get("text").and_then(Value::as_str), list) {
        (Some(text), _) if path.is_some() => lines_meta(text),
        (_, Some((key, len))) => format!("{len} {key}"),
        _ => count(fields.len(), "key", "keys"),
    };
    let plain = |skip: &str| {
        fields
            .iter()
            .filter(|(key, value)| *key != skip && is_plain(value))
            .take(3)
            .map(|(key, value)| format!("{key}: {value}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match path {
        Some(path) => Line {
            icon: if list.is_some() {
                Icon::Folder
            } else {
                Icon::File
            },
            title: path.to_owned(),
            rest: plain("path"),
            meta,
        },
        None => {
            let title = plain("");
            Line {
                icon: Icon::Braces,
                title: if title.is_empty() {
                    let keys = fields.keys().take(4).cloned();
                    format!("{{ {} }}", keys.collect::<Vec<_>>().join(", "))
                } else {
                    title
                },
                rest: String::new(),
                meta,
            }
        }
    }
}

/// A value short enough to read on one line: a number, a boolean,
/// `null` or a short string.
fn is_plain(value: &Value) -> bool {
    match value {
        Value::String(text) => {
            text.len() <= SHORT_STRING / 2 && !text.contains('\n')
        }
        Value::Object(_) | Value::Array(_) => false,
        _ => true,
    }
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    preview(line, 80)
}

fn lines_meta(text: &str) -> String {
    match text.lines().count() {
        0 | 1 => String::new(),
        n => format!("{n} lines"),
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `bytes` for a person: `512 B`, `7.5 KB`, `1.2 MB`.
pub fn size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

fn compact(value: &Value, max: usize) -> String {
    preview(&value.to_string(), max)
}

/// Whether the item at `index` of `total` is open: a lone item opens by
/// itself, and a click turns over what it would be.
pub fn is_open(open: &BTreeSet<String>, key: &str, total: usize) -> bool {
    open.contains(key) != (total == 1)
}

/// The key a call's item is opened under.
pub fn key(call_id: &str, index: usize) -> String {
    format!("output-{call_id}-{index}")
}

/// The outline of a call's `items`; a click on a line passes its key to
/// `toggle`.
pub fn outline(
    call_id: &str,
    items: &[Shown],
    open: &BTreeSet<String>,
    toggle: impl Fn(String, &mut App) + Clone + 'static,
    t: &Theme,
) -> Div {
    let mut list = div()
        .flex()
        .flex_col()
        .rounded(tau_ui_kit::theme::radius::BOX)
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .overflow_hidden();
    for (index, item) in items.iter().enumerate() {
        let key = key(call_id, index);
        let opens = !matches!(item, Shown::Image { .. });
        let is_open = opens && is_open(open, &key, items.len());
        let line = line(item);
        let toggle = toggle.clone();
        let mut row = div()
            .id(SharedString::from(key.clone()))
            .flex()
            .items_center()
            .gap(sp(2.))
            .px(sp(3.))
            .py(sp(1.5))
            .min_h(rems(2.))
            .when(index > 0, |row| row.border_t_1().border_color(t.border))
            .when(is_open, |row| row.bg(t.raised))
            .child(
                div().w(rems(0.75)).flex_none().children(opens.then(|| {
                    icon(
                        if is_open { Icon::Down } else { Icon::Chevron },
                        IconSize::SMALL,
                        t.dim,
                    )
                })),
            )
            .child(mono(
                format!("{}", index + 1),
                Type::MICRO,
                t.dim,
            ))
            .child(icon(line.icon, IconSize::SMALL, t.dim))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(rems(0.))
                    .gap(sp(2.))
                    .overflow_hidden()
                    .child(
                        mono(line.title, Type::CAPTION, t.text_soft)
                            .flex_none()
                            .max_w(rems(24.))
                            .truncate(),
                    )
                    .when(!line.rest.is_empty(), |cell| {
                        cell.child(
                            mono(line.rest, Type::CAPTION, t.dim)
                                .flex_1()
                                .min_w(rems(0.))
                                .truncate(),
                        )
                    }),
            )
            .when(!line.meta.is_empty(), |row| {
                row.child(mono(line.meta, Type::MICRO, t.dim).flex_none())
            });
        if opens {
            let key = key.clone();
            row = row
                .cursor_pointer()
                .on_click(move |_, _, cx| toggle(key.clone(), cx));
        }
        list = list.child(row);
        if is_open {
            list = list.child(
                div()
                    .px(sp(3.))
                    .pl(rems(3.5))
                    .py(sp(2.))
                    .border_t_1()
                    .border_color(t.border)
                    .child(opened(item, t)),
            );
        }
    }
    list
}

/// What an open item holds.
fn opened(item: &Shown, t: &Theme) -> Div {
    match item {
        Shown::Text(text) => text_block(text, t),
        Shown::Json(Value::Object(fields)) => object(fields, t),
        Shown::Json(Value::Array(entries)) => {
            let more = entries.len().saturating_sub(ARRAY_ROWS);
            div()
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .children(entries.iter().take(ARRAY_ROWS).enumerate().map(
                    |(index, entry)| {
                        div()
                            .flex()
                            .gap(sp(3.))
                            .child(
                                mono(index.to_string(), Type::CAPTION, t.dim)
                                    .w(rems(2.))
                                    .flex_none(),
                            )
                            .child(
                                value(entry, t)
                                    .flex_1()
                                    .min_w(rems(0.))
                                    .truncate(),
                            )
                    },
                ))
                .when(more > 0, |list| {
                    list.child(mono(
                        format!("… {more} more"),
                        Type::MICRO,
                        t.dim,
                    ))
                })
        }
        Shown::Json(other) => div().child(value(other, t)),
        Shown::Image { .. } => div(),
        Shown::Cut { head, bytes, .. } => text_block(head, t).child(mono(
            format!(
                "The card keeps the first {}; the model read all {}.",
                size(head.len()),
                size(*bytes)
            ),
            Type::MICRO,
            t.dim,
        )),
    }
}

/// An object's fields: plain ones as rows, long strings as blocks under
/// them.
fn object(fields: &Map<String, Value>, t: &Theme) -> Div {
    let long = |value: &Value| {
        value.as_str().is_some_and(|text| {
            text.len() > SHORT_STRING || text.contains('\n')
        })
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .children(fields.iter().filter(|(_, v)| !long(v)).map(|(key, v)| {
            div()
                .flex()
                .gap(sp(3.))
                .child(
                    mono(key.clone(), Type::CAPTION, t.syntax.color(Kind::Property))
                        .w(rems(9.))
                        .flex_none()
                        .truncate(),
                )
                .child(value(v, t).flex_1().min_w(rems(0.)).truncate())
        }))
        .children(fields.iter().filter(|(_, v)| long(v)).map(|(key, v)| {
            div()
                .flex()
                .flex_col()
                .gap(sp(1.))
                .pt(sp(2.))
                .child(mono(
                    key.clone(),
                    Type::CAPTION,
                    t.syntax.color(Kind::Property),
                ))
                .child(
                    div()
                        .pl(sp(2.5))
                        .border_l_2()
                        .border_color(t.border)
                        .child(text_block(v.as_str().unwrap_or_default(), t)),
                )
        }))
}

/// Up to [`TEXT_LINES`] of `text`, and how many more there are.
fn text_block(text: &str, t: &Theme) -> Div {
    let lines: Vec<&str> = text.lines().collect();
    let more = lines.len().saturating_sub(TEXT_LINES);
    div()
        .flex()
        .flex_col()
        .gap(sp(1.))
        .child(mono(
            lines[..lines.len() - more].join("\n"),
            Type::CAPTION,
            t.text_soft,
        ))
        .when(more > 0, |block| {
            block.child(mono(format!("… {more} more lines"), Type::MICRO, t.dim))
        })
}

/// A value on one line, in its kind's color.
fn value(value: &Value, t: &Theme) -> Div {
    let color: Hsla = match value {
        Value::String(_) => t.syntax.color(Kind::String),
        Value::Number(_) => t.syntax.color(Kind::Number),
        Value::Bool(_) | Value::Null => t.syntax.color(Kind::Constant),
        Value::Object(_) | Value::Array(_) => t.dim,
    };
    mono(compact(value, 200), Type::CAPTION, color)
}
