//! tau-vcs's UI: the picks its log cards keep as UI state, and the
//! trees its diff cards draw files with.

use std::collections::BTreeMap;

use hegel::generators as gs;
use serde_json::json;
use tau_agent::tool::RunId;
use tau_ui_plugin::tree::Node;
use tau_vcs::ui::{Card, change_diff::ChangeDiff, diff_card, pick, pick_key};

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

/// A diff card's files are sections, closed until opened, each under
/// its own id and holding its hunks as a diff in its path's language.
#[test]
fn a_diffs_files_are_sections_of_their_hunks() {
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
    let Node::Stack { children } = diff_card::files_body(&card, &diff) else {
        panic!("a stack of files");
    };
    let sections: Vec<_> = children
        .iter()
        .map(|node| match node {
            Node::Section {
                id, open, children, ..
            } => (id.clone(), *open, children[0].clone()),
            other => panic!("a file's section: {other:?}"),
        })
        .collect();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].0, card.file_id("src/a.rs"));
    assert_ne!(sections[0].0, sections[1].0);
    assert!(sections.iter().all(|(_, open, _)| !open));
    let Node::Diff { text, path } = &sections[0].2 else {
        panic!("the hunks as a diff: {:?}", sections[0].2);
    };
    assert_eq!(path.as_deref(), Some("src/a.rs"));
    assert_eq!(text, "@@ -1,2 +1,2 @@\n keep\n-old\n+new\n");
    // A file with no hunks says so in place of a diff.
    assert_eq!(sections[1].2, Node::text("No lines changed."));
}
