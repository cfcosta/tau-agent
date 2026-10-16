//! A script's output on its card: the details keep each item as the
//! script made it, strings as text and other values as JSON, and the
//! outline gives each a line. How the details are cut is
//! `tau-codemode`'s `output_details_keep_what_fits_and_cut_the_rest`.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{collections::BTreeSet, sync::Arc};

use common::{FakeHost, script};
use hegel::generators as gs;
use serde_json::{Value, json};
use tau_codemode::outline::{self, Line, Shown, line};
use tau_codemode_host::Item;
use tau_ui_kit::assets::Icon;

/// Strings stay text and other values JSON, in order, whether the
/// script gave them to `text`, `print` or `return`.
#[tokio::test]
async fn the_details_keep_each_item_as_made() {
    let outcome = script(
        &Arc::new(FakeHost::default()),
        "text('a')\ntext({ n = 1 })\nprint('b', 2)\nreturn 'c', { 3 }",
    )
    .await;
    assert_eq!(
        outline::items(&outcome.details()),
        [
            Shown::Text("a".into()),
            Shown::Json(json!({ "n": 1 })),
            Shown::Text("b\t2".into()),
            Shown::Text("c".into()),
            Shown::Json(json!([3])),
        ]
    );
}

/// The model reads a JSON item as text, as before.
#[tokio::test]
async fn the_model_reads_json_as_text() {
    let outcome =
        script(&Arc::new(FakeHost::default()), "return { n = 1 }").await;
    let rendered = outcome.render(10_000);
    assert!(
        rendered
            .content
            .iter()
            .all(|item| !matches!(item, Item::Json(_)))
    );
    assert_eq!(rendered.content[1], Item::Text(r#"{"n":1}"#.into()));
}

/// An item of a script's output, by the shape its line takes.
#[derive(Debug, Clone)]
enum Shape {
    Text(bool, Vec<String>),
    File(String, Vec<(String, i64)>, Vec<String>),
    Folder(String, usize),
    Fields(Vec<(String, i64)>),
    List(Vec<i64>),
    Scalar(Option<i64>),
}

hegel::pretty_print_as_debug!(Shape);

#[hegel::composite]
fn fields(tc: &hegel::TestCase, min: usize) -> Vec<(String, i64)> {
    let keys: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[a-o]{1,4}").fullmatch(true))
            .min_size(min)
            .max_size(5)
            .unique(true),
    );
    keys.into_iter()
        .map(|key| {
            (
                key,
                tc.draw(gs::integers::<i64>().min_value(-99).max_value(99)),
            )
        })
        .collect()
}

#[hegel::composite]
fn shape(tc: &hegel::TestCase) -> Shape {
    let path =
        || gs::from_regex("[a-z]{1,8}(/[a-z]{1,8}){0,2}").fullmatch(true);
    let lines = |min| {
        gs::vecs(gs::from_regex("[a-z ]{0,8}").fullmatch(true))
            .min_size(min)
            .max_size(5)
    };
    match tc.draw(gs::integers::<u8>().max_value(5)) {
        0 => Shape::Text(tc.draw(gs::booleans()), tc.draw(lines(0))),
        1 => {
            Shape::File(tc.draw(path()), tc.draw(fields(0)), tc.draw(lines(2)))
        }
        2 => Shape::Folder(
            tc.draw(path()),
            tc.draw(gs::integers::<usize>().max_value(4)),
        ),
        3 => Shape::Fields(tc.draw(fields(1))),
        4 => Shape::Scalar(tc.draw(gs::optional(gs::integers::<i64>()))),
        _ => Shape::List(
            tc.draw(
                gs::vecs(gs::integers::<i64>().min_value(-9).max_value(9))
                    .max_size(5),
            ),
        ),
    }
}

/// `n` of a thing, as a person counts.
fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How many lines `text` has: one more than its breaks, unless it ends
/// with one.
fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.matches('\n').count() + usize::from(!text.ends_with('\n'))
}

/// A text's line count, as `meta`: nothing for one line or none.
fn lines_meta(lines: usize) -> String {
    if lines > 1 {
        format!("{lines} lines")
    } else {
        String::new()
    }
}

/// Plain fields as a line reads them: the first three, in order.
fn plain(fields: &[(String, i64)]) -> String {
    let shown: Vec<String> = fields
        .iter()
        .take(3)
        .map(|(key, value)| format!("{key}: {value}"))
        .collect();
    shown.join(", ")
}

/// Whatever an item holds, its line says what it is: text by its first
/// line and how many lines it has; a read file by its path and its text's
/// lines; a listing by its path and how many entries; other objects by
/// their first plain fields and how many keys; a list by how many items
/// and its first; any other value as itself. The counts are taken here from what was generated.
#[hegel::test(test_cases = 300)]
fn a_line_says_what_its_item_is(tc: hegel::TestCase) {
    let shape = tc.draw(shape());
    let (shown, expected) = match &shape {
        Shape::Text(blank_first, lines) => {
            let text = format!(
                "{}{}",
                if *blank_first { "\n" } else { "" },
                lines.join("\n")
            );
            let count = line_count(&text);
            let title = lines
                .iter()
                .map(|line| line.trim())
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_owned();
            let expected = Line {
                icon: Icon::Text,
                title,
                rest: String::new(),
                meta: lines_meta(count),
            };
            (Shown::Text(text), expected)
        }
        Shape::File(path, fields, lines) => {
            let mut object = serde_json::Map::new();
            object.insert("path".into(), json!(path));
            for (key, value) in fields {
                object.insert(key.clone(), json!(value));
            }
            object.insert("text".into(), json!(lines.join("\n")));
            let expected = Line {
                icon: Icon::File,
                title: path.clone(),
                rest: plain(fields),
                meta: lines_meta(line_count(&lines.join("\n"))),
            };
            (Shown::Json(Value::Object(object)), expected)
        }
        Shape::Folder(path, entries) => {
            let listing: Vec<Value> = (0..*entries)
                .map(|i| json!({ "name": format!("{i}.md") }))
                .collect();
            let expected = Line {
                icon: Icon::Folder,
                title: path.clone(),
                rest: String::new(),
                meta: format!("{entries} entries"),
            };
            (
                Shown::Json(json!({ "path": path, "entries": listing })),
                expected,
            )
        }
        Shape::Fields(fields) => {
            let object: serde_json::Map<String, Value> = fields
                .iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect();
            let expected = Line {
                icon: Icon::Braces,
                title: plain(fields),
                rest: String::new(),
                meta: counted(fields.len(), "key", "keys"),
            };
            (Shown::Json(Value::Object(object)), expected)
        }
        Shape::List(items) => {
            let expected = Line {
                icon: Icon::Braces,
                title: counted(items.len(), "item", "items"),
                rest: items
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                meta: String::new(),
            };
            (Shown::Json(json!(items)), expected)
        }
        Shape::Scalar(value) => {
            let value = json!(value);
            let expected = Line {
                icon: Icon::Braces,
                title: value.to_string(),
                rest: String::new(),
                meta: String::new(),
            };
            (Shown::Json(value), expected)
        }
    };
    assert_eq!(line(&shown), expected, "{shown:?}");
}

/// A lone item opens by itself; a click turns over what it would be,
/// for any number of items.
#[test]
fn a_lone_item_starts_open() {
    let key = outline::key("call_1", 0);
    for total in 0..4 {
        for clicked in [false, true] {
            let open: BTreeSet<String> =
                clicked.then(|| key.clone()).into_iter().collect();
            assert_eq!(
                outline::is_open(&open, &key, total),
                (total == 1) != clicked,
                "{total} items, clicked {clicked}"
            );
        }
    }
}
