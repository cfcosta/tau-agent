//! [`Selection`]: a span of cells, from where a drag started to where it
//! is, and its text.

use crate::screen::Line;

/// A cell: its row among all the rows (0 is the oldest) and its column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub row: usize,
    pub col: u16,
}

/// The cells from `anchor` to `head`, both included, in reading order:
/// the rest of the first row, whole rows between, and the last row up
/// to `head`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
}

impl Selection {
    /// Just the cell at `at`.
    pub fn at(at: Point) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    /// The first and last cells, in reading order.
    pub fn ordered(&self) -> (Point, Point) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The columns selected on `row`, from the first up to, not
    /// including, the second; `None` when none are. `cols` is the row's
    /// width.
    pub fn columns(&self, row: usize, cols: u16) -> Option<(u16, u16)> {
        let (start, end) = self.ordered();
        if row < start.row || row > end.row {
            return None;
        }
        let from = if row == start.row { start.col } else { 0 };
        let to = if row == end.row {
            end.col.saturating_add(1).min(cols)
        } else {
            cols
        };
        (from < to).then_some((from, to))
    }

    /// The selected text of `lines`, where `lines[0]` is row `first`.
    /// Rows join with a newline unless the first soft-wraps into the
    /// second; blanks at the end of a row that ends with a newline are
    /// left out.
    pub fn text(&self, lines: &[Line], first: usize, cols: u16) -> String {
        let (start, end) = self.ordered();
        let mut out = String::new();
        for row in start.row.max(first)..=end.row {
            let Some(line) = lines.get(row - first) else {
                break;
            };
            let Some((from, to)) = self.columns(row, cols) else {
                continue;
            };
            let text = line.text_between(from, to);
            let joined = line.wrapped && to == cols && row < end.row;
            if joined {
                // A full row that wraps: the next row continues it.
                out.push_str(&text);
            } else {
                out.push_str(text.trim_end());
                if row < end.row {
                    out.push('\n');
                }
            }
        }
        out
    }
}

/// The plain text of `lines`: each row's text, joined with a newline
/// unless a row soft-wraps into the next.
pub fn text_of(lines: &[Line], cols: u16) -> String {
    let Some(last) = lines.len().checked_sub(1) else {
        return String::new();
    };
    Selection {
        anchor: Point { row: 0, col: 0 },
        head: Point {
            row: last,
            col: cols.saturating_sub(1),
        },
    }
    .text(lines, 0, cols)
}
