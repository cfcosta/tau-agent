//! [`Terminal`]: libghostty-vt's terminal, with plain text for a model,
//! VT for replay and a styled [`Screen`] for a renderer.

use std::marker::PhantomData;

use libghostty_vt::{
    self as vt,
    fmt::{Format, Formatter, FormatterOptions},
    render::{CellIterator, CursorVisualStyle, RenderState, RowIterator},
    screen::{CellWide, Screen as ActiveScreen, TrackedGridRef},
    selection::{FormatOptions, Selection},
    style::{self as vt_style, StyleColor},
    terminal::{Point, PointCoordinate, PointSpace, ScrollViewport},
};

use crate::{
    error::Error,
    screen::{
        Cursor,
        CursorShape,
        Line,
        Rgb,
        Screen,
        Style,
        TextRun,
        Underline,
    },
};

/// A terminal's size in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    /// The size tool output runs at: 120 columns by 40 rows. It is fixed
    /// so a command's plain text never depends on a window's width.
    pub const TOOL: Size = Size {
        cols: 120,
        rows: 40,
    };
}

/// How to create a [`Terminal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub size: Size,
    /// The most memory the scrollback may take, in bytes (libghostty-vt
    /// counts scrollback in bytes, not rows: about 1 KiB per row at 120
    /// columns). The oldest rows go first past it.
    pub scrollback: usize,
    /// Record rows as they scroll into the scrollback, for
    /// [`Terminal::take_recorded`], so no row is lost to the scrollback
    /// limit. Recording raises `scrollback` to what it needs.
    pub record: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            size: Size::TOOL,
            scrollback: 8 << 20,
            record: false,
        }
    }
}

/// How far to scroll the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// To the oldest row kept.
    Top,
    /// To the active screen, where output goes.
    Bottom,
    /// By this many rows; negative is up.
    Delta(isize),
}

/// While recording, [`Terminal::write`] feeds bytes in pieces of at
/// most this many, recording after each. One byte moves the cursor down
/// at most one row, so at most this many rows can scroll into the
/// scrollback between two recordings.
const RECORD_PIECE: usize = 4096;

/// Bytes of scrollback per cell to allow for when recording: libghostty
/// takes about 9 per cell, and this leaves room for styles and
/// graphemes.
const RECORD_BYTES_PER_CELL: usize = 16;

/// A terminal emulator: libghostty-vt's parser and screen state.
///
/// Not `Send` nor `Sync` (libghostty-vt is not thread-safe): create it
/// on the thread that uses it.
///
/// ```compile_fail
/// fn send<T: Send>() {}
/// send::<tau_terminal::Terminal>();
/// ```
pub struct Terminal {
    // Declared before `inner`, so they are dropped first.
    mark: Option<TrackedGridRef>,
    recording: bool,
    recorded: String,
    render: RenderState<'static>,
    rows_iter: RowIterator<'static>,
    cells_iter: CellIterator<'static>,
    inner: vt::Terminal<'static, 'static>,
    _not_send: PhantomData<*const ()>,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("size", &self.size())
            .field("recording", &self.recording)
            .finish_non_exhaustive()
    }
}

impl Terminal {
    pub fn new(options: Options) -> Result<Self, Error> {
        let Size { cols, rows } = options.size;
        let scrollback = if options.record {
            let needed = (RECORD_PIECE + usize::from(rows))
                * usize::from(cols)
                * RECORD_BYTES_PER_CELL;
            options.scrollback.max(needed)
        } else {
            options.scrollback
        };
        let inner = vt::Terminal::new(vt::TerminalOptions {
            cols,
            rows,
            max_scrollback: scrollback,
        })?;
        Ok(Self {
            mark: None,
            recording: options.record,
            recorded: String::new(),
            render: RenderState::new()?,
            rows_iter: RowIterator::new()?,
            cells_iter: CellIterator::new()?,
            inner,
            _not_send: PhantomData,
        })
    }

    /// The size in cells.
    pub fn size(&self) -> Size {
        Size {
            cols: self.inner.cols().unwrap_or(0),
            rows: self.inner.rows().unwrap_or(0),
        }
    }

    /// Feeds output a program wrote: text, control characters and escape
    /// sequences, split anywhere (even inside a sequence or a UTF-8
    /// character). Invalid input never fails; libghostty-vt skips it.
    ///
    /// Queries a program sends (cursor position, device attributes) go
    /// unanswered.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if !self.recording {
            self.inner.vt_write(bytes);
            return Ok(());
        }
        for piece in bytes.chunks(RECORD_PIECE) {
            self.inner.vt_write(piece);
            self.record()?;
        }
        Ok(())
    }

    /// Resizes to `size`. The primary screen reflows its soft-wrapped
    /// lines; the alternate screen does not.
    pub fn resize(&mut self, size: Size) -> Result<(), Error> {
        self.inner.resize(size.cols, size.rows, 1, 1)?;
        Ok(())
    }

    /// Scrolls the viewport [`Terminal::snapshot`] shows.
    pub fn scroll(&mut self, scroll: Scroll) {
        self.inner.scroll_viewport(match scroll {
            Scroll::Top => ScrollViewport::Top,
            Scroll::Bottom => ScrollViewport::Bottom,
            Scroll::Delta(rows) => ScrollViewport::Delta(rows),
        });
    }

    /// The rows recorded since the last call, as plain text (see
    /// [`Terminal::text`]); empty unless [`Options::record`] is set.
    ///
    /// Every row that scrolls into the scrollback is recorded once, in
    /// order, ending with a newline unless it soft-wraps into the next.
    /// So all the recorded text followed by [`Terminal::text`] is the
    /// whole output, however long, as plain text.
    pub fn take_recorded(&mut self) -> String {
        std::mem::take(&mut self.recorded)
    }

    /// The plain text of the rows not yet recorded: the scrollback and
    /// the screen when not recording, and otherwise the rows since the
    /// last one recorded. It is what a program's output reads as once
    /// its escape sequences are applied:
    ///
    /// - soft-wrapped rows are joined, and written spaces are kept;
    /// - a `\r` redraw leaves only what was drawn last, as a person saw;
    /// - blank rows up to the cursor count as lines, so output that ends
    ///   with a newline gives text that ends with one;
    /// - a tab becomes the spaces up to the next tab stop.
    ///
    /// For output with no control characters but `\r\n` line ends, it is
    /// the output with each `\r\n` turned into `\n`.
    pub fn text(&self) -> Result<String, Error> {
        let history = self.history_rows()?;
        let total = history + usize::from(self.inner.rows()?);
        let cursor = history + usize::from(self.inner.cursor_y()?);
        let start = if self.recording {
            self.first_unrecorded(history)?
        } else {
            0
        };

        // The text runs to the cursor's row, or past it to the last
        // row with something on it.
        let mut end = cursor;
        for y in (cursor + 1..total).rev() {
            if !self.row_is_blank(y)? {
                end = y;
                break;
            }
        }
        if start > end {
            return Ok(String::new());
        }
        self.rows_text(start, end)
    }

    /// The terminal's content as VT sequences: the palette, the
    /// scrollback and the screen with their styles, then the cursor's
    /// position. Written to a new terminal of the same size, it rebuilds
    /// what this one shows. Modes (a hidden cursor, a scrolling region)
    /// are left out: the result is for showing a finished run, not for
    /// going on with one.
    pub fn vt(&self) -> Result<Vec<u8>, Error> {
        let mut formatter = Formatter::new(
            &self.inner,
            FormatterOptions::new()
                .with_format(Format::Vt)
                .with_unwrap(true)
                .with_trim(false)
                .with_palette(true),
        )?;
        let mut out = formatter.format_alloc(None)?.to_vec();

        // The formatter stops at the last row with something on it; the
        // blank rows after it put the content back at the same height.
        let history = self.history_rows()?;
        let total = history + usize::from(self.inner.rows()?);
        let mut last = total - 1;
        while last > 0 && self.row_is_blank(last)? {
            last -= 1;
        }
        out.extend_from_slice(b"\x1b[0m");
        out.extend_from_slice(&b"\r\n".repeat(total - 1 - last));
        out.extend_from_slice(
            format!(
                "\x1b[{};{}H",
                self.inner.cursor_y()? + 1,
                self.inner.cursor_x()? + 1
            )
            .as_bytes(),
        );
        Ok(out)
    }

    /// The viewport, styled, for a renderer.
    pub fn snapshot(&mut self) -> Result<Screen, Error> {
        let snapshot = self.render.update(&self.inner)?;
        let colors = snapshot.colors()?;
        let cols = snapshot.cols()?;
        let rows = snapshot.rows()?;
        let cursor = match snapshot.cursor_viewport()? {
            Some(at) => Some(Cursor {
                col: at.x,
                row: at.y,
                shape: match snapshot.cursor_visual_style()? {
                    CursorVisualStyle::Bar => CursorShape::Bar,
                    CursorVisualStyle::Underline => CursorShape::Underline,
                    CursorVisualStyle::BlockHollow => CursorShape::BlockHollow,
                    _ => CursorShape::Block,
                },
                visible: snapshot.cursor_visible()?,
                at_wide_tail: at.at_wide_tail,
            }),
            None => None,
        };

        let mut lines = Vec::with_capacity(usize::from(rows));
        let mut row_iter = self.rows_iter.update(&snapshot)?;
        while let Some(row) = row_iter.next() {
            let wrapped = row.raw_row()?.is_wrapped()?;
            let mut runs: Vec<TextRun> = Vec::new();
            let mut cell_iter = self.cells_iter.update(row)?;
            let mut col = 0u16;
            let mut last_wide = false;
            let mut grapheme = String::new();
            while let Some(cell) = cell_iter.next() {
                let at = col;
                col += 1;
                let wide = cell.raw_cell()?.wide()?;
                if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                    continue;
                }
                let style = style_of(
                    cell.style()?,
                    cell.fg_color()?.map(rgb),
                    cell.bg_color()?.map(rgb),
                );
                grapheme.clear();
                cell.graphemes_utf8(&mut grapheme)?;
                if grapheme.is_empty() {
                    if style.bg.is_none() && !style.inverse {
                        continue;
                    }
                    grapheme.push(' ');
                }
                let is_wide = wide == CellWide::Wide;
                match runs.last_mut() {
                    Some(last)
                        if !is_wide
                            && !last_wide
                            && last.style == style
                            && last.col + last.cells == at =>
                    {
                        last.text.push_str(&grapheme);
                        last.cells += 1;
                    }
                    _ => runs.push(TextRun {
                        col: at,
                        cells: if is_wide { 2 } else { 1 },
                        text: grapheme.clone(),
                        style,
                    }),
                }
                last_wide = is_wide;
            }
            lines.push(Line { runs, wrapped });
        }

        Ok(Screen {
            cols,
            rows,
            lines,
            cursor,
            foreground: rgb(colors.foreground),
            background: rgb(colors.background),
        })
    }

    /// Rows in the scrollback of the active screen (the alternate screen
    /// has none).
    fn history_rows(&self) -> Result<usize, Error> {
        Ok(match self.inner.active_screen()? {
            ActiveScreen::Primary => self.inner.scrollback_rows()?,
            ActiveScreen::Alternate => 0,
        })
    }

    /// The first scrollback row not yet recorded: the one after the mark
    /// on the last recorded row. With no mark (nothing recorded yet, or
    /// the marked row was dropped past the scrollback limit or erased by
    /// the program), the oldest row kept.
    fn first_unrecorded(&self, history: usize) -> Result<usize, Error> {
        let marked = match &self.mark {
            Some(mark) => mark.point(PointSpace::Screen)?,
            None => None,
        };
        Ok(match marked {
            Some(point) => (point.y as usize + 1).min(history),
            None => 0,
        })
    }

    /// Records the rows that scrolled into the scrollback since the last
    /// time. History rows never change once there (a program can only
    /// write to the screen), so a row is recorded exactly once.
    fn record(&mut self) -> Result<(), Error> {
        if self.inner.active_screen()? != ActiveScreen::Primary {
            return Ok(());
        }
        let history = self.inner.scrollback_rows()?;
        let start = self.first_unrecorded(history)?;
        if start >= history {
            return Ok(());
        }
        let last = history - 1;
        let mut text = self.rows_text(start, last)?;
        if !self.row_wrapped(last)? {
            text.push('\n');
        }
        self.recorded.push_str(&text);

        let point = Point::Screen(PointCoordinate {
            x: 0,
            y: last as u32,
        });
        match &mut self.mark {
            Some(mark) => {
                mark.set(&mut self.inner, point)?;
            }
            None => self.mark = Some(self.inner.track_grid_ref(point)?),
        }
        Ok(())
    }

    /// The plain text of screen rows `start..=end` (scrollback first),
    /// soft wraps joined. The formatter leaves out blank rows at the
    /// end, so they are added back as the newlines between them.
    fn rows_text(&self, start: usize, end: usize) -> Result<String, Error> {
        let mut last = end;
        while last > start && self.row_is_blank(last)? {
            last -= 1;
        }
        if last == start && self.row_is_blank(start)? {
            return Ok("\n".repeat(end - start));
        }
        let mut text = self.format_rows(start, end, true)?;
        text.push_str(&"\n".repeat(end - last));
        Ok(text)
    }

    fn row_is_blank(&self, y: usize) -> Result<bool, Error> {
        Ok(self.format_rows(y, y, false)?.is_empty())
    }

    fn row_wrapped(&self, y: usize) -> Result<bool, Error> {
        Ok(self
            .inner
            .grid_ref(Point::Screen(PointCoordinate { x: 0, y: y as u32 }))?
            .row()?
            .is_wrapped()?)
    }

    /// Screen rows `start..=end` as plain text, with written spaces.
    fn format_rows(
        &self,
        start: usize,
        end: usize,
        unwrap: bool,
    ) -> Result<String, Error> {
        let cols = self.inner.cols()?;
        let from = self.inner.grid_ref(Point::Screen(PointCoordinate {
            x: 0,
            y: start as u32,
        }))?;
        let last_col = |x: u16| {
            self.inner
                .grid_ref(Point::Screen(PointCoordinate { x, y: end as u32 }))
        };
        let mut to = last_col(cols.saturating_sub(1))?;
        // A wide character that did not fit leaves a spacer at the end
        // of its row and goes on the next; a selection ending on the
        // spacer would take the character from the next row.
        if cols > 1 && to.cell()?.wide()? == CellWide::SpacerHead {
            to = last_col(cols - 2)?;
        }
        let selection = Selection::new(from, to, false);
        let bytes = self.inner.format_selection_alloc(
            None,
            FormatOptions::new()
                .with_emit_format(Format::Plain)
                .with_unwrap(unwrap)
                .with_trim(false)
                .with_selection(&selection),
        )?;
        Ok(match bytes {
            Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            None => String::new(),
        })
    }
}

fn rgb(color: vt_style::RgbColor) -> Rgb {
    Rgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn style_of(style: vt_style::Style, fg: Option<Rgb>, bg: Option<Rgb>) -> Style {
    // `fg_color` and `bg_color` are already resolved through the
    // palette; the style's own colors only say whether one was set.
    let fg = match style.fg_color {
        StyleColor::None => None,
        _ => fg,
    };
    Style {
        fg,
        bg,
        bold: style.bold,
        faint: style.faint,
        italic: style.italic,
        underline: match style.underline {
            vt_style::Underline::None => Underline::None,
            vt_style::Underline::Single => Underline::Single,
            vt_style::Underline::Double => Underline::Double,
            vt_style::Underline::Curly => Underline::Curly,
            vt_style::Underline::Dotted => Underline::Dotted,
            vt_style::Underline::Dashed => Underline::Dashed,
            _ => Underline::Single,
        },
        strikethrough: style.strikethrough,
        overline: style.overline,
        inverse: style.inverse,
        invisible: style.invisible,
        blink: style.blink,
    }
}
