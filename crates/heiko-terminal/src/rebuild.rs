//! [`render`]: a terminal's active screen, scrollback first, as VT text
//! that rebuilds it cell by cell in a new terminal of the same size.
//!
//! libghostty-vt's VT formatter writes for reading, not rebuilding: it
//! drops rows holding only a background, writes empty cells as spaces
//! in the style before them, sets a style before the character that
//! wraps (so the scroll fills the new row with its background) and
//! joins wrapped rows without the gap a wide character leaves. So the
//! rows are written here from the cells:
//!
//! - every cell with text is written in its own style, from SGR 0;
//! - an empty cell is skipped over, and one with a background is
//!   erased (ECH) with it, so neither becomes a styled space;
//! - a row ends with `\r\n` in the default style, so a scroll brings in
//!   a blank row; a soft-wrapped row ends by writing a blank past its
//!   end, which wraps it, and the next row writes over the blank. The
//!   blank is two cells wide when the row ends empty, as a wide
//!   character that did not fit leaves it, and in the default style
//!   unless that empty cell has one;
//! - every row is written, blank ones too, so the scrollback keeps its
//!   height; then the palette entries and default colors the program
//!   changed, and the cursor's position.
//!
//! libghostty-vt's rules this leans on, found by the property: ECH on a
//! soft-wrapped row undoes its wrap; writing over a wide character
//! turns the spacer it left on the row above into an empty cell in the
//! spacer's style, unless that row has scrolled off (erasing it does so
//! even then); and a narrow character written over a wide one empties
//! the other half in the writer's background.

use std::collections::VecDeque;

use libghostty_vt::{
    self as vt,
    color::Rgb as RgbColor,
    grid_ref::GridRef,
    point::{Coordinate as PointCoordinate, Point},
    screen::{CellContentTag, CellWide},
    style::{self as vt_style, StyleColor},
};

use crate::error::Error;

/// What a cell holds, as far as rebuilding it goes.
#[derive(Clone)]
enum Content {
    /// No text, in a style: the default for a new terminal's cell (and
    /// a wide character's spacer), a background for an erased one.
    Blank(vt_style::Style),
    /// A character, `cells` wide, in a style.
    Text {
        text: String,
        cells: u16,
        style: vt_style::Style,
        link: String,
    },
}

const PLAIN: Content = Content::Blank(PLAIN_STYLE);
const PLAIN_STYLE: vt_style::Style = vt_style::Style {
    fg_color: StyleColor::None,
    bg_color: StyleColor::None,
    underline_color: StyleColor::None,
    bold: false,
    italic: false,
    faint: false,
    blink: false,
    inverse: false,
    invisible: false,
    strikethrough: false,
    overline: false,
    underline: vt_style::Underline::None,
};

/// Writes the VT text; tracks the pen and the cursor of the terminal
/// that reads it, so a sequence is written only when something changes.
struct Writer {
    out: Vec<u8>,
    cols: u16,
    /// The cursor's column on the row being written.
    col: u16,
    /// The style set by the last SGR.
    pen: vt_style::Style,
    /// The hyperlink set by the last OSC 8; empty for none.
    link: String,
    /// Cells at the start of the row being written that hold the blank
    /// [`Writer::wrap`] wrote: written over, or erased if the row has
    /// nothing there.
    blank: u16,
}

impl Writer {
    fn pen(&mut self, style: vt_style::Style) {
        if self.pen != style {
            self.out.extend_from_slice(&sgr(style));
            self.pen = style;
        }
    }

    fn link(&mut self, uri: &str) {
        if self.link != uri {
            self.out.extend_from_slice(b"\x1b]8;;");
            self.out.extend_from_slice(uri.as_bytes());
            self.out.extend_from_slice(b"\x1b\\");
            self.link = uri.to_owned();
        }
    }

    fn plain(&mut self) {
        self.pen(PLAIN_STYLE);
        self.link("");
    }

    /// Moves to `col` of the row being written.
    fn go(&mut self, col: u16) {
        if self.col != col {
            self.out
                .extend_from_slice(format!("\x1b[{}G", col + 1).as_bytes());
            self.col = col;
        }
    }

    /// Moves `rows` rows up (negative is down), to column 0.
    fn rows(&mut self, rows: isize) {
        let (n, dir) = if rows < 0 { (-rows, 'B') } else { (rows, 'A') };
        self.out
            .extend_from_slice(format!("\x1b[{n}{dir}\r").as_bytes());
        self.col = 0;
    }

    /// Erases `cells` cells at `col` in `style`'s background.
    fn erase(&mut self, col: u16, cells: u16, style: vt_style::Style) {
        self.go(col);
        self.link("");
        self.pen(background(style.bg_color));
        self.out
            .extend_from_slice(format!("\x1b[{cells}X").as_bytes());
    }

    /// Writes `text`, `cells` wide, at `col`. At the last column the
    /// cursor stays there, waiting to wrap.
    fn text(&mut self, col: u16, text: &str, cells: u16) {
        self.go(col);
        self.out.extend_from_slice(text.as_bytes());
        self.col = (col + cells).min(self.cols - 1);
    }

    /// Ends a row that does not wrap.
    fn newline(&mut self) {
        self.plain();
        self.out.extend_from_slice(b"\r\n");
        self.col = 0;
    }

    /// Ends a soft-wrapped row: a blank of `cells` cells written where
    /// the row ends wraps it, in `style` (the default, so a scroll
    /// brings in a blank row, unless a spacer must keep a style). The
    /// next row writes over it; erasing a wide blank would undo the
    /// spacer it leaves.
    fn wrap(&mut self, cells: u16, style: vt_style::Style) {
        self.link("");
        self.pen(style);
        self.out.extend_from_slice(if cells == 2 {
            "\u{3000}".as_bytes()
        } else {
            b" "
        });
        self.out.push(b'\r');
        self.col = 0;
        self.blank = cells;
    }
}

/// One row as [`render`] needs it.
struct Row {
    cells: Vec<Content>,
    wrapped: bool,
    /// Whether the row ends with a wide character's spacer.
    head: bool,
}

/// The first cells of a row that wraps from one that ends with a spacer
/// but starts with no wide character, to write once the row above has
/// scrolled off the screen.
struct Patch {
    /// The row's index from the oldest row of the scrollback.
    y: usize,
    cells: Vec<Content>,
}

/// `inner`'s active screen, with its `history` rows of scrollback, as
/// VT text.
pub(crate) fn render(
    inner: &vt::Terminal<'static, 'static>,
    history: usize,
) -> Result<Vec<u8>, Error> {
    let cols = inner.cols()?;
    let rows = usize::from(inner.rows()?);
    let total = history + rows;
    let last = cols - 1;
    let mut writer = Writer {
        out: b"\x1b[0m".to_vec(),
        cols,
        col: 0,
        pen: PLAIN_STYLE,
        link: String::new(),
        blank: 0,
    };

    let read = |y: usize| -> Result<Row, Error> {
        let cells = (0..cols)
            .map(|x| content(&cell_at(inner, x, y)?))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Row {
            cells,
            wrapped: cell_at(inner, 0, y)?.row()?.is_wrapped()?
                && y + 1 < total,
            head: cell_at(inner, last, y)?.cell()?.wide()?
                == CellWide::SpacerHead,
        })
    };
    let mut patches = VecDeque::<Patch>::new();
    let mut row = read(0)?;
    // Where the row being written starts: past the cells a patch writes.
    let mut start = 0;
    for y in 0..total {
        let next = if y + 1 < total {
            Some(read(y + 1)?)
        } else {
            None
        };
        if patches.front().is_some_and(|patch| patch.y + rows - 1 == y)
            && let Some(patch) = patches.pop_front()
        {
            // The patched row is at the top of the screen and the row
            // above it has scrolled off: write its first characters over
            // the blank there, then come back. Only characters: ECH on a
            // wrapped row undoes its wrap, and the half of the blank a
            // narrow character leaves is emptied in its style, as it was
            // when the program wrote it.
            let up = (rows - 1) as isize;
            writer.rows(up);
            for (x, cell) in patch.cells.iter().enumerate() {
                if let Content::Text { .. } = cell {
                    write_cell(&mut writer, &patch.cells, x);
                }
            }
            writer.rows(-up);
        }
        write_row(&mut writer, &row.cells, std::mem::take(&mut start));

        let Some(next) = next else { break };
        if !row.wrapped {
            writer.newline();
            row = next;
            continue;
        }
        let end = match &row.cells[usize::from(last)] {
            Content::Text { .. } => {
                writer.wrap(1, PLAIN_STYLE);
                row = next;
                continue;
            }
            Content::Blank(style) => *style,
        };
        // The row ends empty: a wide character that did not fit left a
        // spacer there. Writing one at the last column leaves the same
        // spacer; erasing anything there would undo the wrap.
        writer.go(last);
        let wide_next = matches!(next.cells[0], Content::Text { cells: 2, .. });
        if !row.head {
            // Written over, the spacer is an empty cell in its style.
            writer.wrap(2, end);
            writer.plain();
            writer.out.extend_from_slice(b" \r");
            if end.bg_color != StyleColor::None {
                // The wrap may have scrolled in a row of that background.
                writer.erase(0, cols, PLAIN_STYLE);
                writer.blank = 0;
            } else {
                writer.blank = 1;
            }
        } else if !wide_next && y < history && rows > 1 {
            // The spacer stayed though the next row starts with no wide
            // character: that row was written over once this one had
            // scrolled off, where nothing changes it. Do the same, the
            // blank left in place until then.
            writer.wrap(2, PLAIN_STYLE);
            let mut end = 0;
            while end < 2 {
                end += match &next.cells[end] {
                    Content::Text { cells, .. } => usize::from(*cells),
                    Content::Blank(_) => 1,
                };
            }
            patches.push_back(Patch {
                y: y + 1,
                cells: next.cells[..end].to_vec(),
            });
            writer.blank = 0;
            start = end;
        } else {
            writer.wrap(2, PLAIN_STYLE);
        }
        row = next;
    }
    writer.plain();

    let palette = inner.color_palette()?;
    let defaults = inner.default_color_palette()?;
    for (index, (color, default)) in
        palette.0.iter().zip(defaults.0.iter()).enumerate()
    {
        if color != default {
            writer.out.extend_from_slice(
                format!("\x1b]4;{index};{}\x1b\\", spec(*color)).as_bytes(),
            );
        }
    }
    for (osc, color, default) in [
        (10, inner.fg_color()?, inner.default_fg_color()?),
        (11, inner.bg_color()?, inner.default_bg_color()?),
    ] {
        if let Some(color) = color.filter(|_| color != default) {
            writer.out.extend_from_slice(
                format!("\x1b]{osc};{}\x1b\\", spec(color)).as_bytes(),
            );
        }
    }
    writer.out.extend_from_slice(
        format!("\x1b[{};{}H", inner.cursor_y()? + 1, inner.cursor_x()? + 1)
            .as_bytes(),
    );
    Ok(writer.out)
}

/// Writes one row's cells from `start`, the cursor on its row.
fn write_row(writer: &mut Writer, row: &[Content], start: usize) {
    let blank = std::mem::take(&mut writer.blank);
    if blank > 0 && matches!(row.first(), Some(Content::Blank(_))) {
        writer.erase(0, blank, PLAIN_STYLE);
    }
    let mut x = start;
    while x < row.len() {
        x += write_cell(writer, row, x);
    }
}

/// Writes the cell at `x` of `row`, with the blanks after it of the
/// same style; returns how many cells it took.
fn write_cell(writer: &mut Writer, row: &[Content], x: usize) -> usize {
    let col = x as u16;
    match &row[x] {
        Content::Blank(style) if *style == PLAIN_STYLE => 1,
        Content::Blank(style) => {
            let mut end = x + 1;
            while matches!(row.get(end), Some(Content::Blank(next)) if next == style)
            {
                end += 1;
            }
            writer.erase(col, (end - x) as u16, *style);
            end - x
        }
        Content::Text {
            text,
            cells,
            style,
            link,
        } => {
            writer.pen(*style);
            writer.link(link);
            writer.text(col, text, *cells);
            usize::from(*cells)
        }
    }
}

fn cell_at<'a>(
    inner: &'a vt::Terminal<'static, 'static>,
    x: u16,
    y: usize,
) -> Result<GridRef<'a>, Error> {
    Ok(inner.grid_ref(Point::Screen(PointCoordinate { x, y: y as u32 }))?)
}

fn content(at: &GridRef<'_>) -> Result<Content, Error> {
    let cell = at.cell()?;
    let wide = cell.wide()?;
    if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
        return Ok(PLAIN);
    }
    match cell.content_tag()? {
        CellContentTag::BgColorPalette => {
            return Ok(Content::Blank(background(StyleColor::Palette(
                cell.bg_color_palette()?,
            ))));
        }
        CellContentTag::BgColorRgb => {
            return Ok(Content::Blank(background(StyleColor::Rgb(
                cell.bg_color_rgb()?,
            ))));
        }
        _ => {}
    }
    let style = if cell.has_styling()? {
        at.style()?
    } else {
        PLAIN_STYLE
    };
    if !cell.has_text()? {
        return Ok(Content::Blank(style));
    }
    let mut chars = vec!['\0'; 8];
    let len = match at.graphemes(&mut chars) {
        Ok(len) => len,
        Err(vt::Error::OutOfSpace { required }) => {
            chars.resize(required, '\0');
            at.graphemes(&mut chars)?
        }
        Err(error) => return Err(error.into()),
    };
    let link = if cell.has_hyperlink()? {
        let mut buf = vec![0; 256];
        let len = match at.hyperlink_uri(&mut buf) {
            Ok(len) => len,
            Err(vt::Error::OutOfSpace { required }) => {
                buf.resize(required, 0);
                at.hyperlink_uri(&mut buf)?
            }
            Err(error) => return Err(error.into()),
        };
        String::from_utf8_lossy(&buf[..len]).into_owned()
    } else {
        String::new()
    };
    Ok(Content::Text {
        text: chars[..len].iter().collect(),
        cells: if wide == CellWide::Wide { 2 } else { 1 },
        style,
        link,
    })
}

fn background(bg: StyleColor) -> vt_style::Style {
    vt_style::Style {
        bg_color: bg,
        ..PLAIN_STYLE
    }
}

/// SGR 0, then what `style` sets.
fn sgr(style: vt_style::Style) -> Vec<u8> {
    let mut params = vec!["0".to_owned()];
    for (on, code) in [
        (style.bold, "1"),
        (style.faint, "2"),
        (style.italic, "3"),
        (style.blink, "5"),
        (style.inverse, "7"),
        (style.invisible, "8"),
        (style.strikethrough, "9"),
        (style.overline, "53"),
    ] {
        if on {
            params.push(code.to_owned());
        }
    }
    params.extend(
        match style.underline {
            vt_style::Underline::None => None,
            vt_style::Underline::Double => Some("4:2"),
            vt_style::Underline::Curly => Some("4:3"),
            vt_style::Underline::Dotted => Some("4:4"),
            vt_style::Underline::Dashed => Some("4:5"),
            _ => Some("4"),
        }
        .map(str::to_owned),
    );
    for (color, base) in [
        (style.fg_color, 38),
        (style.bg_color, 48),
        (style.underline_color, 58),
    ] {
        match color {
            StyleColor::None => {}
            StyleColor::Palette(index) => {
                params.push(format!("{base};5;{}", index.0))
            }
            StyleColor::Rgb(RgbColor { r, g, b }) => {
                params.push(format!("{base};2;{r};{g};{b}"))
            }
        }
    }
    format!("\x1b[{}m", params.join(";")).into_bytes()
}

/// An X11 color spec, as OSC 4, 10 and 11 take.
fn spec(color: RgbColor) -> String {
    format!("rgb:{:02x}/{:02x}/{:02x}", color.r, color.g, color.b)
}
