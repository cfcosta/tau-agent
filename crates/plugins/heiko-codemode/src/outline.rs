//! A script's output on its card, as an outline: one line per item,
//! which opens on a click to what the item holds. The items come from
//! the result's details ([`crate::result::output_details`]), so a
//! string the script gave stays text and any other value stays JSON.
//! It is a view tree (ADR 0045): each item a line after its numbered
//! square, whether it is open the plugin's UI state.

use heiko_ui_kit::assets::Icon;
use heiko_ui_plugin::tree::{Ink, KeyValue, Node, SquareLook, Tone, open_key};
use serde_json::{Map, Value, json};

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

/// Whether an item starts open, before the person clicks: a lone item
/// opens by itself.
pub fn starts_open(total: usize) -> bool {
    total == 1
}

/// The id a call's item opens under: its UI state key is this id's
/// [`open_key`].
pub fn key(call_id: &str, index: usize) -> String {
    format!("codemode-output-{call_id}-{index}")
}

/// The action of an item's square: set the UI state `key` to `open`.
pub const OPEN: &str = "open";

/// The outline of a call's `items`: each a line after its numbered
/// square, a hairline between them. A click on the square opens the
/// item to what it holds, on a band under its line; `open` says which
/// are. An image, which holds nothing to read, does not open.
pub fn outline(
    call_id: &str,
    items: &[Shown],
    open: &dyn Fn(usize) -> bool,
) -> Node {
    let quiet = |text: &str| Node::Toned {
        text: text.to_owned(),
        tone: Tone::Neutral,
        mono: true,
    };
    let mut rows = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            rows.push(Node::Rule);
        }
        let line = line(item);
        let opens = !matches!(item, Shown::Image { .. });
        let open = opens && open(index);
        let mut row = vec![
            Node::Square {
                label: (index + 1).to_string(),
                style: if open {
                    SquareLook::Filled
                } else {
                    SquareLook::Quiet
                },
                tone: if open { Ink::Info } else { Ink::Neutral },
                action: opens.then(|| OPEN.to_owned()),
                args: if opens {
                    json!({
                        "key": open_key(&key(call_id, index)),
                        "open": !open,
                    })
                } else {
                    Value::Null
                },
            },
            Node::Mono { text: line.title },
        ];
        if !line.rest.is_empty() {
            row.push(quiet(&line.rest));
        }
        row.push(Node::Spacer);
        if !line.meta.is_empty() {
            row.push(quiet(&line.meta));
        }
        rows.push(Node::row(row));
        if open {
            rows.push(Node::Band {
                children: opened(item),
            });
        }
    }
    Node::stack(rows)
}

/// What an open item holds.
fn opened(item: &Shown) -> Vec<Node> {
    match item {
        Shown::Text(text) => text_block(text),
        Shown::Json(Value::Object(fields)) => object(fields),
        Shown::Json(Value::Array(entries)) => {
            let more = entries.len().saturating_sub(ARRAY_ROWS);
            let mut nodes = vec![Node::Rows {
                rows: entries
                    .iter()
                    .take(ARRAY_ROWS)
                    .enumerate()
                    .map(|(index, entry)| KeyValue {
                        key: index.to_string(),
                        value: compact(entry, 200),
                    })
                    .collect(),
            }];
            if more > 0 {
                nodes.push(Node::text(format!("… {more} more")));
            }
            nodes
        }
        Shown::Json(other) => vec![Node::Mono {
            text: compact(other, 200),
        }],
        Shown::Image { .. } => Vec::new(),
        Shown::Cut { head, bytes, .. } => {
            let mut nodes = text_block(head);
            nodes.push(Node::text(format!(
                "The card keeps the first {}; the model read all {}.",
                size(head.len()),
                size(*bytes)
            )));
            nodes
        }
    }
}

/// An object's fields: plain ones as rows, long strings as blocks under
/// them.
fn object(fields: &Map<String, Value>) -> Vec<Node> {
    let long = |value: &Value| {
        value.as_str().is_some_and(|text| {
            text.len() > SHORT_STRING || text.contains('\n')
        })
    };
    let mut nodes = vec![Node::Rows {
        rows: fields
            .iter()
            .filter(|(_, value)| !long(value))
            .map(|(key, value)| KeyValue {
                key: key.clone(),
                value: compact(value, 200),
            })
            .collect(),
    }];
    for (key, value) in fields.iter().filter(|(_, value)| long(value)) {
        nodes.push(Node::Toned {
            text: key.clone(),
            tone: Tone::Neutral,
            mono: true,
        });
        nodes.extend(text_block(value.as_str().unwrap_or_default()));
    }
    nodes
}

/// Up to [`TEXT_LINES`] of `text`, and how many more there are.
fn text_block(text: &str) -> Vec<Node> {
    let lines: Vec<&str> = text.lines().collect();
    let more = lines.len().saturating_sub(TEXT_LINES);
    let mut nodes = vec![Node::Mono {
        text: lines[..lines.len() - more].join("\n"),
    }];
    if more > 0 {
        nodes.push(Node::text(format!("… {more} more lines")));
    }
    nodes
}
