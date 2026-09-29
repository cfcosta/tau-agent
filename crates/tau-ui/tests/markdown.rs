//! Replies read as blocks: what is written as a table reads back as that
//! table, links keep where they go, and code stays code.

use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_ui::markdown::{Align, Block, Span, blocks, plain};

/// A cell as written, and the text it shows: plain, marked, empty, and
/// pipes, escaped as GitHub's tables need even inside code.
fn cell(tc: &TestCase) -> (String, String) {
    tc.draw(gs::sampled_from(vec![
        (String::new(), String::new()),
        ("a".into(), "a".into()),
        ("two words".into(), "two words".into()),
        ("**bold**".into(), "bold".into()),
        ("*slanted*".into(), "slanted".into()),
        ("`code`".into(), "code".into()),
        ("`x\\|y`".into(), "x|y".into()),
        ("a \\| b".into(), "a | b".into()),
        ("[docs](https://example.com/a_b)".into(), "docs".into()),
    ]))
}

/// What a block shows, without styles: enough to compare.
#[derive(Debug, Clone, PartialEq, hegel::PrettyPrintable)]
enum Shape {
    Paragraph(String),
    Table {
        // Our own library type: printed through Debug.
        #[pretty(debug)]
        align: Vec<Align>,
        head: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

fn shape(block: &Block) -> Shape {
    match block {
        Block::Paragraph(inline) => Shape::Paragraph(plain(inline)),
        Block::Table { align, head, rows } => Shape::Table {
            align: align.clone(),
            head: head.iter().map(|cell| plain(cell)).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|cell| plain(cell)).collect())
                .collect(),
        },
        other => panic!("unexpected {other:?}"),
    }
}

/// A table as written, and what it shows.
#[hegel::composite]
fn table(tc: &TestCase) -> (String, Shape) {
    let width = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let align: Vec<Align> = (0..width)
        .map(|_| {
            tc.draw(
                gs::sampled_from(vec![
                    Align::Left,
                    Align::Center,
                    Align::Right,
                ])
                .print_as_debug(),
            )
        })
        .collect();
    let row = |tc: &TestCase| -> Vec<(String, String)> {
        (0..width).map(|_| cell(tc)).collect()
    };
    let head = row(tc);
    let count = tc.draw(gs::integers::<usize>().max_value(3));
    let rows: Vec<Vec<(String, String)>> =
        (0..count).map(|_| row(tc)).collect();
    let line = |cells: &[(String, String)]| {
        let written: Vec<&str> = cells.iter().map(|c| c.0.as_str()).collect();
        format!("| {} |", written.join(" | "))
    };
    let delimiter: Vec<&str> = align
        .iter()
        .map(|align| match align {
            Align::Left => "---",
            Align::Center => ":-:",
            Align::Right => "--:",
        })
        .collect();
    let mut lines = vec![line(&head), format!("|{}|", delimiter.join("|"))];
    lines.extend(rows.iter().map(|cells| line(cells)));
    let shows = |cells: &[(String, String)]| {
        cells.iter().map(|c| c.1.clone()).collect::<Vec<_>>()
    };
    let shape = Shape::Table {
        align,
        head: shows(&head),
        rows: rows.iter().map(|cells| shows(cells)).collect(),
    };
    (lines.join("\n"), shape)
}

/// A paragraph of words, some over a soft line break, which reads as a
/// space.
#[hegel::composite]
fn paragraph(tc: &TestCase) -> (String, Shape) {
    let lines = tc.draw(
        gs::vecs(gs::sampled_from(vec![
            "Some text.".to_owned(),
            "More words here".to_owned(),
            "and 25% of it".to_owned(),
        ]))
        .min_size(1)
        .max_size(3),
    );
    (lines.join("\n"), Shape::Paragraph(lines.join(" ")))
}

#[hegel::test(test_cases = 200)]
fn written_blocks_read_back(tc: TestCase) {
    let count = tc.draw(gs::integers::<usize>().max_value(5));
    let parts: Vec<(String, Shape)> = (0..count)
        .map(|_| {
            if tc.draw(gs::booleans()) {
                tc.draw(table())
            } else {
                tc.draw(paragraph())
            }
        })
        .collect();
    let text: Vec<&str> = parts.iter().map(|(text, _)| text.as_str()).collect();
    let expected: Vec<Shape> = parts.iter().map(|(_, s)| s.clone()).collect();
    let read: Vec<Shape> =
        blocks(&text.join("\n\n")).iter().map(shape).collect();
    assert_eq!(read, expected);
}

#[test]
fn a_link_keeps_where_it_goes() {
    let url = "https://github.com/joelhooks/pi-fast-jev-compaction/tree/\
               eb83f533f4fd10a08728b02c062c249afda5a4dc";
    let text =
        format!("I compared the repository at [eb83f53]({url}) with ours.");
    let read = blocks(&text);
    let [Block::Paragraph(inline)] = read.as_slice() else {
        panic!("one paragraph: {read:?}");
    };
    assert_eq!(
        inline.as_slice(),
        [
            Span::plain("I compared the repository at "),
            Span {
                link: Some(url.into()),
                ..Span::plain("eb83f53")
            },
            Span::plain(" with ours."),
        ]
    );
}

#[test]
fn a_table_in_a_code_fence_stays_code() {
    assert_eq!(
        blocks("```rust\n| a | b |\n|---|---|\n```"),
        [Block::Code {
            lang: Some("rust".into()),
            text: "| a | b |\n|---|---|".into(),
        }]
    );
}

#[test]
fn a_table_streaming_in_is_a_paragraph_until_its_delimiter() {
    assert!(matches!(
        blocks("| a | b |").as_slice(),
        [Block::Paragraph(_)]
    ));
    assert!(matches!(
        blocks("| a | b |\n|---|---|").as_slice(),
        [Block::Table { .. }]
    ));
}

#[test]
fn lists_keep_their_marks_and_numbers() {
    let bold = Span {
        bold: true,
        ..Span::plain("a")
    };
    assert_eq!(
        blocks("- **a** b\n- c\n\n3. d"),
        [
            Block::List {
                start: None,
                items: vec![
                    vec![Block::Paragraph(vec![bold, Span::plain(" b")])],
                    vec![Block::Paragraph(vec![Span::plain("c")])],
                ],
            },
            Block::List {
                start: Some(3),
                items: vec![vec![Block::Paragraph(vec![Span::plain("d")])]],
            },
        ]
    );
}

#[test]
fn a_hard_break_is_a_new_line() {
    assert_eq!(
        blocks("one\\\ntwo"),
        [Block::Paragraph(vec![Span::plain("one\ntwo")])]
    );
}

#[test]
fn nothing_reads_as_nothing() {
    assert_eq!(blocks(""), []);
}
