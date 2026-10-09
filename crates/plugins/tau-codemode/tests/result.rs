//! How a script's output reaches the model and the card: cut to a token
//! budget with the full text kept, and cut again for the card's details.
//!
//! Properties: under budget nothing changes; over it the text merges into
//! one item, the images follow in order and the full text is kept;
//! `preview` never exceeds its width; the card's details keep every item,
//! keep what fits whole, and cut the rest at character boundaries.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hegel::{
    TestCase,
    generators::{self as gs, Generator as _, PrintableGenerator},
};
use serde_json::{Value, json};
use tau_agent::output::estimate_tokens;
use tau_codemode::{
    image::Image,
    result::{
        CUT_HEAD_BYTES,
        CallStatus,
        Item,
        MAX_OUTPUT_DETAIL_BYTES,
        budget,
        output_details,
        preview,
    },
};

/// Text with multi-byte characters, so cuts can land inside one.
fn text(max: usize) -> impl PrintableGenerator<String> {
    gs::text().max_size(max)
}

#[hegel::composite]
fn image(tc: &TestCase) -> Image {
    let bytes = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(20));
    Image {
        mime_type: "image/png",
        data: STANDARD.encode(bytes),
    }
}

#[hegel::composite]
fn item(tc: &TestCase, max: usize) -> Item {
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => Item::Text(tc.draw(text(max))),
        1 => Item::Json(json!({"k": tc.draw(text(max))}).to_string()),
        _ => Item::Image(tc.draw(image().print_as_debug())),
    }
}

fn tokens(items: &[Item]) -> u64 {
    items
        .iter()
        .filter_map(Item::text)
        .map(estimate_tokens)
        .sum()
}

fn kept(_: &str) -> std::io::Result<std::path::PathBuf> {
    Ok("/kept/output.txt".into())
}

/// Within the budget the items are returned as they came, JSON, images
/// and order included, and nothing is saved.
#[hegel::test(test_cases = 300)]
fn output_within_the_budget_is_untouched(tc: TestCase) {
    let items = tc.draw(gs::vecs(item(40).print_as_debug()).max_size(6));
    let max = tokens(&items) + tc.draw(gs::integers::<u64>().max_value(5));
    let out = budget(items.clone(), max, |_| panic!("nothing to save"));
    assert_eq!(out, items);
}

/// Past the budget the text becomes one item that says it was cut and
/// where the whole of it went; images follow it in order; the text
/// handed to `save` is every text item joined by newlines.
#[hegel::test(test_cases = 300)]
fn output_past_the_budget_is_one_text_then_the_images(tc: TestCase) {
    let items =
        tc.draw(gs::vecs(item(200).print_as_debug()).min_size(1).max_size(6));
    let total = tokens(&items);
    tc.assume(total > 0);
    let max = tc.draw(gs::integers::<u64>().max_value(total - 1));
    let saved = std::cell::RefCell::new(None);
    let out = budget(items.clone(), max, |full| {
        *saved.borrow_mut() = Some(full.to_owned());
        kept(full)
    });

    let joined = items
        .iter()
        .filter_map(Item::text)
        .collect::<Vec<_>>()
        .join("\n");
    let [Item::Text(shown), images @ ..] = out.as_slice() else {
        panic!("one text first: {out:?}");
    };
    // Each item's tokens round up, so the sum can pass a budget that the
    // joined text fits: then the items merge whole and nothing is saved.
    if estimate_tokens(&joined) <= max {
        assert_eq!((shown, saved.borrow().as_ref()), (&joined, None));
    } else {
        assert!(shown.starts_with("Warning: truncated output"), "{shown}");
        assert!(shown.contains("/kept/output.txt"));
        assert_eq!(saved.borrow().as_ref(), Some(&joined));
    }
    let want: Vec<&Item> = items
        .iter()
        .filter(|item| matches!(item, Item::Image(_)))
        .collect();
    assert_eq!(images.iter().collect::<Vec<_>>(), want);
}

/// `preview` is the text itself when it fits, and otherwise its start
/// with `…`, never longer than the width.
#[hegel::test(test_cases = 500)]
fn a_preview_never_exceeds_its_width(tc: TestCase) {
    let text = tc.draw(text(60));
    let max = tc.draw(gs::integers::<usize>().max_value(70));
    let shown = preview(&text, max);
    assert!(shown.chars().count() <= max, "{shown:?} in {max}");
    if text.chars().count() <= max {
        assert_eq!(shown, text);
    } else if let Some(start) = shown.strip_suffix('…') {
        assert!(text.starts_with(start));
        assert_eq!(start.chars().count(), max - 1);
    } else {
        assert_eq!((max, shown.as_str()), (0, ""));
    }
}

/// The card keeps one detail per item. Items stay whole while the
/// total fits [`MAX_OUTPUT_DETAIL_BYTES`]; from the first that does not
/// fit on, text keeps only a head, cut on a character boundary, with
/// its true size. Images report the size they decode to.
#[hegel::test(test_cases = 100)]
fn output_details_keep_what_fits_and_cut_the_rest(tc: TestCase) {
    let big = || {
        gs::vecs(gs::sampled_from(vec!["a", "é", "雪", "😀"]))
            .min_size(1)
            .max_size(60_000)
            .map(|chars| chars.concat())
    };
    let items = tc.draw(
        gs::vecs(hegel::one_of!(
            big().map(Item::Text).print_as_debug(),
            item(50).print_as_debug(),
        ))
        .max_size(8),
    );
    let details = output_details(&items);
    assert_eq!(details.len(), items.len());

    let mut left = MAX_OUTPUT_DETAIL_BYTES;
    let mut cutting = false;
    for (item, detail) in items.iter().zip(&details) {
        match item {
            Item::Image(image) => {
                assert_eq!(detail["kind"], "image");
                let size = STANDARD.decode(&image.data).unwrap().len();
                assert_eq!(detail["bytes"], size);
            }
            Item::Text(text) | Item::Json(text) => {
                cutting |= text.len() > left;
                if cutting {
                    let head = detail["text"].as_str().expect("a head");
                    assert!(text.starts_with(head));
                    assert!(head.len() <= CUT_HEAD_BYTES);
                    assert!(
                        head.len() + 4 > CUT_HEAD_BYTES.min(text.len()),
                        "the head is as long as a boundary allows"
                    );
                    assert_eq!(detail["bytes"], text.len());
                    assert_eq!(detail["cut"], true);
                    left = 0;
                } else {
                    left -= text.len();
                    assert!(detail.get("cut").is_none());
                    match item {
                        Item::Text(text) => assert_eq!(detail["text"], *text),
                        _ => assert_eq!(
                            detail["value"],
                            serde_json::from_str::<Value>(text).unwrap()
                        ),
                    }
                }
            }
        }
    }
}

/// A call status reads back from the word it prints as, and no other
/// word reads as one.
#[hegel::test(test_cases = 100)]
fn a_call_status_reads_back_from_its_word(tc: TestCase) {
    for status in [
        CallStatus::Running,
        CallStatus::Ok,
        CallStatus::Error,
        CallStatus::Cancelled,
    ] {
        assert_eq!(CallStatus::parse(status.as_str()), Some(status));
    }
    let word = tc.draw(text(10));
    if let Some(status) = CallStatus::parse(&word) {
        assert_eq!(status.as_str(), word);
    }
}
