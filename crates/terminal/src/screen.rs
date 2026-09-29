//! A styled snapshot of a terminal's viewport: plain data a renderer
//! draws from, with no tie to libghostty-vt or to a thread.

/// A 24-bit color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
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

/// How a run of cells looks. Colors are resolved through the
/// terminal's palette; `None` means the renderer's default foreground
/// or background. `inverse` is left for the renderer to apply, so it
/// can swap its own defaults in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
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
            .map(|line| {
                let mut out = String::new();
                let mut col = 0u16;
                for run in &line.runs {
                    while col < run.col {
                        out.push(' ');
                        col += 1;
                    }
                    out.push_str(&run.text);
                    col = run.col + run.cells;
                }
                out.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
