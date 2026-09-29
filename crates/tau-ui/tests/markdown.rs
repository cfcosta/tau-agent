//! Replies split into prose and tables: whatever is written as a table
//! reads back as that table, and nothing else does.

use hegel::{TestCase, generators as gs};
use tau_ui::markdown::{Align, Block, blocks};

/// Cell text: plain, marked, empty, and pipes both escaped and inside
/// code, which must not split the cell.
fn cell(tc: &TestCase) -> String {
    tc.draw(gs::sampled_from(vec![
        String::new(),
        "a".into(),
        "two words".into(),
        "**bold**".into(),
        "`code`".into(),
        "`x|y`".into(),
        "a | b".into(),
        "42.5".into(),
    ]))
}

/// A cell as a model writes it: a pipe outside code escaped.
fn written(cell: &str) -> String {
    let mut out = String::new();
    let mut code = false;
    for c in cell.chars() {
        match c {
            '`' => {
                code = !code;
                out.push(c);
            }
            '|' if !code => out.push_str("\\|"),
            _ => out.push(c),
        }
    }
    out
}

fn row(cells: &[String]) -> String {
    let cells: Vec<String> = cells.iter().map(|cell| written(cell)).collect();
    format!("| {} |", cells.join(" | "))
}

#[hegel::composite]
fn table(tc: TestCase) -> Block {
    let width = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let align = (0..width)
        .map(|_| {
            tc.draw(gs::sampled_from(vec![
                Align::Left,
                Align::Center,
                Align::Right,
            ]))
        })
        .collect();
    let head = (0..width).map(|_| cell(&tc)).collect();
    let rows = tc.draw(gs::integers::<usize>().max_value(3));
    let rows = (0..rows)
        .map(|_| (0..width).map(|_| cell(&tc)).collect())
        .collect();
    Block::Table { align, head, rows }
}

/// Prose lines: no pipes and no fences, so none can start a table.
#[hegel::composite]
fn prose(tc: TestCase) -> Block {
    let lines = tc.draw(
        gs::vecs(gs::sampled_from(vec![
            String::new(),
            "Some text.".to_owned(),
            "- a list item".to_owned(),
            "# A heading".to_owned(),
            "---".to_owned(),
            "A line with `code` and **bold**.".to_owned(),
        ]))
        .min_size(1)
        .max_size(3),
    );
    Block::Prose(lines.join("\n"))
}

fn write(block: &Block) -> String {
    match block {
        Block::Prose(text) => text.clone(),
        Block::Table { align, head, rows } => {
            let delimiter: Vec<String> = align
                .iter()
                .map(|align| {
                    match align {
                        Align::Left => "---",
                        Align::Center => ":-:",
                        Align::Right => "--:",
                    }
                    .to_owned()
                })
                .collect();
            let mut lines =
                vec![row(head), format!("|{}|", delimiter.join("|"))];
            lines.extend(rows.iter().map(|cells| row(cells)));
            lines.join("\n")
        }
    }
}

#[hegel::test(test_cases = 200)]
fn written_tables_read_back(tc: TestCase) {
    let count = tc.draw(gs::integers::<usize>().max_value(5));
    let parts: Vec<Block> = (0..count)
        .map(|_| {
            if tc.draw(gs::booleans()) {
                tc.draw(table())
            } else {
                tc.draw(prose())
            }
        })
        .collect();
    // What the parser should give: adjacent prose joined, and a blank
    // line between two tables, or the second would be rows of the first.
    let mut expected: Vec<Block> = Vec::new();
    for part in parts {
        match (expected.last_mut(), &part) {
            (Some(Block::Prose(before)), Block::Prose(text)) => {
                before.push('\n');
                before.push_str(text);
            }
            (Some(Block::Table { .. }), Block::Table { .. }) => {
                expected.push(Block::Prose(String::new()));
                expected.push(part);
            }
            _ => expected.push(part),
        }
    }
    let text = expected.iter().map(write).collect::<Vec<_>>().join("\n");
    // Empty text is no blocks, not one empty paragraph.
    if text.is_empty() {
        expected.clear();
    }
    assert_eq!(blocks(&text), expected);
}

#[test]
fn a_table_in_a_code_fence_stays_code() {
    let text = "```\n| a | b |\n|---|---|\n| 1 | 2 |\n```";
    assert_eq!(blocks(text), [Block::Prose(text.into())]);
}

#[test]
fn a_table_streaming_in_is_prose_until_its_delimiter() {
    assert_eq!(blocks("| a | b |"), [Block::Prose("| a | b |".into())]);
    assert_eq!(
        blocks("| a | b |\n|---|---|"),
        [Block::Table {
            align: vec![Align::Left, Align::Left],
            head: vec!["a".into(), "b".into()],
            rows: vec![],
        }]
    );
}

#[test]
fn rows_take_the_heads_width_and_outer_pipes_are_optional() {
    assert_eq!(
        blocks("a | b\n:-- | --:\n| 1\n1 | 2 | 3"),
        [Block::Table {
            align: vec![Align::Left, Align::Right],
            head: vec!["a".into(), "b".into()],
            rows: vec![
                vec!["1".into(), String::new()],
                vec!["1".into(), "2".into()],
            ],
        }]
    );
}

#[test]
fn a_rule_under_a_line_is_not_a_table() {
    let text = "Title | subtitle\n---";
    assert_eq!(blocks(text), [Block::Prose(text.into())]);
}
