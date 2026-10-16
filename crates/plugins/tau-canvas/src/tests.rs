use hegel::generators as gs;

use super::*;

fn document(call: &str, item: u32, text: &str) -> Record {
    Record::Revised {
        call: call.to_owned(),
        item,
        title: format!("Doc {item}"),
        content: Content::Document {
            text: text.to_owned(),
        },
    }
}

fn edit(old: &str, new: &str) -> Edit {
    Edit {
        old: old.to_owned(),
        new: new.to_owned(),
    }
}

#[test]
fn edits_refuse_what_is_missing_or_ambiguous() {
    let text = "- one\n- one\n- two";
    assert!(apply_edits(text, &[]).unwrap_err().contains("No edits"));
    assert!(
        apply_edits(text, &[edit("", "x")])
            .unwrap_err()
            .contains("empty")
    );
    assert!(
        apply_edits(text, &[edit("three", "x")])
            .unwrap_err()
            .contains("does not occur")
    );
    let ambiguous =
        apply_edits(text, &[edit("two", "2"), edit("one", "1")]).unwrap_err();
    assert!(ambiguous.starts_with("Edit 2:"), "{ambiguous}");
    assert!(ambiguous.contains("more than once"));
}

/// Markers each edit finds once: the text around them, and what the
/// edits put in, never holds one.
const MARKERS: [&str; 4] = ["§", "¤", "¶", "†"];

// Property: one to four edits, given in any order, each of a marker
// that occurs exactly once, replace each marker in place, and nothing
// else in the text changes. Each edit reads the text the ones before it
// left: an edit of what an earlier one wrote finds it.
#[hegel::test]
fn edits_replace_only_their_text_in_order(tc: hegel::TestCase) {
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let plain =
        |size: usize| gs::text().exclude_characters("§¤¶†").max_size(size);
    let segments: Vec<String> =
        (0..=count).map(|_| tc.draw(plain(20))).collect();
    let news: Vec<String> = (0..count).map(|_| tc.draw(plain(10))).collect();
    // The order the edits are given in, shuffled.
    let mut order: Vec<usize> = (0..count).collect();
    for at in (1..count).rev() {
        let other = tc.draw(gs::integers::<usize>().max_value(at));
        order.swap(at, other);
    }
    let mut text = segments[0].clone();
    let mut expected = segments[0].clone();
    for n in 0..count {
        text.push_str(MARKERS[n]);
        text.push_str(&segments[n + 1]);
        expected.push_str(&news[n]);
        expected.push_str(&segments[n + 1]);
    }
    let edits: Vec<Edit> =
        order.iter().map(|&n| edit(MARKERS[n], &news[n])).collect();
    assert_eq!(apply_edits(&text, &edits), Ok(expected));

    // A later edit finds what an earlier one wrote: a marker of its own
    // keeps it the one place it occurs.
    let written = format!("†{}", tc.draw(plain(8)));
    assert_eq!(
        apply_edits(
            &format!("§ {}", segments[0]),
            &[edit("§", &written), edit(&format!("{written} "), "done ")]
        ),
        Ok(format!("done {}", segments[0]))
    );
}

#[test]
fn records_of_other_plugins_are_skipped() {
    let bodies = vec![
        serde_json::to_value(document("a", 1, "kept")).unwrap(),
        serde_json::json!({ "kind": "asked", "call": "x" }),
    ];
    let canvas = Canvas::from_records(&bodies);
    assert_eq!(canvas.items.len(), 1);
}

/// A revision as the plugin writes one: of a document, or of an image
/// with or without a size.
#[hegel::composite]
fn record(tc: &hegel::TestCase) -> Record {
    let content = if tc.draw(gs::booleans()) {
        Content::Document {
            text: tc.draw(gs::text().max_size(40)),
        }
    } else {
        let size = tc.draw(gs::optional(hegel::tuples!(
            gs::integers::<u32>().min_value(1),
            gs::integers::<u32>().min_value(1),
        )));
        Content::Image {
            image: ImageRef {
                blob: Blob {
                    id: tc
                        .draw(gs::from_regex("[0-9a-f-]{36}").fullmatch(true)),
                    digest: tc
                        .draw(gs::from_regex("[0-9a-f]{64}").fullmatch(true)),
                    size_bytes: tc
                        .draw(gs::integers::<u64>().max_value(MAX_IMAGE_BYTES)),
                },
                mime: tc
                    .draw(gs::sampled_from(vec![
                        "image/png",
                        "image/svg+xml",
                        "image/jpeg",
                    ]))
                    .to_owned(),
                width: size.map(|(width, _)| width),
                height: size.map(|(_, height)| height),
                path: tc.draw(gs::text().max_size(20)),
            },
            caption: tc.draw(gs::text().max_size(20)),
        }
    };
    Record::Revised {
        call: tc.draw(gs::text().min_size(1).max_size(12)),
        item: tc.draw(gs::integers::<u32>().min_value(1)),
        title: tc.draw(gs::text().max_size(20)),
        content,
    }
}

/// Any revision reads back as it was written, tagged as a revision and
/// by what it holds, as the store keeps it.
#[hegel::test(test_cases = 300)]
fn a_record_reads_back_as_it_was_written(tc: hegel::TestCase) {
    use hegel::generators::Generator as _;
    // Record is tau's own type: it prints through Debug.
    let record = tc.draw(record().print_as_debug());
    let body = serde_json::to_value(&record).unwrap();
    let Record::Revised { content, .. } = &record;
    assert_eq!(body["kind"], "revised");
    let tag = match content {
        Content::Document { .. } => "document",
        Content::Image { .. } => "image",
    };
    assert_eq!(body["content"]["type"], tag);
    assert_eq!(Record::parse(&body), Some(record));
}

// Property: folding any sequence of revisions keeps the items in the
// order they were first named, gives each one revision per record that
// named it, in order, shows the last one, and finds each call's item and
// revision, and no other call's. The model is a plain list of (item,
// call) pairs.
#[hegel::test]
fn the_canvas_keeps_every_revision_in_order(tc: hegel::TestCase) {
    let items: Vec<u32> = tc.draw(
        gs::vecs(gs::integers::<u32>().min_value(1).max_value(4)).max_size(24),
    );
    let mut canvas = Canvas::default();
    for (n, item) in items.iter().enumerate() {
        canvas.revise(document(&format!("call-{n}"), *item, &n.to_string()));
    }
    let mut first_seen: Vec<u32> = Vec::new();
    for item in &items {
        if !first_seen.contains(item) {
            first_seen.push(*item);
        }
    }
    assert_eq!(
        canvas.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        first_seen
    );
    for found in &canvas.items {
        let named: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| **item == found.id)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            found
                .revisions
                .iter()
                .map(|revision| revision.call.clone())
                .collect::<Vec<_>>(),
            named
                .iter()
                .map(|n| format!("call-{n}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            found.latest().content,
            Content::Document {
                text: named.last().unwrap().to_string()
            }
        );
    }
    for (n, item) in items.iter().enumerate() {
        let (found, revision) = canvas.by_call(&format!("call-{n}")).unwrap();
        assert_eq!(found.id, *item);
        let earlier = items[..n].iter().filter(|other| *other == item).count();
        assert_eq!(revision, earlier, "call-{n}");
    }
    assert!(canvas.by_call("missing").is_none());
    assert_eq!(canvas.fresh, items.len() as u64);
    assert_eq!(canvas.last, items.last().copied());
    assert_eq!(
        canvas.next_id(),
        items.iter().max().copied().unwrap_or(0) + 1
    );
}
