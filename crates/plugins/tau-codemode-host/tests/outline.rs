//! A script's output on its card: the details keep each item as the
//! script made it, strings as text and other values as JSON, and the
//! outline gives each a line.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{collections::BTreeSet, sync::Arc};

use common::{FakeHost, script};
use hegel::generators as gs;
use serde_json::{Value, json};
use tau_codemode::{
    outline::{self, Line, Shown, line},
    result::{CUT_HEAD_BYTES, MAX_OUTPUT_DETAIL_BYTES, output_details},
};
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

/// Past the limit, an item keeps its head, its size and that it was
/// cut; whatever the items, the details stay near the limit.
#[hegel::test(test_cases = 100)]
fn the_details_are_bounded(tc: hegel::TestCase) {
    let sizes: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().max_value(MAX_OUTPUT_DETAIL_BYTES))
            .max_size(6),
    );
    let items: Vec<Item> = sizes
        .iter()
        .map(|&n| Item::Text("é".repeat(n / 2)))
        .collect();
    let details = output_details(&items);
    assert_eq!(details.len(), items.len());
    let kept: usize = details
        .iter()
        .map(|item| item["text"].as_str().unwrap().len())
        .sum();
    assert!(kept <= MAX_OUTPUT_DETAIL_BYTES + items.len() * CUT_HEAD_BYTES);
    for (item, shown) in items.iter().zip(&details) {
        let text = item.text().unwrap();
        if shown["cut"] == true {
            assert_eq!(shown["bytes"], text.len());
            assert!(text.starts_with(shown["text"].as_str().unwrap()));
        } else {
            assert_eq!(shown["text"], text);
        }
    }
}

/// Fields read in the object's order (a script's come back sorted).
#[test]
fn a_read_file_is_a_file_line() {
    let read = json!({
        "path": "README.md",
        "kind": "text",
        "complete": true,
        "text": "# Ascend\n\nThe home page.\n",
        "artifact": { "id": "x" },
    });
    assert_eq!(
        line(&Shown::Json(read)),
        Line {
            icon: Icon::File,
            title: "README.md".into(),
            rest: "kind: \"text\", complete: true".into(),
            meta: "3 lines".into(),
        }
    );
}

#[test]
fn a_listing_is_a_folder_line() {
    let ls = json!({ "path": "docs", "entries": [{ "name": "a.md" }, { "name": "b.md" }] });
    let shown = line(&Shown::Json(ls));
    assert_eq!(
        (shown.icon, shown.title.as_str(), shown.meta.as_str()),
        (Icon::Folder, "docs", "2 entries")
    );
}

#[test]
fn other_values_read_by_their_fields() {
    let status = line(&Shown::Json(json!({ "clean": true, "change": "qyst" })));
    assert_eq!(
        (status.icon, status.title.as_str(), status.meta.as_str()),
        (Icon::Braces, "clean: true, change: \"qyst\"", "2 keys")
    );
    let list = line(&Shown::Json(json!([1, 2, 3])));
    assert_eq!((list.title.as_str(), list.rest.as_str()), ("3 items", "1"));
    let text = line(&Shown::Text("\n  first\nsecond".into()));
    assert_eq!(
        (text.icon, text.title.as_str(), text.meta.as_str()),
        (Icon::Text, "first", "3 lines")
    );
    assert_eq!(line(&Shown::Json(Value::Null)).title, "null");
}

/// A lone item opens by itself; a click turns over what it would be.
#[test]
fn a_lone_item_starts_open() {
    let key = outline::key("call_1", 0);
    let mut open = BTreeSet::new();
    assert!(outline::is_open(&open, &key, 1));
    assert!(!outline::is_open(&open, &key, 2));
    open.insert(key.clone());
    assert!(!outline::is_open(&open, &key, 1));
    assert!(outline::is_open(&open, &key, 2));
}
