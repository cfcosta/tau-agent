//! A text field that works with input methods, so dead keys and composed
//! characters (ã, ç) type correctly. One line by default; a
//! [`TextInput::multiline`] field wraps, takes a new line on shift+enter,
//! and grows up to [`MAX_LINES`] rows, then scrolls to keep the caret in
//! view.
//!
//! Adapted from GPUI's `examples/input.rs` (Apache-2.0, Zed Industries):
//! restyled for the theme, and it emits [`InputEvent::Submit`] on enter.

use std::ops::Range;

use gpui::{
    App,
    Bounds,
    ClipboardItem,
    Context,
    CursorStyle,
    ElementId,
    ElementInputHandler,
    Entity,
    EntityInputHandler,
    EventEmitter,
    FocusHandle,
    Focusable,
    GlobalElementId,
    KeyBinding,
    LayoutId,
    MouseButton,
    MouseDownEvent,
    MouseMoveEvent,
    MouseUpEvent,
    PaintQuad,
    Pixels,
    Point,
    SharedString,
    Size,
    Style,
    TextAlign,
    TextRun,
    UTF16Selection,
    UnderlineStyle,
    Window,
    WrappedLine,
    actions,
    div,
    fill,
    point,
    prelude::*,
    px,
    relative,
    size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::theme;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Submit,
        Newline,
    ]
);

/// The most rows a multiline field grows to before it scrolls.
pub const MAX_LINES: usize = 10;

const CONTEXT: &str = "TextInput";

/// Binds the field's keys. [`crate::init`] calls it.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("secondary-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("secondary-v", Paste, Some(CONTEXT)),
        KeyBinding::new("secondary-c", Copy, Some(CONTEXT)),
        KeyBinding::new("secondary-x", Cut, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
        KeyBinding::new("shift-enter", Newline, Some(CONTEXT)),
    ]);
}

pub enum InputEvent {
    /// Enter was pressed; the field holds the text.
    Submit(String),
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// What the last paint laid out, one box per line of the text.
    last_lines: Vec<LineBox>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    /// How far a tall field is scrolled, to keep the caret in view.
    scroll: Pixels,
    /// Wraps, and takes a new line on shift+enter.
    multiline: bool,
    is_selecting: bool,
    /// Draw a dot for each character, for secrets.
    masked: bool,
    /// Keep the text when enter is pressed, for fields that are part of
    /// a form rather than a prompt.
    keep_on_submit: bool,
}

/// What a masked field draws for each character.
const MASK: char = '•';

impl EventEmitter<InputEvent> for TextInput {}

impl TextInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_bounds: None,
            line_height: px(20.),
            scroll: Pixels::ZERO,
            multiline: false,
            is_selecting: false,
            masked: false,
            keep_on_submit: false,
        }
    }

    /// A field for a secret: it draws dots and refuses to copy.
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// A field that wraps and takes new lines, for prompts.
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    pub fn keep_on_submit(mut self) -> Self {
        self.keep_on_submit = true;
        self
    }

    /// Replaces the text and puts the cursor at its end.
    pub fn set_text(
        &mut self,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    /// Where `offset` into the text falls in what the field draws.
    fn display_offset(&self, offset: usize) -> usize {
        if self.masked {
            self.content[..offset].chars().count() * MASK.len_utf8()
        } else {
            offset
        }
    }

    /// The offset into the text of `index` into what the field draws.
    fn text_offset(&self, index: usize) -> usize {
        if self.masked {
            self.content
                .char_indices()
                .nth(index / MASK.len_utf8())
                .map_or(self.content.len(), |(at, _)| at)
        } else {
            index
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content = SharedString::default();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.content.trim().to_owned();
        if !text.is_empty() {
            cx.emit(InputEvent::Submit(text));
            if !self.keep_on_submit {
                self.clear(cx);
            }
        }
    }

    fn newline(
        &mut self,
        _: &Newline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    /// Moves the caret a row up (`-1`) or down (`1`) in a multiline
    /// field. False when there is no row there, so the key can do
    /// something else, such as move through a menu.
    pub fn move_vertical(&mut self, rows: i32, cx: &mut Context<Self>) -> bool {
        if !self.multiline || self.last_lines.is_empty() {
            return false;
        }
        let at = locate(
            &self.last_lines,
            self.display_offset(self.cursor_offset()),
            self.line_height,
        );
        let y = at.y + self.line_height * (rows as f32 + 0.5);
        let bottom = self.last_lines.last().map_or(Pixels::ZERO, |line| {
            line.top + line.layout.size(self.line_height).height
        });
        if y < Pixels::ZERO || y >= bottom {
            return false;
        }
        let index =
            index_at(&self.last_lines, point(at.x, y), self.line_height);
        self.move_to(self.text_offset(index), cx);
        true
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(
        &mut self,
        _: &SelectLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(
        &mut self,
        _: &SelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(
        &mut self,
        _: &SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(
        &mut self,
        _: &Backspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(
        &mut self,
        _: &Delete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn paste(
        &mut self,
        _: &Paste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) =
            cx.read_from_clipboard().and_then(|item| item.text())
        {
            let text = text.replace("\r\n", "\n");
            let text = if self.multiline {
                text
            } else {
                text.replace('\n', " ")
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() && !self.multiline {
            return 0;
        }
        if position.y > bounds.bottom() && !self.multiline {
            return self.content.len();
        }
        let local = point(
            position.x - bounds.left(),
            position.y - bounds.top() + self.scroll,
        );
        self.text_offset(index_at(&self.last_lines, local, self.line_height))
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range =
                self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)
            ..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content = (self.content[0..range.start].to_owned()
            + new_text
            + &self.content[range.end..])
            .into();
        self.selected_range =
            range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content = (self.content[0..range.start].to_owned()
            + new_text
            + &self.content[range.end..])
            .into();
        self.marked_range = (!new_text.is_empty())
            .then(|| range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new| new.start + range.start..new.end + range.end)
            .unwrap_or_else(|| {
                range.start + new_text.len()..range.start + new_text.len()
            });
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if self.last_lines.is_empty() {
            return None;
        }
        let range = self.range_from_utf16(&range_utf16);
        let height = self.line_height;
        let start =
            locate(&self.last_lines, self.display_offset(range.start), height);
        let end =
            locate(&self.last_lines, self.display_offset(range.end), height);
        let top = bounds.top() + start.y - self.scroll;
        Some(Bounds::from_corners(
            point(bounds.left() + start.x, top),
            point(bounds.left() + end.x.max(start.x), top + height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        bounds.localize(&point)?;
        let local = gpui::point(
            point.x - bounds.left(),
            point.y - bounds.top() + self.scroll,
        );
        let index = index_at(&self.last_lines, local, self.line_height);
        Some(self.offset_to_utf16(self.text_offset(index)))
    }
}

/// One line of the text as last laid out: where it starts in what the
/// field draws, its wrapped layout, and how far down it sits.
struct LineBox {
    start: usize,
    layout: WrappedLine,
    top: Pixels,
}

/// Where each row of `line` starts, as offsets into it.
fn row_starts(line: &WrappedLine) -> Vec<usize> {
    let runs = &line.unwrapped_layout.runs;
    std::iter::once(0)
        .chain(
            line.wrap_boundaries()
                .iter()
                .map(|wrap| runs[wrap.run_ix].glyphs[wrap.glyph_ix].index),
        )
        .collect()
}

/// Where the caret at `index` (into what the field draws) sits, from the
/// top left of the text.
fn locate(
    lines: &[LineBox],
    index: usize,
    line_height: Pixels,
) -> Point<Pixels> {
    let Some(line) = lines.iter().rev().find(|line| line.start <= index) else {
        return Point::default();
    };
    let local = (index - line.start).min(line.layout.len());
    let rows = row_starts(&line.layout);
    let row = rows.iter().rposition(|start| *start <= local).unwrap_or(0);
    let layout = &line.layout.unwrapped_layout;
    point(
        layout.x_for_index(local) - layout.x_for_index(rows[row]),
        line.top + line_height * row as f32,
    )
}

/// The index (into what the field draws) closest to `position`, from the
/// top left of the text.
fn index_at(
    lines: &[LineBox],
    position: Point<Pixels>,
    line_height: Pixels,
) -> usize {
    let Some(line) = lines
        .iter()
        .rev()
        .find(|line| line.top <= position.y)
        .or(lines.first())
    else {
        return 0;
    };
    let rows = row_starts(&line.layout);
    let row = ((position.y - line.top) / line_height).floor().max(0.) as usize;
    let row = row.min(rows.len() - 1);
    let start = rows[row];
    let end = rows.get(row + 1).copied().unwrap_or(line.layout.len());
    let layout = &line.layout.unwrapped_layout;
    let x = position.x.max(Pixels::ZERO) + layout.x_for_index(start);
    let local = layout.closest_index_for_x(x).clamp(start, end);
    // The end of a wrapped row is where the next one starts; stay on
    // this row.
    let local = if local == end && row + 1 < rows.len() && end > start {
        end - 1
    } else {
        local
    };
    line.start + local
}

/// What the field draws, in its runs: the text (dots when masked) or the
/// placeholder, with the text being composed underlined.
fn display(
    input: &TextInput,
    window: &Window,
    dim: gpui::Hsla,
) -> (SharedString, Vec<TextRun>) {
    let content: SharedString = if input.masked {
        MASK.to_string()
            .repeat(input.content.chars().count())
            .into()
    } else {
        input.content.clone()
    };
    let style = window.text_style();
    let (text, color) = if content.is_empty() {
        (input.placeholder.clone(), dim)
    } else {
        (content, style.color)
    };
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    // Composition in a masked field is drawn plain.
    let marked = input
        .marked_range
        .as_ref()
        .filter(|_| !input.masked && !input.content.is_empty());
    let runs = match marked {
        Some(marked) => [
            TextRun {
                len: marked.start,
                ..run.clone()
            },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(UnderlineStyle {
                    color: Some(run.color),
                    thickness: px(1.0),
                    wavy: false,
                }),
                ..run.clone()
            },
            TextRun {
                len: text.len() - marked.end,
                ..run
            },
        ]
        .into_iter()
        .filter(|run| run.len > 0)
        .collect(),
        None => vec![run],
    };
    (text, runs)
}

/// Lays `text` out one box per line, wrapped at `width` when given.
fn lay_out(
    text: SharedString,
    runs: &[TextRun],
    font_size: Pixels,
    width: Option<Pixels>,
    line_height: Pixels,
    window: &Window,
) -> Vec<LineBox> {
    let Ok(shaped) = window
        .text_system()
        .shape_text(text, font_size, runs, width, None)
    else {
        return Vec::new();
    };
    let mut boxes = Vec::with_capacity(shaped.len());
    let (mut start, mut top) = (0, Pixels::ZERO);
    for layout in shaped {
        let (len, height) = (layout.len(), layout.size(line_height).height);
        boxes.push(LineBox { start, layout, top });
        start += len + 1;
        top += height;
    }
    boxes
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<LineBox>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    scroll: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(
        &self,
    ) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let line_height = window.line_height();
        let input = self.input.read(cx);
        if !input.multiline {
            style.size.height = line_height.into();
            return (window.request_layout(style, [], cx), ());
        }
        // One row for the placeholder; else as tall as the wrapped text,
        // up to MAX_LINES rows.
        if input.content.is_empty() {
            style.size.height = line_height.into();
            return (window.request_layout(style, [], cx), ());
        }
        let (text, runs) = display(input, window, theme(cx).dim);
        let font_size =
            window.text_style().font_size.to_pixels(window.rem_size());
        let layout = window.request_measured_layout(
            style,
            move |known, available, window, _| {
                let width = known.width.or(match available.width {
                    gpui::AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let rows: usize = window
                    .text_system()
                    .shape_text(text.clone(), font_size, &runs, width, None)
                    .map_or(1, |lines| {
                        lines
                            .iter()
                            .map(|line| line.wrap_boundaries().len() + 1)
                            .sum()
                    });
                Size {
                    width: width.unwrap_or_default(),
                    height: line_height * rows.clamp(1, MAX_LINES) as f32,
                }
            },
        );
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let colors = theme(cx).clone();
        let input = self.input.read(cx);
        let (text, runs) = display(input, window, colors.dim);
        let line_height = window.line_height();
        let font_size =
            window.text_style().font_size.to_pixels(window.rem_size());
        // The placeholder stays on one line, clipped if it must be.
        let width = (input.multiline && !input.content.is_empty())
            .then_some(bounds.size.width);
        let lines = lay_out(text, &runs, font_size, width, line_height, window);
        let selected = input.display_offset(input.selected_range.start)
            ..input.display_offset(input.selected_range.end);
        let caret = locate(
            &lines,
            input.display_offset(input.cursor_offset()),
            line_height,
        );

        // Keep the caret in view.
        let mut scroll = input.scroll;
        if caret.y < scroll {
            scroll = caret.y;
        }
        if caret.y + line_height > scroll + bounds.size.height {
            scroll = caret.y + line_height - bounds.size.height;
        }
        let total = lines.last().map_or(Pixels::ZERO, |line| {
            line.top + line.layout.size(line_height).height
        });
        scroll = scroll
            .min((total - bounds.size.height).max(Pixels::ZERO))
            .max(Pixels::ZERO);
        let origin = point(bounds.left(), bounds.top() - scroll);

        let cursor = selected.is_empty().then(|| {
            fill(
                Bounds::new(
                    point(origin.x + caret.x, origin.y + caret.y),
                    size(px(2.), line_height),
                ),
                colors.accent,
            )
        });
        // The selection, row by row; a line's end gets a sliver for the
        // new line it takes in.
        let mut selection = Vec::new();
        if !selected.is_empty() {
            for line in &lines {
                let rows = row_starts(&line.layout);
                let layout = &line.layout.unwrapped_layout;
                for (row, start) in rows.iter().enumerate() {
                    let last = row + 1 == rows.len();
                    let end =
                        rows.get(row + 1).copied().unwrap_or(line.layout.len());
                    let (row_start, row_end) =
                        (line.start + start, line.start + end);
                    let from = selected.start.max(row_start);
                    let to = selected.end.min(row_end);
                    let past = selected.end > row_end && last;
                    if from > to || (from == to && !past) {
                        continue;
                    }
                    let x = |index: usize| {
                        layout.x_for_index(index - line.start)
                            - layout.x_for_index(*start)
                    };
                    let right =
                        x(to) + if past { px(6.) } else { Pixels::ZERO };
                    let top = origin.y + line.top + line_height * row as f32;
                    selection.push(fill(
                        Bounds::from_corners(
                            point(origin.x + x(from), top),
                            point(origin.x + right, top + line_height),
                        ),
                        colors.blue.opacity(0.3),
                    ));
                }
            }
        }
        PrepaintState {
            lines,
            cursor,
            selection,
            scroll,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line_height = window.line_height();
        let scroll = prepaint.scroll;
        let lines = std::mem::take(&mut prepaint.lines);
        window.with_content_mask(
            Some(gpui::ContentMask { bounds }),
            |window| {
                for selection in prepaint.selection.drain(..) {
                    window.paint_quad(selection)
                }
                for line in &lines {
                    let origin =
                        point(bounds.left(), bounds.top() + line.top - scroll);
                    // A failed paint leaves the line blank for one frame;
                    // nothing to recover.
                    let _ = line.layout.paint(
                        origin,
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
                if focus_handle.is_focused(window)
                    && let Some(cursor) = prepaint.cursor.take()
                {
                    window.paint_quad(cursor);
                }
            },
        );
        self.input.update(cx, |input, _| {
            input.last_lines = lines;
            input.last_bounds = Some(bounds);
            input.line_height = line_height;
            input.scroll = scroll;
        });
    }
}

impl Render for TextInput {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::newline))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .overflow_hidden()
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
