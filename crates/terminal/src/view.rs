//! [`TerminalView`]: a GPUI view of a terminal, live or frozen.
//!
//! Feed it bytes with [`TerminalView::write`] while a program runs, then
//! [`TerminalView::freeze`] it when the program ends: that drops the
//! libghostty terminal and keeps only styled rows. Or build a frozen
//! view at once from stored output with [`TerminalView::replay`].
//!
//! It draws a fixed grid of `cols` columns at the advance of one glyph
//! of its font (lines longer than the view is wide are clipped and
//! scroll sideways), and a window of rows over the whole scrollback
//! that scrolls with the wheel and follows the end while it is there.
//! Each row draws its backgrounds, then its text (each run of one style
//! shaped as one line, one glyph per cell), then box-drawing and block
//! characters as shapes; the cursor goes last, while the view is live.
//! Dragging selects text; [`Copy`](struct@Copy) copies the selection, or everything
//! when nothing is selected.
//!
//! The view keeps its [`Terminal`] on the thread GPUI runs on, as every
//! entity is.

use std::{cell::Cell, ops::Range, rc::Rc};

use gpui::{
    App,
    BorderStyle,
    Bounds,
    ClipboardItem,
    Context,
    CursorStyle,
    DispatchPhase,
    EventEmitter,
    FocusHandle,
    Focusable,
    Font,
    FontFeatures,
    FontStyle,
    FontWeight,
    Hsla,
    KeyBinding,
    MouseButton,
    MouseDownEvent,
    MouseMoveEvent,
    MouseUpEvent,
    Pixels,
    Point as PixelPoint,
    Render,
    Rgba,
    ScrollWheelEvent,
    ShapedLine,
    SharedString,
    StrikethroughStyle,
    TextRun as GpuiRun,
    UnderlineStyle,
    Window,
    actions,
    canvas,
    div,
    fill,
    font,
    outline,
    point,
    prelude::*,
    px,
    size,
};

use crate::{
    error::Error,
    glyph::{Arms, Glyph, Weight},
    layout::{RowLayout, char_at, layout},
    palette::Palette,
    screen::{Cursor, CursorShape, Line, Rgb, Underline},
    scroll::{ScrollTo, Scroller},
    selection::{Point, Selection, text_of},
    terminal::{Options, Size, Terminal},
};

actions!(
    terminal,
    [
        /// Copies the selection, or all the text when nothing is
        /// selected.
        Copy,
        /// Selects every row.
        SelectAll,
    ]
);

/// The key context [`TerminalView`] sets while focused.
pub const KEY_CONTEXT: &str = "Terminal";

/// Binds the terminal's keys: `secondary-c` (⌘C, or Ctrl-C off macOS)
/// and `ctrl-shift-c` copy, `secondary-a` selects all. Call it once at
/// startup; an app can bind [`Copy`](struct@Copy) and [`SelectAll`] itself instead.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-c", Copy, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-c", Copy, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-a", SelectAll, Some(KEY_CONTEXT)),
    ]);
}

/// How a [`TerminalView`] looks and how big its terminal is.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewOptions {
    /// The terminal's size: `cols` is the grid's width, `rows` the
    /// screen programs draw on. Frozen views take `cols` from their rows.
    pub size: Size,
    /// The live terminal's scrollback, in bytes (see
    /// [`Options::scrollback`]).
    pub scrollback: usize,
    /// A monospace font family.
    pub font_family: SharedString,
    pub font_size: Pixels,
    /// The line height, as a multiple of the font size.
    pub line_height: f32,
    pub palette: Palette,
    /// How many rows to show at most, scrolling through the rest; `None`
    /// shows every row.
    pub visible_rows: Option<usize>,
    /// How many rows tall the view is at least, blank ones included.
    pub min_rows: usize,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            size: Size::TOOL,
            scrollback: 64 << 20,
            font_family: if cfg!(target_os = "macos") {
                "Menlo".into()
            } else if cfg!(target_os = "windows") {
                "Consolas".into()
            } else {
                "DejaVu Sans Mono".into()
            },
            font_size: px(12.),
            line_height: 1.5,
            palette: Palette::default(),
            visible_rows: Some(24),
            min_rows: 1,
        }
    }
}

/// What a [`TerminalView`] tells its observers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEvent {
    /// The rows shown changed: scrolled, or new rows while following.
    Scrolled,
    /// The selection changed.
    Selected,
}

/// A running terminal: the emulator, and what the view knows of it.
struct Live {
    terminal: Terminal,
    /// Rows with something on them, or up to the cursor.
    used: usize,
    /// The cursor, by row among all rows.
    cursor: Option<(usize, Cursor)>,
    /// Bumped on every write, so cached rows go stale.
    generation: u64,
}

enum Source {
    Live(Box<Live>),
    /// Styled rows only: what a finished terminal showed.
    Frozen(Rc<Vec<Line>>),
}

/// The geometry of the last paint, for mapping the mouse to cells.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame {
    bounds: Bounds<Pixels>,
    cell: gpui::Size<Pixels>,
}

/// A terminal drawn with GPUI. See the [module docs](self).
pub struct TerminalView {
    source: Source,
    cols: u16,
    options: ViewOptions,
    scroller: Scroller,
    /// The first column shown, when lines are wider than the view.
    left: u16,
    selection: Option<Selection>,
    selecting: bool,
    /// Wheel travel not yet turned into whole rows and columns.
    wheel: PixelPoint<Pixels>,
    /// The live rows last fetched: the generation, the range, the rows.
    cache: Option<(u64, Range<usize>, Rc<Vec<Line>>)>,
    frame: Rc<Cell<Option<Frame>>>,
    focus: FocusHandle,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    /// A live view of an empty terminal of `options.size`.
    pub fn new(
        options: ViewOptions,
        cx: &mut Context<Self>,
    ) -> Result<Self, Error> {
        let terminal = Terminal::new(Options {
            size: options.size,
            scrollback: options.scrollback,
            record: false,
        })?;
        let mut view = Self::with_source(
            Source::Live(Box::new(Live {
                terminal,
                used: 0,
                cursor: None,
                generation: 0,
            })),
            options.size.cols,
            options,
            cx,
        );
        view.refresh()?;
        Ok(view)
    }

    /// A frozen view of `lines`, `cols` wide.
    pub fn frozen(
        lines: Vec<Line>,
        cols: u16,
        options: ViewOptions,
        cx: &mut Context<Self>,
    ) -> Self {
        let total = lines.len();
        let mut view = Self::with_source(
            Source::Frozen(Rc::new(lines)),
            cols,
            options,
            cx,
        );
        view.scroller.set_total(total);
        view
    }

    /// A frozen view of what `bytes` draw on a terminal of
    /// `options.size`: a finished program's output, or a
    /// [`Terminal::vt`] rendering.
    pub fn replay(
        bytes: &[u8],
        options: ViewOptions,
        cx: &mut Context<Self>,
    ) -> Result<Self, Error> {
        let mut view = Self::new(options, cx)?;
        view.write(bytes, cx)?;
        view.freeze(cx)?;
        Ok(view)
    }

    fn with_source(
        source: Source,
        cols: u16,
        options: ViewOptions,
        cx: &mut Context<Self>,
    ) -> Self {
        let visible = options.visible_rows.unwrap_or(usize::MAX);
        Self {
            source,
            cols,
            options,
            scroller: Scroller::new(visible),
            left: 0,
            selection: None,
            selecting: false,
            wheel: PixelPoint::default(),
            cache: None,
            frame: Rc::new(Cell::new(None)),
            focus: cx.focus_handle(),
        }
    }

    /// Feeds output the program wrote. Fails on a frozen view.
    pub fn write(
        &mut self,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) -> Result<(), Error> {
        let Source::Live(live) = &mut self.source else {
            return Err(Error::Frozen);
        };
        live.terminal.write(bytes)?;
        live.generation += 1;
        let top = self.scroller.top();
        self.refresh()?;
        if self.scroller.top() != top {
            cx.emit(TerminalEvent::Scrolled);
        }
        cx.notify();
        Ok(())
    }

    /// Drops the terminal and keeps its rows, styled, up to the last one
    /// with something on it. A frozen view stays as it is.
    pub fn freeze(&mut self, cx: &mut Context<Self>) -> Result<(), Error> {
        let Source::Live(live) = &mut self.source else {
            return Ok(());
        };
        let mut lines = live.terminal.lines(0..live.used)?;
        while lines.last().is_some_and(Line::is_blank) {
            lines.pop();
        }
        self.source = Source::Frozen(Rc::new(lines));
        self.cache = None;
        self.refresh()?;
        cx.emit(TerminalEvent::Scrolled);
        cx.notify();
        Ok(())
    }

    pub fn is_frozen(&self) -> bool {
        matches!(self.source, Source::Frozen(_))
    }

    /// The grid's width, in columns.
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// Rows there are to show: every row of a frozen view; the
    /// scrollback and the screen down to its last row with something on
    /// it (or the cursor) for a live one.
    pub fn total_rows(&self) -> usize {
        self.scroller.total()
    }

    /// The rows shown, among [`Self::total_rows`].
    pub fn visible_range(&self) -> Range<usize> {
        self.scroller.range()
    }

    /// Whether the view follows the end as rows are added.
    pub fn follows_end(&self) -> bool {
        self.scroller.follows()
    }

    /// Whether there are rows out of view to scroll to.
    pub fn scrolls(&self) -> bool {
        self.scroller.scrolls()
    }

    /// Shows at most `rows` rows, or every row with `None`.
    pub fn set_visible_rows(
        &mut self,
        rows: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.options.visible_rows == rows {
            return;
        }
        self.options.visible_rows = rows;
        self.scroller.set_visible(rows.unwrap_or(usize::MAX));
        cx.emit(TerminalEvent::Scrolled);
        cx.notify();
    }

    pub fn options(&self) -> &ViewOptions {
        &self.options
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        self.options.palette = palette;
        cx.notify();
    }

    pub fn set_font(
        &mut self,
        family: impl Into<SharedString>,
        size: Pixels,
        cx: &mut Context<Self>,
    ) {
        self.options.font_family = family.into();
        self.options.font_size = size;
        cx.notify();
    }

    pub fn scroll_to(&mut self, to: ScrollTo, cx: &mut Context<Self>) {
        if self.scroller.scroll_to(to) {
            cx.emit(TerminalEvent::Scrolled);
            cx.notify();
        }
    }

    /// Scrolls by `rows` (negative is up). Returns whether it moved.
    pub fn scroll_by(&mut self, rows: isize, cx: &mut Context<Self>) -> bool {
        let moved = self.scroller.scroll_by(rows);
        if moved {
            cx.emit(TerminalEvent::Scrolled);
            cx.notify();
        }
        moved
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn set_selection(
        &mut self,
        selection: Option<Selection>,
        cx: &mut Context<Self>,
    ) {
        if self.selection != selection {
            self.selection = selection;
            cx.emit(TerminalEvent::Selected);
            cx.notify();
        }
    }

    /// The selected text, if anything is selected.
    pub fn selected_text(&mut self) -> Result<Option<String>, Error> {
        let Some(selection) = self.selection else {
            return Ok(None);
        };
        let (start, end) = selection.ordered();
        let rows = self.rows(start.row..end.row + 1)?;
        Ok(Some(selection.text(&rows, start.row, self.cols)))
    }

    /// All the text, as [`Selection::text`] reads it: rows joined by
    /// newlines, soft wraps joined, trailing blanks left out.
    pub fn text(&mut self) -> Result<String, Error> {
        let rows = self.rows(0..self.scroller.total())?;
        Ok(text_of(&rows, self.cols))
    }

    /// The rows `range`, styled.
    pub fn rows(
        &mut self,
        range: Range<usize>,
    ) -> Result<Rc<Vec<Line>>, Error> {
        match &mut self.source {
            Source::Frozen(lines) => {
                let end = range.end.min(lines.len());
                let start = range.start.min(end);
                if start == 0 && end == lines.len() {
                    return Ok(lines.clone());
                }
                Ok(Rc::new(lines[start..end].to_vec()))
            }
            Source::Live(live) => {
                if let Some((generation, cached, rows)) = &self.cache
                    && *generation == live.generation
                    && *cached == range
                {
                    return Ok(rows.clone());
                }
                let end = range.end.min(live.used);
                let rows = Rc::new(live.terminal.lines(range.start..end)?);
                self.cache = Some((live.generation, range, rows.clone()));
                Ok(rows)
            }
        }
    }

    /// Recounts the rows after a write or a freeze.
    fn refresh(&mut self) -> Result<(), Error> {
        let total = match &mut self.source {
            Source::Frozen(lines) => lines.len(),
            Source::Live(live) => {
                let screen = live.terminal.snapshot()?;
                let history = live.terminal.history_rows()?;
                let last = screen
                    .lines
                    .iter()
                    .rposition(|line| !line.is_blank())
                    .map_or(0, |row| row + 1);
                let cursor_row = screen
                    .cursor
                    .map_or(0, |cursor| usize::from(cursor.row) + 1);
                live.cursor = screen
                    .cursor
                    .map(|cursor| (history + usize::from(cursor.row), cursor));
                live.used = history + last.max(cursor_row);
                live.used
            }
        };
        self.scroller.set_total(total);
        Ok(())
    }

    fn font(&self) -> Font {
        Font {
            features: FontFeatures::disable_ligatures(),
            ..font(self.options.font_family.clone())
        }
    }

    /// The cell under `position`, clamped to the grid and the rows.
    fn cell_at(&self, position: PixelPoint<Pixels>) -> Option<Point> {
        let frame = self.frame.get()?;
        let x = f32::from(position.x - frame.bounds.left())
            / f32::from(frame.cell.width);
        let y = f32::from(position.y - frame.bounds.top())
            / f32::from(frame.cell.height);
        let range = self.scroller.range();
        let last = self.scroller.total().checked_sub(1)?;
        let row =
            (range.start as f32 + y.floor()).clamp(0.0, last as f32) as usize;
        let col = (f32::from(self.left) + x.floor())
            .clamp(0.0, f32::from(self.cols.saturating_sub(1)))
            as u16;
        Some(Point { row, col })
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus);
        let Some(at) = self.cell_at(event.position) else {
            return;
        };
        self.selecting = true;
        let selection = if event.modifiers.shift
            && let Some(selection) = self.selection
        {
            Selection {
                head: at,
                ..selection
            }
        } else {
            Selection::at(at)
        };
        self.set_selection(Some(selection), cx);
        cx.stop_propagation();
    }

    fn drag_to(
        &mut self,
        position: PixelPoint<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        // Past the top or the bottom, the view scrolls along.
        if let Some(frame) = self.frame.get() {
            if position.y < frame.bounds.top() {
                self.scroll_by(-1, cx);
            } else if position.y > frame.bounds.bottom() {
                self.scroll_by(1, cx);
            }
        }
        if let (Some(at), Some(selection)) =
            (self.cell_at(position), self.selection)
        {
            self.set_selection(
                Some(Selection {
                    head: at,
                    ..selection
                }),
                cx,
            );
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        // A click with no drag selects nothing.
        if self
            .selection
            .is_some_and(|selection| selection.anchor == selection.head)
        {
            self.set_selection(None, cx);
        }
    }

    fn on_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(frame) = self.frame.get() else {
            return;
        };
        let delta = event.delta.pixel_delta(frame.cell.height);
        self.wheel.y += delta.y;
        self.wheel.x += delta.x;
        let rows = (self.wheel.y / frame.cell.height).trunc();
        let cols = (self.wheel.x / frame.cell.width).trunc();
        self.wheel.y -= frame.cell.height * rows;
        self.wheel.x -= frame.cell.width * cols;
        let moved = self.scroll_by(-(rows as isize), cx);
        let shown = (frame.bounds.size.width / frame.cell.width).floor() as u16;
        let max_left = self.cols.saturating_sub(shown);
        let left = (i32::from(self.left) - cols as i32)
            .clamp(0, i32::from(max_left)) as u16;
        let slid = left != self.left;
        self.left = left;
        if moved || slid {
            cx.notify();
            cx.stop_propagation();
        } else {
            // At an edge: the wheel goes on to whatever holds the view.
            self.wheel = PixelPoint::default();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = match self.selected_text() {
            Ok(Some(text)) => Ok(text),
            Ok(None) => self.text(),
            Err(error) => Err(error),
        };
        if let Ok(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn select_all(
        &mut self,
        _: &SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(last) = self.scroller.total().checked_sub(1) else {
            return;
        };
        self.set_selection(
            Some(Selection {
                anchor: Point { row: 0, col: 0 },
                head: Point {
                    row: last,
                    col: self.cols.saturating_sub(1),
                },
            }),
            cx,
        );
    }
}

/// What the view hands its canvas to draw.
struct Scene {
    rows: Rc<Vec<Line>>,
    first: usize,
    cols: u16,
    left: u16,
    cursor: Option<(usize, Cursor)>,
    selection: Option<Selection>,
    palette: Palette,
    font: Font,
    font_size: Pixels,
    cell: gpui::Size<Pixels>,
    thumb: Scroller,
}

/// A row shaped for painting.
struct ShapedRow {
    y: Pixels,
    layout: RowLayout,
    texts: Vec<(Pixels, ShapedLine)>,
    selected: Option<(u16, u16)>,
}

impl Render for TerminalView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let font = self.font();
        let font_size = self.options.font_size;
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let width = text_system
            .advance(font_id, font_size, 'm')
            .map(|advance| advance.width)
            .unwrap_or(font_size * 0.6);
        let height = (font_size * self.options.line_height).round();
        let cell = size(width, height);

        let range = self.scroller.range();
        let rows = self.rows(range.clone()).unwrap_or_default();
        let cursor = match &self.source {
            Source::Live(live) => {
                live.cursor.filter(|(_, cursor)| cursor.visible)
            }
            Source::Frozen(_) => None,
        };
        let shown = range.len().max(self.options.min_rows);
        let scene = Scene {
            rows,
            first: range.start,
            cols: self.cols,
            left: self.left,
            cursor,
            selection: self.selection,
            palette: self.options.palette,
            font,
            font_size,
            cell,
            thumb: self.scroller,
        };
        let frame = self.frame.clone();
        let this = cx.entity().downgrade();
        let selecting = self.selecting;

        div()
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all))
            .cursor(CursorStyle::IBeam)
            .w_full()
            .h(height * shown)
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(Self::on_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        frame.set(Some(Frame { bounds, cell }));
                        let shaped = prepaint(&scene, bounds, window, cx);
                        (scene, shaped)
                    },
                    move |bounds, (scene, shaped), window, cx| {
                        paint(&scene, &shaped, bounds, window, cx);
                        if selecting {
                            let moved = this.clone();
                            window.on_mouse_event(
                                move |event: &MouseMoveEvent, phase, _, cx| {
                                    if phase == DispatchPhase::Bubble {
                                        let _ = moved.update(cx, |view, cx| {
                                            view.drag_to(event.position, cx)
                                        });
                                    }
                                },
                            );
                            let released = this.clone();
                            window.on_mouse_event(
                                move |_: &MouseUpEvent, phase, _, cx| {
                                    if phase == DispatchPhase::Bubble {
                                        let _ = released
                                            .update(cx, |view, cx| {
                                                view.end_drag(cx)
                                            });
                                    }
                                },
                            );
                        }
                    },
                )
                .size_full(),
            )
    }
}

fn hsla(color: Rgb) -> Hsla {
    Rgba::from(color).into()
}

fn prepaint(
    scene: &Scene,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    _: &mut App,
) -> Vec<ShapedRow> {
    let mask = window.content_mask().bounds;
    let cell = scene.cell;
    let mut shaped = Vec::new();
    for (index, line) in scene.rows.iter().enumerate() {
        let y = bounds.top() + cell.height * index;
        // Rows outside what is on screen (a tall view in a scrolled
        // list) are not shaped.
        if y + cell.height < mask.top() || y > mask.bottom() {
            continue;
        }
        let layout = layout(line, &scene.palette);
        let mut texts = Vec::with_capacity(layout.texts.len());
        for text in &layout.texts {
            if text.col + text.cells <= scene.left {
                continue;
            }
            let x = bounds.left()
                + cell.width * (f32::from(text.col) - f32::from(scene.left));
            let mut color = hsla(text.color);
            if text.faint {
                color.a *= 0.6;
            }
            let run = GpuiRun {
                len: text.text.len(),
                font: Font {
                    weight: if text.bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if text.italic {
                        FontStyle::Italic
                    } else {
                        FontStyle::Normal
                    },
                    ..scene.font.clone()
                },
                color,
                background_color: None,
                underline: (text.underline != Underline::None).then_some(
                    UnderlineStyle {
                        thickness: px(1.),
                        color: Some(color),
                        wavy: text.underline == Underline::Curly,
                    },
                ),
                strikethrough: text.strikethrough.then_some(
                    StrikethroughStyle {
                        thickness: px(1.),
                        color: Some(color),
                    },
                ),
            };
            // One glyph per cell: a wide character spans two.
            let per_glyph =
                if text.text.chars().count() == usize::from(text.cells) {
                    cell.width
                } else {
                    cell.width * f32::from(text.cells)
                };
            let line = window.text_system().shape_line(
                SharedString::from(text.text.clone()),
                scene.font_size,
                &[run],
                Some(per_glyph),
            );
            texts.push((x, line));
        }
        let row = scene.first + index;
        shaped.push(ShapedRow {
            y,
            layout,
            texts,
            selected: scene
                .selection
                .and_then(|selection| selection.columns(row, scene.cols)),
        });
    }
    shaped
}

fn paint(
    scene: &Scene,
    rows: &[ShapedRow],
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let cell = scene.cell;
    let palette = &scene.palette;
    let x_of = |col: u16| {
        bounds.left() + cell.width * (f32::from(col) - f32::from(scene.left))
    };
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        window.paint_quad(fill(bounds, hsla(palette.background)));
        for row in rows {
            for fill_span in &row.layout.fills {
                window.paint_quad(fill(
                    Bounds::new(
                        point(x_of(fill_span.col), row.y),
                        size(
                            cell.width * f32::from(fill_span.cells),
                            cell.height,
                        ),
                    ),
                    hsla(fill_span.color),
                ));
            }
            if let Some((from, to)) = row.selected {
                let mut color = hsla(palette.selection);
                color.a = 0.55;
                window.paint_quad(fill(
                    Bounds::new(
                        point(x_of(from), row.y),
                        size(cell.width * f32::from(to - from), cell.height),
                    ),
                    color,
                ));
            }
            for shape in &row.layout.shapes {
                let mut color = hsla(shape.color);
                if shape.faint {
                    color.a *= 0.6;
                }
                let at = Bounds::new(point(x_of(shape.col), row.y), cell);
                paint_glyph(&shape.glyph, at, color, window);
            }
            for (x, line) in &row.texts {
                let _ = line.paint(point(*x, row.y), cell.height, window, cx);
            }
        }

        if let Some((row, cursor)) = scene.cursor {
            paint_cursor(
                scene,
                row,
                cursor,
                x_of(cursor.col),
                bounds,
                window,
                cx,
            );
        }

        let track = f32::from(bounds.size.height) - 8.;
        if let Some((offset, len)) = scene.thumb.thumb(track, 16.) {
            let thumb = Bounds::new(
                point(bounds.right() - px(7.), bounds.top() + px(4. + offset)),
                size(px(4.), px(len)),
            );
            window.paint_quad(
                fill(thumb, hsla(palette.scrollbar)).corner_radii(px(2.)),
            );
        }
    });
}

fn paint_cursor(
    scene: &Scene,
    row: usize,
    cursor: Cursor,
    x: Pixels,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(index) = row
        .checked_sub(scene.first)
        .filter(|index| *index < scene.rows.len())
    else {
        return;
    };
    let cell = scene.cell;
    let y = bounds.top() + cell.height * index;
    let color = hsla(scene.palette.cursor);
    let at = Bounds::new(point(x, y), cell);
    match cursor.shape {
        CursorShape::Block => {
            window.paint_quad(fill(at, color));
            // The character under it, in the ground's color.
            if let Some((text, style)) = char_at(&scene.rows[index], cursor.col)
            {
                let run = GpuiRun {
                    len: text.len(),
                    font: Font {
                        weight: if style.bold {
                            FontWeight::BOLD
                        } else {
                            FontWeight::NORMAL
                        },
                        ..scene.font.clone()
                    },
                    color: hsla(scene.palette.background),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window.text_system().shape_line(
                    text.into(),
                    scene.font_size,
                    &[run],
                    Some(cell.width),
                );
                let _ = line.paint(point(x, y), cell.height, window, cx);
            }
        }
        CursorShape::BlockHollow => {
            window.paint_quad(outline(at, color, BorderStyle::Solid));
        }
        CursorShape::Bar => {
            window.paint_quad(fill(
                Bounds::new(at.origin, size(px(2.), cell.height)),
                color,
            ));
        }
        CursorShape::Underline => {
            window.paint_quad(fill(
                Bounds::new(
                    point(x, y + cell.height - px(2.)),
                    size(cell.width, px(2.)),
                ),
                color,
            ));
        }
    }
}

/// Paints a box-drawing or block character in the cell `at`.
fn paint_glyph(
    glyph: &Glyph,
    at: Bounds<Pixels>,
    color: Hsla,
    window: &mut Window,
) {
    let scale = window.scale_factor();
    // Snapped to device pixels, so lines stay crisp and meet.
    let snap = |value: Pixels| px((f32::from(value) * scale).round() / scale);
    match glyph {
        Glyph::Rects(rects) => {
            for rect in rects {
                let x0 = snap(at.left() + at.size.width * rect.x0);
                let x1 = snap(at.left() + at.size.width * rect.x1);
                let y0 = snap(at.top() + at.size.height * rect.y0);
                let y1 = snap(at.top() + at.size.height * rect.y1);
                let mut color = color;
                color.a *= rect.alpha;
                window.paint_quad(fill(
                    Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0)),
                    color,
                ));
            }
        }
        Glyph::Lines(arms) => paint_arms(*arms, at, color, snap, window),
    }
}

fn paint_arms(
    arms: Arms,
    at: Bounds<Pixels>,
    color: Hsla,
    snap: impl Fn(Pixels) -> Pixels,
    window: &mut Window,
) {
    let light = snap(px((f32::from(at.size.width) / 8.).max(1.)));
    let light = if light < px(1.) { px(1.) } else { light };
    let cx = snap(at.left() + at.size.width / 2.);
    let cy = snap(at.top() + at.size.height / 2.);
    let (left, right, top, bottom) =
        (at.left(), at.right(), at.top(), at.bottom());
    let mut quad = |x0: Pixels, y0: Pixels, x1: Pixels, y1: Pixels| {
        window.paint_quad(fill(
            Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0)),
            color,
        ));
    };
    // Each arm runs from the edge to the far side of the center line,
    // so arms of any weight meet with no notch.
    let reach = |weight: Weight| match weight {
        Weight::None => px(0.),
        Weight::Light => light / 2.,
        Weight::Heavy | Weight::Double => light,
    };
    let across = reach(arms.up).max(reach(arms.down));
    let along = reach(arms.left).max(reach(arms.right));
    for (weight, horizontal, toward_start) in [
        (arms.left, true, true),
        (arms.right, true, false),
        (arms.up, false, true),
        (arms.down, false, false),
    ] {
        let half = match weight {
            Weight::None => continue,
            Weight::Light => light / 2.,
            Weight::Heavy => light,
            Weight::Double => light / 2.,
        };
        let offsets: &[Pixels] = match weight {
            Weight::Double => &[-light, light],
            _ => &[px(0.)],
        };
        for offset in offsets {
            if horizontal {
                let (x0, x1) = if toward_start {
                    (left, cx + across)
                } else {
                    (cx - across, right)
                };
                quad(x0, cy + *offset - half, x1, cy + *offset + half);
            } else {
                let (y0, y1) = if toward_start {
                    (top, cy + along)
                } else {
                    (cy - along, bottom)
                };
                quad(cx + *offset - half, y0, cx + *offset + half, y1);
            }
        }
    }
}

impl From<Rgb> for Rgba {
    fn from(color: Rgb) -> Self {
        Rgba {
            r: f32::from(color.r) / 255.0,
            g: f32::from(color.g) / 255.0,
            b: f32::from(color.b) / 255.0,
            a: 1.0,
        }
    }
}

impl From<Rgb> for Hsla {
    fn from(color: Rgb) -> Self {
        Rgba::from(color).into()
    }
}
