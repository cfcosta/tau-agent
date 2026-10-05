//! Selecting text (`select.rs`): a drag across the texts of a scope
//! selects from where it started to where it ended, in drawn order, and
//! ctrl+c copies exactly that; a double-click takes a word, a triple
//! click a whole text, and a click off the text clears the selection.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    Context,
    IntoElement,
    Modifiers,
    MouseButton,
    Pixels,
    Point,
    Render,
    StyledText,
    TestAppContext,
    TextLayout,
    VisualTestContext,
    Window,
    div,
    point,
    prelude::*,
    px,
    size,
};
use hegel::{TestCase, generators as gs};
use tau_ui_kit::select::{selectable, selection_scope};

/// The texts drawn, and their layouts once drawn.
type Texts = Rc<RefCell<Vec<String>>>;
type Layouts = Rc<RefCell<Vec<TextLayout>>>;

struct Doc {
    texts: Texts,
    layouts: Layouts,
}

impl Render for Doc {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> impl IntoElement {
        self.layouts.borrow_mut().clear();
        let texts: Vec<String> = self.texts.borrow().clone();
        let texts = texts.into_iter().map(|text| {
            let styled = StyledText::new(text);
            self.layouts.borrow_mut().push(styled.layout().clone());
            div().child(selectable(styled))
        });
        div().size_full().child(selection_scope(
            "doc",
            gpui::blue(),
            div()
                .w(px(400.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(texts),
        ))
    }
}

fn open(
    cx: &mut TestAppContext,
    texts: Vec<String>,
) -> (VisualTestContext, Texts, Layouts) {
    let texts = Rc::new(RefCell::new(texts));
    let layouts = Rc::new(RefCell::new(Vec::new()));
    cx.update(tau_ui_kit::select::init);
    let window = cx.add_window({
        let (texts, layouts) = (texts.clone(), layouts.clone());
        move |_, _| Doc { texts, layouts }
    });
    let cx = VisualTestContext::from_window(*window, cx);
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
    (cx, texts, layouts)
}

/// Draws `new` in place of the texts drawn, with nothing selected.
fn show(cx: &mut VisualTestContext, texts: &Texts, new: Vec<String>) {
    *texts.borrow_mut() = new;
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    // Off every text: whatever was selected goes.
    cx.simulate_click(point(px(800.), px(650.)), Modifiers::none());
    cx.run_until_parked();
}

/// The point at byte `index` of text `at`: its left edge, half a line
/// down.
fn at(layouts: &Layouts, at: usize, index: usize) -> Point<Pixels> {
    let layout = layouts.borrow()[at].clone();
    let position = layout.position_for_index(index).expect("laid out");
    point(position.x, position.y + layout.line_height() / 2.)
}

fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

fn copied(cx: &mut VisualTestContext) -> Option<String> {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
    cx.simulate_keystrokes("ctrl-c");
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .filter(|text| text != "before")
}

const WORDS: [&str; 6] = ["alpha", "beta", "gamma", "delta", "pi", "omega"];

/// A drag from any place to any other copies the text between them, in
/// drawn order whichever way it went, the texts a line apart.
#[gpui::test]
fn a_drag_copies_what_lies_between_its_ends(cx: &mut TestAppContext) {
    let (mut window, shown, layouts) = open(cx, Vec::new());
    hegel::Hegel::new(|tc: TestCase| {
        let count: usize = tc.draw(gs::integers().min_value(1).max_value(4));
        let texts: Vec<String> = (0..count)
            .map(|_| {
                let words: Vec<&str> = tc.draw(
                    gs::vecs(gs::sampled_from(WORDS.to_vec()))
                        .min_size(1)
                        .max_size(5),
                );
                words.join(" ")
            })
            .collect();
        // Some texts hold a line break of their own.
        let texts: Vec<String> = texts
            .into_iter()
            .map(|text| {
                if tc.draw(gs::booleans()) {
                    text.replacen(' ', "\n", 1)
                } else {
                    text
                }
            })
            .collect();
        let end = |tc: &TestCase| -> (usize, usize) {
            let at: usize = tc.draw(gs::integers().max_value(texts.len() - 1));
            let index: usize =
                tc.draw(gs::integers().max_value(texts[at].len()));
            (at, index)
        };
        let (from, to) = (end(&tc), end(&tc));
        show(&mut window, &shown, texts.clone());
        drag(
            &mut window,
            at(&layouts, from.0, from.1),
            at(&layouts, to.0, to.1),
        );
        let (first, last) = if from <= to { (from, to) } else { (to, from) };
        let want: Vec<&str> = (first.0..=last.0)
            .filter_map(|at| {
                let text = &texts[at];
                let s = if at == first.0 { first.1 } else { 0 };
                let e = if at == last.0 { last.1 } else { text.len() };
                (s < e).then(|| &text[s..e])
            })
            .collect();
        let want = (!want.is_empty()).then(|| want.join("\n"));
        assert_eq!(copied(&mut window), want, "{texts:?} {from:?} → {to:?}");
    })
    .settings(hegel::Settings::new().test_cases(60))
    .run();
}

/// A double-click takes the word, a triple click the whole text, and a
/// click off the text clears what was selected.
#[gpui::test]
fn clicks_take_words_and_texts(cx: &mut TestAppContext) {
    let texts = vec!["copy this_word now".to_owned(), "second".to_owned()];
    let (mut window, _, layouts) = open(cx, texts);
    let inside = at(&layouts, 0, "copy th".len());
    window.simulate_click(inside, Modifiers::none());
    window.simulate_event(gpui::MouseDownEvent {
        button: MouseButton::Left,
        position: inside,
        modifiers: Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    window.run_until_parked();
    assert_eq!(copied(&mut window).as_deref(), Some("this_word"));
    window.simulate_event(gpui::MouseDownEvent {
        button: MouseButton::Left,
        position: inside,
        modifiers: Modifiers::none(),
        click_count: 3,
        first_mouse: false,
    });
    window.run_until_parked();
    assert_eq!(copied(&mut window).as_deref(), Some("copy this_word now"));
    // Off the text, right of its column.
    window.simulate_click(point(px(700.), inside.y), Modifiers::none());
    window.run_until_parked();
    assert_eq!(copied(&mut window), None);
}
