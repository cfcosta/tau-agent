//! heiko-vcs's UI: the picks its log cards keep as UI state, and the
//! trees its diff cards draw files with.

use std::collections::BTreeMap;

use hegel::generators as gs;
use heiko_agent::tool::RunId;
use heiko_ui_plugin::tree::Node;
use heiko_vcs::ui::{Card, change_diff::ChangeDiff, diff_card, pick, pick_key};
use serde_json::json;

/// Picking changes over any clicks: a log's pick is the last change
/// clicked unless clicked again, and each card keeps its own under its
/// own key.
#[hegel::test(test_cases = 200)]
fn a_log_keeps_the_change_last_clicked(tc: hegel::TestCase) {
    // (card, which)
    let clicks: Vec<(u8, u8)> = tc.draw(gs::vecs(hegel::tuples!(
        gs::integers::<u8>().max_value(2),
        gs::integers::<u8>().max_value(3),
    )));
    let run = RunId("r".into());
    let mut state: BTreeMap<String, String> = BTreeMap::new();
    let mut model: BTreeMap<String, String> = BTreeMap::new();
    for (card, which) in &clicks {
        let (card, which) = (format!("c{card}"), format!("x{which}"));
        let key = pick_key(&run, &card);
        match pick(state.get(&key).map(String::as_str), &which) {
            Some(change) => state.insert(key, change),
            None => state.remove(&key),
        };
        if model.get(&card) == Some(&which) {
            model.remove(&card);
        } else {
            model.insert(card.clone(), which.clone());
        }
    }
    for card in 0..3 {
        let card = format!("c{card}");
        assert_eq!(state.get(&pick_key(&run, &card)), model.get(&card));
    }
}

/// A diff card's files are rows, each path a button that opens the
/// file under its own key; an open file's hunks follow its row, as a
/// diff in its path's language.
#[test]
fn a_diffs_files_open_to_their_hunks() {
    let diff = ChangeDiff::parse(&json!({
        "change": {
            "change_id": "onvkmqwo", "commit_id": "abc", "description": "",
            "empty": false, "conflict": false, "immutable": false,
            "working_copy": false, "divergent": false, "bookmarks": [],
        },
        "files": [
            { "path": "src/a.rs", "kind": "modified" },
            { "path": "run.sh", "kind": "modified" },
        ],
        "diff": "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n",
        "truncated": false,
    }))
    .expect("a diff");
    let card = Card {
        run: RunId("r".into()),
        call_id: "c".into(),
        compact: false,
    };
    // The key a row's path sets, and what it sets it to.
    let sets = |row: &Node| match row {
        Node::Row { children } => children.iter().find_map(|node| match node {
            Node::Button { label, args, .. } => Some((
                label.clone(),
                args["key"].as_str().unwrap_or_default().to_owned(),
                args["value"].clone(),
            )),
            _ => None,
        }),
        _ => None,
    };
    let Node::Stack { children } =
        diff_card::files_body(&card, &diff, &|_| false)
    else {
        panic!("a stack of files");
    };
    // Closed, a row each.
    let rows: Vec<_> = children.iter().filter_map(sets).collect();
    assert_eq!(children.len(), 2);
    assert_eq!(
        rows,
        [
            ("src/a.rs".into(), card.file_key("src/a.rs"), json!(true)),
            ("run.sh".into(), card.file_key("run.sh"), json!(true)),
        ]
    );
    assert_ne!(rows[0].1, rows[1].1);
    let Node::Stack { children } =
        diff_card::files_body(&card, &diff, &|_| true)
    else {
        panic!("a stack of files");
    };
    assert_eq!(children.len(), 4);
    // An open file's path closes it.
    assert_eq!(sets(&children[0]).map(|row| row.2), Some(json!(null)));
    let Node::Diff { text, path } = &children[1] else {
        panic!("the hunks as a diff: {:?}", children[1]);
    };
    assert_eq!(path.as_deref(), Some("src/a.rs"));
    assert_eq!(text, "@@ -1,2 +1,2 @@\n keep\n-old\n+new\n");
    // A file with no hunks says so in place of a diff.
    assert_eq!(children[3], Node::text("No lines changed."));
}
