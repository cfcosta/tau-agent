//! What one row draws, in cells: plain data the element paints, so the
//! batching, colors and shapes are testable without a window.

use crate::{
    glyph::{Glyph, glyph},
    palette::Palette,
    screen::{Line, Rgb, Style, Underline},
};

/// A background rectangle: `cells` cells from `col`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    pub col: u16,
    pub cells: u16,
    pub color: Rgb,
}

/// Text to shape as one line and draw at `col`, one glyph per cell
/// (`cells` wide).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub col: u16,
    pub cells: u16,
    pub text: String,
    pub color: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub underline: Underline,
    pub strikethrough: bool,
}

/// A box-drawing or block character, drawn as shapes in its cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub col: u16,
    pub glyph: Glyph,
    pub color: Rgb,
    pub faint: bool,
}

/// A row, ready to paint: backgrounds first, then text and shapes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowLayout {
    /// Backgrounds other than the palette's, adjacent ones of one color
    /// merged.
    pub fills: Vec<Fill>,
    pub texts: Vec<Text>,
    pub shapes: Vec<Shape>,
}

/// Lays `line` out with `palette`.
///
/// - Each [`crate::TextRun`] (adjacent cells of one style) is one
///   [`Text`], less its box-drawing and block characters, which become
///   [`Shape`]s and split the text around them.
/// - A run of blanks with no decoration draws no text.
/// - Invisible text (SGR 8) draws only its background.
/// - A run whose characters do not map one to a cell (a wide character,
///   a cluster of several code points) stays one [`Text`], whole.
pub fn layout(line: &Line, palette: &Palette) -> RowLayout {
    let mut out = RowLayout::default();
    for run in &line.runs {
        let (fg, bg) = palette.colors(&run.style);
        if let Some(color) = bg.filter(|bg| *bg != palette.background) {
            match out.fills.last_mut() {
                Some(last)
                    if last.color == color
                        && last.col + last.cells == run.col =>
                {
                    last.cells += run.cells
                }
                _ => out.fills.push(Fill {
                    col: run.col,
                    cells: run.cells,
                    color,
                }),
            }
        }
        if run.style.invisible {
            continue;
        }
        let decorated =
            run.style.underline != Underline::None || run.style.strikethrough;
        let text = |col: u16, cells: u16, text: String| Text {
            col,
            cells,
            text,
            color: fg,
            bold: run.style.bold,
            italic: run.style.italic,
            faint: run.style.faint,
            underline: run.style.underline,
            strikethrough: run.style.strikethrough,
        };
        let push = |out: &mut RowLayout, piece: Text| {
            if decorated || piece.text.chars().any(|ch| ch != ' ') {
                out.texts.push(piece);
            }
        };
        if usize::from(run.cells) != run.text.chars().count() {
            push(&mut out, text(run.col, run.cells, run.text.clone()));
            continue;
        }
        let mut start = run.col;
        let mut pending = String::new();
        for (col, ch) in (run.col..).zip(run.text.chars()) {
            match glyph(ch) {
                Some(glyph) => {
                    if !pending.is_empty() {
                        let cells = col - start;
                        push(&mut out, text(start, cells, pending.clone()));
                        pending.clear();
                    }
                    out.shapes.push(Shape {
                        col,
                        glyph,
                        color: fg,
                        faint: run.style.faint,
                    });
                    start = col + 1;
                }
                None => pending.push(ch),
            }
        }
        if !pending.is_empty() {
            let cells = run.col + run.cells - start;
            push(&mut out, text(start, cells, pending));
        }
    }
    out
}

/// The character in cell `col` of `line`, if one starts there: what a
/// block cursor draws over.
pub fn char_at(line: &Line, col: u16) -> Option<(String, Style)> {
    let run = line
        .runs
        .iter()
        .find(|run| run.col <= col && col < run.col + run.cells)?;
    if usize::from(run.cells) == run.text.chars().count() {
        let ch = run.text.chars().nth(usize::from(col - run.col))?;
        Some((ch.to_string(), run.style))
    } else {
        (run.col == col).then(|| (run.text.clone(), run.style))
    }
}
