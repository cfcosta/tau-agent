//! The history, the tree and the view (`docs/reference/tree-compaction.md`,
//! "The view"), without a model.
//!
//! Property inventory:
//! - node names round-trip through `zoom`'s `(id, n)`, and children tile
//!   their parent (algebraic);
//! - `fit` leaves a view that tiles the history in order and within its
//!   budget, or one with no built merge left (oracle: a scan of the
//!   view);
//! - appending a message and fitting again never splits a line: every
//!   old line lies inside one new line (stateful, OptChat's "never
//!   split");
//! - building the lines `wanted` names makes the view fit whenever it
//!   can (differential: `wanted` simulates `fit`);
//! - zooming from any line reaches every message under it, whole;
//! - `restore` gives back what was kept, and `cap` keeps a long text's
//!   head and tail.

use hegel::{TestCase, generators as gs};
use serde_json::{Map, json};
use tau_ai::message::{
    AssistantBlock,
    AssistantMessage,
    ImageContent,
    InputBlock,
    Message,
    StopReason,
    TextContent,
    ThinkingContent,
    ToolCall,
    ToolResultMessage,
    Usage,
    UserContent,
    UserMessage,
};
use tau_tree_compaction::tree::{
    ENTRY_CHARS,
    Entry,
    Fit,
    History,
    Kind,
    NODE_BYTES,
    Node,
    cap,
    entries,
};

/// A text of `bytes` ASCII bytes, starting with `tag` so lines differ.
fn sized(tag: &str, bytes: usize) -> String {
    let mut text = format!("{tag}:");
    while text.len() < bytes {
        text.push('x');
    }
    text.truncate(bytes.max(1));
    text
}

/// A history of `lengths.len()` entries whose leaves are built, a leaf's
/// line `lengths[i]` bytes.
fn leaves(lengths: &[usize]) -> History {
    let mut history = History::new();
    for (id, bytes) in lengths.iter().enumerate() {
        history.push(Entry::new(Kind::Echo, sized(&format!("e{id}"), 2_000)));
        history.set(Node::leaf(id), sized(&format!("l{id}"), *bytes));
    }
    history
}

/// Every parent over `history`'s entries that `build` gives a size,
/// built bottom up, `build` drawing its size from its children's.
fn build_parents(
    history: &mut History,
    mut build: impl FnMut(Node, usize, usize) -> Option<usize>,
) {
    let total = history.len();
    let mut level = 1;
    while (1usize << level) <= total {
        for index in 0..total >> level {
            let node = Node::new(level, index);
            let (a, b) = node.children().unwrap();
            let (Some(a), Some(b)) = (history.text(a), history.text(b)) else {
                continue;
            };
            if let Some(bytes) = build(node, a.len(), b.len()) {
                history.set(node, sized(&node.label(), bytes));
            }
        }
        level += 1;
    }
}

/// Whether `view` tiles `[0, total)` in order.
fn tiles(view: &[Node], total: usize) -> bool {
    view.iter()
        .try_fold(0, |at, node| (node.first() == at).then(|| node.end()))
        == Some(total)
}

/// Whether some two siblings sit side by side with their parent built.
fn mergeable(history: &History) -> bool {
    history.view().windows(2).any(|pair| {
        pair[0].level == pair[1].level
            && pair[0].index % 2 == 0
            && pair[1].index == pair[0].index + 1
            && history.is_built(pair[0].parent())
    })
}

#[hegel::composite]
fn lengths(tc: &TestCase, most: usize) -> Vec<usize> {
    tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(1).max_value(NODE_BYTES))
            .min_size(most / 8 + 1)
            .max_size(most),
    )
}

#[hegel::test]
fn node_names_round_trip_and_children_tile_their_parent(tc: TestCase) {
    let level = tc.draw(gs::integers::<u32>().max_value(12));
    let index = tc.draw(gs::integers::<usize>().max_value(10_000));
    let node = Node::new(level, index);
    assert_eq!(Node::named(node.first(), node.count()), Some(node));
    assert_eq!(node.label(), format!("{}+{}", node.first(), node.count()));
    match node.children() {
        None => assert_eq!(level, 0),
        Some((a, b)) => {
            assert_eq!(
                (a.first(), a.end(), b.end()),
                (node.first(), b.first(), node.end())
            );
            assert_eq!((a.parent(), b.parent()), (node, node));
        }
    }
    // A count that is not a power of two, or an id it does not divide,
    // names nothing.
    let n = tc.draw(gs::integers::<usize>().min_value(2).max_value(1 << 12));
    let id = tc.draw(gs::integers::<usize>().max_value(1 << 16));
    let named = Node::named(id, n);
    assert_eq!(named.is_some(), n.is_power_of_two() && id.is_multiple_of(n));
}

#[hegel::test]
fn fit_tiles_the_history_within_budget_or_waits_for_a_line(tc: TestCase) {
    let lengths = tc.draw(lengths(300));
    let mut history = leaves(&lengths);
    // Some parents built, at sizes their children allow.
    let built: Vec<bool> =
        tc.draw(gs::vecs(gs::booleans()).min_size(600).max_size(600));
    let mut n = 0;
    build_parents(&mut history, |_, a, b| {
        n += 1;
        built[n % built.len()].then_some((a + 1 + b).min(NODE_BYTES))
    });
    let budget = tc.draw(gs::integers::<usize>().max_value(80_000));
    let fit = history.fit(budget);
    assert!(tiles(history.view(), history.len()));
    match fit {
        Fit::Fits => assert!(history.view_bytes(NODE_BYTES) <= budget),
        Fit::Waiting => {
            assert!(history.view_bytes(NODE_BYTES) > budget);
            assert!(!mergeable(&history));
        }
    }
}

#[hegel::test]
fn appending_and_fitting_never_splits_a_line(tc: TestCase) {
    let lengths = tc.draw(lengths(200));
    let budget = tc.draw(
        gs::integers::<usize>()
            .min_value(NODE_BYTES)
            .max_value(20_000),
    );
    let mut history = History::new();
    for (id, bytes) in lengths.iter().enumerate() {
        let before = history.view().to_vec();
        history.push(Entry::new(Kind::User, format!("m{id}")));
        history.set(Node::leaf(id), sized(&format!("l{id}"), *bytes));
        // Every parent is set again each step, at the size its
        // children give it.
        build_parents(&mut history, |_, a, b| {
            Some((a + 1 + b).min(NODE_BYTES))
        });
        history.fit(budget);
        let after = history.view();
        assert!(tiles(after, id + 1));
        for old in before {
            assert!(
                after
                    .iter()
                    .any(|new| new.first() <= old.first()
                        && old.end() <= new.end()),
                "{} was split",
                old.label()
            );
        }
    }
}

#[hegel::test]
fn building_the_wanted_lines_makes_the_view_fit(tc: TestCase) {
    let lengths = tc.draw(lengths(400));
    let mut history = leaves(&lengths);
    // Enough for the coarsest view: one line per set bit of the count.
    let floor = NODE_BYTES * lengths.len().count_ones() as usize;
    let budget =
        tc.draw(gs::integers::<usize>().min_value(floor).max_value(200_000));
    let wanted = history.wanted(budget);
    for node in &wanted {
        let (a, b) = node.children().unwrap();
        let (a, b) = (
            history.text(a).unwrap().len(),
            history.text(b).unwrap().len(),
        );
        history.set(*node, sized(&node.label(), (a + 1 + b).min(NODE_BYTES)));
    }
    assert_eq!(history.fit(budget), Fit::Fits);
    // Only what was wanted was merged.
    assert!(
        history
            .view()
            .iter()
            .all(|node| node.level == 0 || wanted.contains(node))
    );
}

#[hegel::test]
fn zoom_reaches_every_message_under_a_line(tc: TestCase) {
    let lengths = tc.draw(lengths(64));
    let mut history = leaves(&lengths);
    build_parents(&mut history, |_, a, b| Some((a + 1 + b).min(NODE_BYTES)));
    let budget = tc.draw(gs::integers::<usize>().max_value(8_000));
    history.fit(budget);
    let view = history.view().to_vec();
    for line in view {
        let mut open = vec![line];
        let mut reached = Vec::new();
        while let Some(node) = open.pop() {
            let shown = history.zoom(node.first(), node.count()).unwrap();
            match node.children() {
                None => {
                    let entry = &history.entries()[node.first()];
                    assert_eq!(
                        shown,
                        format!("{}+1|{}", node.first(), entry.line())
                    );
                    reached.push(node.first());
                }
                Some((a, b)) => {
                    let lines: Vec<&str> = shown.lines().collect();
                    assert_eq!(lines.len(), 2);
                    assert!(lines[0].starts_with(&format!("{}|", a.label())));
                    assert!(lines[1].starts_with(&format!("{}|", b.label())));
                    open.extend([b, a]);
                }
            }
        }
        reached.sort_unstable();
        assert_eq!(reached, (line.first()..line.end()).collect::<Vec<_>>());
    }
    let total = history.len();
    assert_eq!(history.zoom(total, 1), Err(format!("No line {total}+1.")));
    assert_eq!(history.zoom(0, 3), Err("No line 0+3.".to_owned()));
    assert_eq!(history.zoom(1, 2), Err("No line 1+2.".to_owned()));
}

#[hegel::test]
fn render_is_one_labelled_line_per_part(tc: TestCase) {
    let lengths = tc.draw(lengths(100));
    let mut history = leaves(&lengths);
    history.set(Node::leaf(0), "first\nline  with\tspaces".to_owned());
    build_parents(&mut history, |_, a, b| Some((a + 1 + b).min(NODE_BYTES)));
    history.fit(tc.draw(gs::integers::<usize>().max_value(10_000)));
    let rendered = history.render();
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), history.view().len());
    for (line, node) in lines.iter().zip(history.view()) {
        let text = line
            .strip_prefix(&format!("{}|", node.label()))
            .expect("labelled");
        assert_eq!(text, history.flat(*node));
    }
    if history.view()[0] == Node::leaf(0) {
        assert_eq!(lines[0], "0+1|first line with spaces");
    }
    // The context a compactor reads has no labels.
    assert!(!history.context(history.len()).contains("+1|"));
}

#[hegel::test]
fn restore_gives_back_what_was_kept(tc: TestCase) {
    let lengths = tc.draw(lengths(100));
    let mut history = leaves(&lengths);
    build_parents(&mut history, |_, a, b| Some((a + 1 + b).min(NODE_BYTES)));
    history.fit(tc.draw(gs::integers::<usize>().max_value(10_000)));
    let nodes: Vec<(Node, String)> = history
        .nodes()
        .map(|(node, text)| (node, text.to_owned()))
        .collect();
    let restored = History::restore(
        history.entries().to_vec(),
        nodes.clone(),
        history.view().to_vec(),
    );
    assert_eq!(restored, history);
    // A view that does not tile what was kept falls back to the leaves.
    let short = History::restore(
        history.entries()[..history.len() - 1].to_vec(),
        nodes,
        history.view().to_vec(),
    );
    assert!(tiles(short.view(), short.len()));
    assert!(short.view().iter().all(|node| node.level == 0));
}

#[hegel::test]
fn cap_keeps_a_long_texts_head_and_tail(tc: TestCase) {
    let len = tc.draw(gs::integers::<usize>().max_value(3 * ENTRY_CHARS));
    let text: String = (0..len)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    let capped = cap(&text);
    if len <= ENTRY_CHARS {
        assert_eq!(capped, text);
    } else {
        let half = ENTRY_CHARS / 2;
        assert!(capped.starts_with(&text[..half]));
        assert!(capped.ends_with(&text[len - half..]));
        assert!(
            capped
                .contains(&format!("[… {} characters cut …]", len - 2 * half))
        );
    }
}

fn assistant(content: Vec<AssistantBlock>) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        model: "m".into(),
        response_id: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    })
}

/// A message's parts, kind by kind: an assistant message's text, then
/// each call, its thinking left out; a tool result named by its tool;
/// nothing for an empty message.
#[test]
fn messages_split_into_entries_by_kind() {
    let user = Message::User(UserMessage {
        content: UserContent::Blocks(vec![
            InputBlock::Text(TextContent {
                text: "look at this".into(),
                text_signature: None,
            }),
            InputBlock::Image(ImageContent {
                data: String::new(),
                mime_type: "image/png".into(),
            }),
        ]),
        timestamp: 0,
    });
    assert_eq!(
        entries(&user),
        [Entry::new(Kind::User, "look at this\n[image]")]
    );
    let blank = Message::User(UserMessage {
        content: UserContent::Text("  ".into()),
        timestamp: 0,
    });
    assert!(entries(&blank).is_empty());
    let reply = assistant(vec![
        AssistantBlock::Thinking(ThinkingContent {
            thinking: "hmm".into(),
            thinking_signature: None,
            redacted: None,
        }),
        AssistantBlock::Text(TextContent {
            text: "reading both".into(),
            text_signature: None,
        }),
        AssistantBlock::ToolCall(ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
        }),
        AssistantBlock::ToolCall(ToolCall {
            id: "c2".into(),
            name: "bash".into(),
            arguments: Map::from_iter([("command".into(), json!("ls"))]),
        }),
    ]);
    assert_eq!(
        entries(&reply),
        [
            Entry::new(Kind::Talk, "reading both"),
            Entry::new(Kind::Tool, r#"read {"path":"a.rs"}"#),
            Entry::new(Kind::Tool, r#"bash {"command":"ls"}"#),
        ]
    );
    let result = Message::ToolResult(ToolResultMessage {
        tool_call_id: "c1".into(),
        tool_name: "read".into(),
        content: vec![InputBlock::Text(TextContent {
            text: "no such file".into(),
            text_signature: None,
        })],
        details: None,
        is_error: true,
        timestamp: 0,
    });
    assert_eq!(
        entries(&result),
        [Entry::new(Kind::Echo, "[read failed] no such file")]
    );
}

/// A short entry is its own line, word for word; a long one needs a
/// model. Two lines that fit together are their parent.
#[test]
fn short_lines_need_no_model() {
    let mut history = History::new();
    history.push(Entry::new(Kind::User, "keep the AST shape"));
    history.push(Entry::new(Kind::Echo, "x".repeat(NODE_BYTES)));
    assert_eq!(
        history.free(Node::leaf(0)).as_deref(),
        Some("user: keep the AST shape")
    );
    assert_eq!(history.free(Node::leaf(1)), None);
    history.set(Node::leaf(0), "user: keep the AST shape".into());
    history.set(Node::leaf(1), "echo: 512 x".into());
    assert_eq!(
        history.free(Node::new(1, 0)).as_deref(),
        Some("user: keep the AST shape\necho: 512 x")
    );
    history.set(Node::leaf(1), "y".repeat(NODE_BYTES));
    assert_eq!(history.free(Node::new(1, 0)), None);
}

/// OptChat's rule, worked by hand: eight lines of 100 bytes, every
/// parent 100 bytes, a 500-byte budget. The pair at 0 is most due (8/4),
/// then the one at 2 (6/4); then the pair of level-1 lines at 0 (8/8)
/// ties the leaves at 4 (4/4), and the older pair wins.
#[test]
fn the_most_due_pair_merges_first_and_ties_go_to_the_older() {
    let mut history = leaves(&[100; 8]);
    build_parents(&mut history, |_, _, _| Some(100));
    assert_eq!(history.fit(700), Fit::Fits);
    assert_eq!(history.view()[0], Node::new(1, 0));
    assert_eq!(history.fit(600), Fit::Fits);
    assert_eq!(&history.view()[..2], [Node::new(1, 0), Node::new(1, 1)]);
    assert_eq!(history.fit(500), Fit::Fits);
    assert_eq!(
        history.view(),
        [
            Node::new(2, 0),
            Node::leaf(4),
            Node::leaf(5),
            Node::leaf(6),
            Node::leaf(7)
        ]
    );
}

/// Detail fades with age: grown one message at a time, every merge
/// available, a line never covers more than the lines before it.
#[hegel::test]
fn older_lines_never_cover_less_than_newer_ones(tc: TestCase) {
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(400));
    let bytes =
        tc.draw(gs::integers::<usize>().min_value(40).max_value(NODE_BYTES));
    let budget = tc.draw(
        gs::integers::<usize>()
            .min_value(NODE_BYTES * 10)
            .max_value(40_000),
    );
    let mut history = History::new();
    for id in 0..count {
        history.push(Entry::new(Kind::User, format!("m{id}")));
        history.set(Node::leaf(id), sized(&format!("l{id}"), bytes));
        build_parents(&mut history, |_, _, _| Some(bytes));
        assert_eq!(history.fit(budget), Fit::Fits);
        let levels: Vec<u32> =
            history.view().iter().map(|node| node.level).collect();
        assert!(
            levels.windows(2).all(|pair| pair[0] >= pair[1]),
            "{levels:?}"
        );
    }
}
