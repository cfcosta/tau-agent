//! Text the person can select and copy, as in a browser: drag across it,
//! double-click a word, and copy with ctrl+c (cmd+c on a Mac).
//!
//! GPUI draws text that cannot be selected. A [`selection_scope`] makes
//! the [`Selectable`] texts drawn inside it one document ([`init`]
//! binds the copy): each frame
//! they register their layouts in the order they are drawn, a drag runs
//! from one text to another in that order, and the selection paints
//! behind the glyphs. Outside a scope a [`Selectable`] is plain text.
//!
//! A text is known across frames by its contents and how many texts
//! with the same contents were drawn before it, so callers name none.
//! A selection only covers what is drawn: one end scrolled out of a
//! list is taken as being before the texts drawn.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AnyElement,
    App,
    Bounds,
    ClipboardItem,
    CursorStyle,
    DispatchPhase,
    Element,
    ElementId,
    GlobalElementId,
    Hitbox,
    HitboxBehavior,
    Hsla,
    InspectorElementId,
    IntoElement,
    LayoutId,
    MouseButton,
    MouseDownEvent,
    MouseMoveEvent,
    MouseUpEvent,
    Pixels,
    Point,
    StyledText,
    TextLayout,
    Window,
    fill,
    point,
};

/// Where a text is, across frames: a hash of its contents and how many
/// texts with those contents were drawn before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    hash: u64,
    nth: usize,
}

/// One end of a selection: a text and a byte offset in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct End {
    key: Key,
    index: usize,
}

/// A text drawn this frame.
struct Drawn {
    key: Key,
    layout: TextLayout,
    text: String,
    bounds: Bounds<Pixels>,
}

/// What a scope knows: the texts drawn this frame, in order, and the
/// selection.
#[derive(Default)]
pub struct State {
    drawn: Vec<Drawn>,
    anchor: Option<End>,
    head: Option<End>,
    dragging: bool,
    /// The color the selection paints in.
    color: Option<Hsla>,
}

/// The scope whose texts are being drawn now, if any.
#[derive(Default)]
struct Current(Option<Rc<RefCell<State>>>);

impl gpui::Global for Current {}

fn current(cx: &App) -> Option<Rc<RefCell<State>>> {
    cx.try_global::<Current>()
        .and_then(|current| current.0.clone())
}

fn set_current(state: Option<Rc<RefCell<State>>>, cx: &mut App) {
    cx.set_global(Current(state));
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl State {
    /// Where `end` is in this frame's order: its text's place and its
    /// offset. A text not drawn is taken as before every drawn one.
    fn place(&self, end: End) -> (isize, usize) {
        match self.drawn.iter().position(|drawn| drawn.key == end.key) {
            Some(at) => (at as isize, end.index),
            None => (-1, 0),
        }
    }

    /// The selection, in order, when it selects anything.
    fn ordered(&self) -> Option<((isize, usize), (isize, usize))> {
        let (anchor, head) = (self.anchor?, self.head?);
        let (a, b) = (self.place(anchor), self.place(head));
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        (start != end).then_some((start, end))
    }

    /// The bytes of the `at`th drawn text that are selected.
    fn range_of(&self, at: usize) -> Option<std::ops::Range<usize>> {
        let ((start_at, start), (end_at, end)) = self.ordered()?;
        let at = at as isize;
        if at < start_at || at > end_at {
            return None;
        }
        let len = self.drawn[at as usize].text.len();
        let from = if at == start_at { start.min(len) } else { 0 };
        let to = if at == end_at { end.min(len) } else { len };
        (from < to).then_some(from..to)
    }

    /// What is selected, as text: the selected part of each text, a line
    /// apart.
    fn selected_text(&self) -> Option<String> {
        let parts: Vec<&str> = (0..self.drawn.len())
            .filter_map(|at| {
                let range = self.range_of(at)?;
                self.drawn[at].text.get(range)
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    /// The text under `position`, and the offset there.
    fn hit(&self, position: Point<Pixels>) -> Option<End> {
        let drawn = self
            .drawn
            .iter()
            .find(|drawn| drawn.bounds.contains(&position))?;
        Some(End {
            key: drawn.key,
            index: index_at(drawn, position),
        })
    }

    /// The place nearest `position`, for a drag that leaves the texts:
    /// in a text level with it, the nearest across; above every text,
    /// the first's start; else the end of the lowest text above it.
    fn nearest(&self, position: Point<Pixels>) -> Option<End> {
        let level = self
            .drawn
            .iter()
            .filter(|drawn| {
                drawn.bounds.top() <= position.y
                    && position.y <= drawn.bounds.bottom()
            })
            .min_by(|a, b| {
                distance_x(a.bounds, position.x)
                    .total_cmp(&distance_x(b.bounds, position.x))
            });
        if let Some(drawn) = level {
            return Some(End {
                key: drawn.key,
                index: index_at(drawn, position),
            });
        }
        let above = self
            .drawn
            .iter()
            .filter(|drawn| drawn.bounds.bottom() < position.y)
            .max_by(|a, b| {
                f32::from(a.bounds.bottom())
                    .total_cmp(&f32::from(b.bounds.bottom()))
            });
        match above {
            Some(drawn) => Some(End {
                key: drawn.key,
                index: drawn.text.len(),
            }),
            None => self.drawn.first().map(|drawn| End {
                key: drawn.key,
                index: 0,
            }),
        }
    }
}

/// How far `x` is from `bounds` across: 0 within it.
fn distance_x(bounds: Bounds<Pixels>, x: Pixels) -> f32 {
    if x < bounds.left() {
        f32::from(bounds.left() - x)
    } else if x > bounds.right() {
        f32::from(x - bounds.right())
    } else {
        0.
    }
}

/// The offset in `drawn` nearest `position`, clamped into its lines.
fn index_at(drawn: &Drawn, position: Point<Pixels>) -> usize {
    let bounds = drawn.bounds;
    let inside = point(
        position.x.clamp(bounds.left(), bounds.right()),
        position
            .y
            .clamp(bounds.top(), bounds.bottom() - gpui::px(1.)),
    );
    let index = match drawn.layout.index_for_position(inside) {
        Ok(index) | Err(index) => index,
    };
    floor_boundary(&drawn.text, index)
}

/// `index`, moved back to a character boundary of `text`.
fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The word around `index` in `text`: a run of letters, digits and
/// underscores, else the one character there.
fn word_at(text: &str, index: usize) -> std::ops::Range<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let index = floor_boundary(text, index);
    let Some(here) = text[index..].chars().next() else {
        return index..index;
    };
    if !is_word(here) {
        return index..index + here.len_utf8();
    }
    let start = text[..index]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(index, |(at, _)| at);
    let end = text[index..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(at, _)| index + at);
    start..end
}

/// The rectangles covering bytes `range` of a laid out text.
fn rects(
    layout: &TextLayout,
    bounds: Bounds<Pixels>,
    range: std::ops::Range<usize>,
) -> Vec<Bounds<Pixels>> {
    let line = layout.line_height();
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return Vec::new();
    };
    if start.y == end.y {
        return vec![Bounds::from_corners(start, point(end.x, end.y + line))];
    }
    let mut rects = vec![Bounds::from_corners(
        start,
        point(bounds.right(), start.y + line),
    )];
    if end.y > start.y + line {
        rects.push(Bounds::from_corners(
            point(bounds.left(), start.y + line),
            point(bounds.right(), end.y),
        ));
    }
    rects.push(Bounds::from_corners(
        point(bounds.left(), end.y),
        point(end.x, end.y + line),
    ));
    rects
}

/// Makes the [`Selectable`] texts in `child` one selectable document,
/// its selection painted in `color`.
pub fn selection_scope(
    id: impl Into<ElementId>,
    color: Hsla,
    child: impl IntoElement,
) -> SelectionScope {
    SelectionScope {
        id: id.into(),
        color,
        child: child.into_any_element(),
    }
}

/// See [`selection_scope`].
pub struct SelectionScope {
    id: ElementId,
    color: Hsla,
    child: AnyElement,
}

/// The scope's state across frames, kept as its element state.
#[derive(Default)]
struct Kept(Rc<RefCell<State>>);

impl IntoElement for SelectionScope {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectionScope {
    type RequestLayoutState = ();
    type PrepaintState = Rc<RefCell<State>>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(
        &self,
    ) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let state = window.with_element_state::<Kept, _>(
            id.expect("a scope has an id"),
            |kept, _| {
                let kept = kept.unwrap_or_default();
                (kept.0.clone(), kept)
            },
        );
        state.borrow_mut().drawn.clear();
        let outer = current(cx);
        set_current(Some(state.clone()), cx);
        self.child.prepaint(window, cx);
        set_current(outer, cx);
        state
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let outer = current(cx);
        set_current(Some(state.clone()), cx);
        // The selection paints behind each text, as the text paints.
        state.borrow_mut().color = Some(self.color);
        self.child.paint(window, cx);
        set_current(outer, cx);

        let down = state.clone();
        window.on_mouse_event(
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                {
                    return;
                }
                let mut state = down.borrow_mut();
                let hit = bounds
                    .contains(&event.position)
                    .then(|| state.hit(event.position))
                    .flatten();
                let had = state.ordered().is_some();
                match hit {
                    Some(end) if event.click_count >= 2 => {
                        let drawn = state
                            .drawn
                            .iter()
                            .find(|drawn| drawn.key == end.key);
                        let word =
                            drawn.map_or(end.index..end.index, |drawn| {
                                if event.click_count >= 3 {
                                    0..drawn.text.len()
                                } else {
                                    word_at(&drawn.text, end.index)
                                }
                            });
                        state.anchor = Some(End {
                            index: word.start,
                            ..end
                        });
                        state.head = Some(End {
                            index: word.end,
                            ..end
                        });
                        state.dragging = false;
                    }
                    Some(end) => {
                        state.anchor = Some(end);
                        state.head = Some(end);
                        state.dragging = true;
                    }
                    None => {
                        state.anchor = None;
                        state.head = None;
                        state.dragging = false;
                    }
                }
                let has = state.ordered().is_some();
                drop(state);
                mark(&down, has, cx);
                if had || has {
                    window.refresh();
                }
            },
        );
        let moved = state.clone();
        window.on_mouse_event(
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let mut state = moved.borrow_mut();
                if !state.dragging {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    state.dragging = false;
                    return;
                }
                let head = state.nearest(event.position);
                if head != state.head {
                    state.head = head;
                    let has = state.ordered().is_some();
                    drop(state);
                    mark(&moved, has, cx);
                    window.refresh();
                }
            },
        );
        let up = state.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, _| {
            if phase == DispatchPhase::Bubble
                && event.button == MouseButton::Left
            {
                up.borrow_mut().dragging = false;
            }
        });
    }
}

/// The scope whose selection ctrl+c copies: the last one the person
/// selected in, while it selects anything.
#[derive(Default)]
struct Selected(Option<std::rc::Weak<RefCell<State>>>);

impl gpui::Global for Selected {}

/// Marks `state` as the scope ctrl+c copies from when it `has` a
/// selection, or as copying nothing when it was that scope.
fn mark(state: &Rc<RefCell<State>>, has: bool, cx: &mut App) {
    let selected = cx.default_global::<Selected>();
    if has {
        selected.0 = Some(Rc::downgrade(state));
    } else if selected
        .0
        .as_ref()
        .is_some_and(|weak| weak.as_ptr() == Rc::as_ptr(state))
    {
        selected.0 = None;
    }
}

/// Copies the selection with ctrl+c (cmd+c on a Mac), wherever the
/// focus is: before any field's own copy, and only while text is
/// selected, so a field's copy works once a click left the text. Call
/// it once per app.
pub fn init(cx: &mut App) {
    cx.intercept_keystrokes(|event, _, cx| {
        let keystroke = &event.keystroke;
        let copy = keystroke.key == "c"
            && keystroke.modifiers.secondary()
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.shift;
        if !copy {
            return;
        }
        let state = cx
            .try_global::<Selected>()
            .and_then(|selected| selected.0.as_ref()?.upgrade());
        let Some(text) = state.and_then(|state| state.borrow().selected_text())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.stop_propagation();
    })
    .detach();
}

/// `text`, selectable inside a [`selection_scope`].
pub fn selectable(text: StyledText) -> Selectable {
    let layout = text.layout().clone();
    Selectable {
        layout,
        child: text.into_any_element(),
        cursor: true,
    }
}

/// `child`, which draws the text `layout` lays out, selectable inside a
/// [`selection_scope`]: for a text wrapped in another element, such as
/// an `InteractiveText` with links, which keeps its own cursor.
pub fn selectable_with(
    layout: TextLayout,
    child: impl IntoElement,
) -> Selectable {
    Selectable {
        layout,
        child: child.into_any_element(),
        cursor: false,
    }
}

/// See [`selectable`].
pub struct Selectable {
    layout: TextLayout,
    child: AnyElement,
    /// Shows the text cursor over it.
    cursor: bool,
}

impl IntoElement for Selectable {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What a [`Selectable`] keeps from prepaint to paint.
pub struct Placed {
    /// Its place among the scope's texts, inside a scope.
    at: Option<usize>,
    hitbox: Option<Hitbox>,
}

impl Element for Selectable {
    type RequestLayoutState = ();
    type PrepaintState = Placed;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(
        &self,
    ) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
        let Some(state) = current(cx) else {
            return Placed {
                at: None,
                hitbox: None,
            };
        };
        let text = self.layout.text();
        let hash = hash(&text);
        let mut state = state.borrow_mut();
        let nth = state
            .drawn
            .iter()
            .filter(|drawn| drawn.key.hash == hash)
            .count();
        state.drawn.push(Drawn {
            key: Key { hash, nth },
            layout: self.layout.clone(),
            text,
            bounds,
        });
        Placed {
            at: Some(state.drawn.len() - 1),
            hitbox: self
                .cursor
                .then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal)),
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        placed: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let (Some(at), Some(state)) = (placed.at, current(cx)) {
            let (range, color) = {
                let state = state.borrow();
                (state.range_of(at), state.color)
            };
            if let (Some(range), Some(color)) = (range, color) {
                for rect in rects(&self.layout, bounds, range) {
                    window.paint_quad(fill(rect, color));
                }
            }
        }
        if let Some(hitbox) = &placed.hitbox {
            window.set_cursor_style(CursorStyle::IBeam, hitbox);
        }
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_letters_digits_and_underscores() {
        let text = "copy the snake_case2 word, é ok";
        let at = text.find("snake").unwrap() + 3;
        assert_eq!(&text[word_at(text, at)], "snake_case2");
        let comma = text.find(',').unwrap();
        assert_eq!(&text[word_at(text, comma)], ",");
        let accent = text.find('é').unwrap();
        assert_eq!(&text[word_at(text, accent + 1)], "é");
        assert_eq!(word_at(text, text.len()), text.len()..text.len());
    }
}
