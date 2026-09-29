//! The block structure of a model's reply: prose, and the pipe tables
//! models write (GitHub's table extension). Inline marks inside either
//! are left to [`crate::ui::rich`].
//!
//! A table is a header row, a delimiter row with as many cells (`---`,
//! `:--`, `--:` or `:-:`), then every following line that has a `|`.
//! Lines inside a code fence are never a table. A table still streaming
//! in shows as prose until its delimiter row arrives.

/// How a column's cells line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Text as written, lines joined by `\n`.
    Prose(String),
    Table {
        align: Vec<Align>,
        head: Vec<String>,
        /// Each row has as many cells as the head: short rows are padded
        /// with empty cells, long ones cut.
        rows: Vec<Vec<String>>,
    },
}

/// `text` split into prose and tables, in order. Prose blocks keep their
/// text exactly, blank lines included.
pub fn blocks(text: &str) -> Vec<Block> {
    if text.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut blocks = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut fenced = false;
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        if is_fence(line) {
            fenced = !fenced;
        }
        let table = (!fenced).then(|| table_at(&lines, at)).flatten();
        let Some((table, used)) = table else {
            prose.push(line);
            at += 1;
            continue;
        };
        if !prose.is_empty() {
            blocks.push(Block::Prose(prose.join("\n")));
            prose.clear();
        }
        blocks.push(table);
        at += used;
    }
    if !prose.is_empty() {
        blocks.push(Block::Prose(prose.join("\n")));
    }
    blocks
}

fn is_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// The table starting at `lines[at]`, and how many lines it takes.
fn table_at(lines: &[&str], at: usize) -> Option<(Block, usize)> {
    let head = lines[at];
    if !head.contains('|') {
        return None;
    }
    let head = cells(head);
    let align = delimiter(lines.get(at + 1)?)?;
    if align.len() != head.len() {
        return None;
    }
    let width = head.len();
    let rows: Vec<Vec<String>> = lines[at + 2..]
        .iter()
        .take_while(|line| line.contains('|') && !line.trim().is_empty())
        .map(|line| {
            let mut row = cells(line);
            row.resize(width, String::new());
            row
        })
        .collect();
    let used = 2 + rows.len();
    Some((Block::Table { align, head, rows }, used))
}

/// A delimiter row's alignments, or `None` if `line` is not one.
fn delimiter(line: &str) -> Option<Vec<Align>> {
    if !line.contains('-') {
        return None;
    }
    // One column needs a pipe, or it is a setext heading or a rule.
    let cells = cells(line);
    if cells.len() == 1 && !line.contains('|') {
        return None;
    }
    cells
        .iter()
        .map(|cell| {
            let left = cell.starts_with(':');
            let right = cell.ends_with(':');
            let dashes = cell.trim_start_matches(':').trim_end_matches(':');
            if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

/// A row's cells, trimmed: the outer pipes are optional, `\|` is a pipe
/// inside a cell, and a pipe inside a code span does not split.
fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = match line.strip_suffix('|') {
        Some(inner) if !inner.ends_with('\\') => inner,
        _ => line,
    };
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut code = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '`' => {
                code = !code;
                cell.push(c);
            }
            '|' if !code => cells.push(std::mem::take(&mut cell)),
            _ => cell.push(c),
        }
    }
    cells.push(cell);
    cells
        .into_iter()
        .map(|cell| cell.trim().to_owned())
        .collect()
}
