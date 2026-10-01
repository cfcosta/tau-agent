//! A styled snapshot of a terminal's viewport: plain data a renderer
//! draws from, with no tie to libghostty-vt or to a thread.

/// A 24-bit color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A color a program chose: an entry of the palette, or a color of its
/// own. A renderer resolves palette entries through its own palette
/// (see [`crate::Palette`]), so the same output can take any
/// theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// An entry of the 256-color palette: 0 to 15 are the ANSI colors,
    /// 16 to 231 a 6×6×6 color cube, 232 to 255 a gray ramp.
    Palette(u8),
    /// A 24-bit color (`38;2;r;g;b`).
    Rgb(Rgb),
}

/// How a run's text is underlined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// How a run of cells looks. `None` colors are the renderer's default
/// foreground or background. `inverse` is left for the renderer to
/// apply, so it can swap its own defaults in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub faint: bool,
    pub italic: bool,
    pub underline: Underline,
    pub strikethrough: bool,
    pub overline: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub blink: bool,
}

/// Adjacent cells of one style on one line.
///
/// A wide character (two cells, such as most CJK text and emoji) is
/// always a run of its own, with `cells == 2`, so a renderer can place
/// every run at `col * cell_width` and never has to measure text.
/// Empty cells with no background are left out: the gaps between runs,
/// and the rest of the line after the last run, are blank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRun {
    /// The first column the run covers.
    pub col: u16,
    /// How many columns it covers.
    pub cells: u16,
    /// Its text: one grapheme cluster per cell (or per wide character),
    /// with a space for an empty cell that has a background.
    pub text: String,
    pub style: Style,
}

/// One row of the viewport.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    pub runs: Vec<TextRun>,
    /// Whether the row continues on the next one (a soft wrap), as
    /// opposed to ending with a newline.
    pub wrapped: bool,
}

impl Line {
    /// The row's text: runs at their columns, gaps as spaces, trailing
    /// blanks left out.
    pub fn text(&self) -> String {
        self.text_between(0, u16::MAX).trim_end().to_owned()
    }

    /// The text of the cells from column `start` up to, not including,
    /// `end`, gaps as spaces, and nothing past the last run. A run with a
    /// wide character or a grapheme cluster of several characters is
    /// taken whole when it overlaps the range.
    pub fn text_between(&self, start: u16, end: u16) -> String {
        let mut out = String::new();
        let mut col = start;
        for run in &self.runs {
            let run_end = run.col + run.cells;
            if run_end <= start || run.col >= end {
                continue;
            }
            let from = run.col.max(start);
            while col < from {
                out.push(' ');
                col += 1;
            }
            if usize::from(run.cells) == run.text.chars().count() {
                let skip = usize::from(from - run.col);
                let take = usize::from(end.min(run_end) - from);
                out.extend(run.text.chars().skip(skip).take(take));
            } else {
                out.push_str(&run.text);
            }
            col = end.min(run_end);
        }
        out
    }

    /// Whether nothing is drawn on the row.
    pub fn is_blank(&self) -> bool {
        self.runs.is_empty()
    }
}

/// The cursor's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorShape {
    #[default]
    Block,
    BlockHollow,
    Bar,
    Underline,
}

/// Where the cursor is, in viewport cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShape,
    /// Whether the program left it shown (DECTCEM).
    pub visible: bool,
    /// Whether it sits on the right half of a wide character.
    pub at_wide_tail: bool,
}

/// A terminal's viewport, styled: what [`crate::Terminal::snapshot`]
/// returns and a renderer draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    /// Exactly `rows` lines, top to bottom.
    pub lines: Vec<Line>,
    /// `None` when the cursor is outside the viewport (it has been
    /// scrolled back).
    pub cursor: Option<Cursor>,
    /// The terminal's default colors, which a program can change (OSC
    /// 10 and 11). A renderer may use its own theme's instead.
    pub foreground: Rgb,
    pub background: Rgb,
}

impl Screen {
    /// The viewport's text, one line per row with trailing blanks
    /// left out: a plain view of the snapshot, for tests and logs.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}
