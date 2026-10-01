//! `TerminalView` in GPUI's test app: feeding, following the end,
//! scrolling with the wheel, selecting with the mouse, copying and
//! freezing.

use gpui::{
    Entity,
    Modifiers,
    MouseButton,
    ScrollDelta,
    ScrollWheelEvent,
    TestAppContext,
    VisualTestContext,
    point,
    px,
};
use hegel::{TestCase, generators as gs};
use tau_terminal::{
    Options,
    ScrollTo,
    Size,
    Terminal,
    TerminalView,
    ViewOptions,
    text_of,
    view::{self, Copy},
};

/// The test platform's font is 0.6 em wide per cell: at 10 px, cells
/// are 6 px wide and 15 px tall.
fn options(visible: usize) -> ViewOptions {
    ViewOptions {
        size: Size { cols: 20, rows: 6 },
        font_size: px(10.),
        line_height: 1.5,
        visible_rows: Some(visible),
        ..ViewOptions::default()
    }
}

fn open(
    cx: &mut TestAppContext,
    visible: usize,
) -> (Entity<TerminalView>, &mut VisualTestContext) {
    cx.update(view::bind_keys);
    cx.add_window_view(|_, cx| TerminalView::new(options(visible), cx).unwrap())
}

fn numbered(count: usize) -> String {
    (0..count).map(|i| format!("line {i}\r\n")).collect()
}

#[gpui::test]
fn it_follows_the_end_and_scrolls_with_the_wheel(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, 4);
    view.update(cx, |view, cx| {
        view.write(numbered(10).as_bytes(), cx).unwrap()
    });
    cx.run_until_parked();
    // Ten lines and the cursor's empty row: the last four show.
    view.read_with(cx, |view, _| {
        assert_eq!(view.total_rows(), 11);
        assert_eq!(view.visible_range(), 7..11);
        assert!(view.follows_end());
    });

    // Two rows up with the wheel.
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(20.), px(20.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(30.))),
        ..Default::default()
    });
    view.read_with(cx, |view, _| {
        assert_eq!(view.visible_range(), 5..9);
        assert!(!view.follows_end());
    });

    // New output does not move a window scrolled back.
    view.update(cx, |view, cx| view.write(b"more\r\n", cx).unwrap());
    view.read_with(cx, |view, _| assert_eq!(view.visible_range(), 5..9));

    view.update(cx, |view, cx| view.scroll_to(ScrollTo::Bottom, cx));
    view.read_with(cx, |view, _| {
        assert_eq!(view.visible_range(), 8..12);
        assert!(view.follows_end());
    });
}

#[gpui::test]
fn dragging_selects_and_copy_copies(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, 4);
    view.update(cx, |view, cx| {
        view.write(b"hello world\r\nsecond", cx).unwrap()
    });
    cx.run_until_parked();

    // From "world" on the first row to "sec" on the second.
    cx.simulate_mouse_down(
        point(px(6. * 6. + 1.), px(5.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(
        point(px(6. * 2. + 1.), px(15. + 5.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(px(6. * 2. + 1.), px(20.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    let selected = view.update(cx, |view, _| view.selected_text().unwrap());
    assert_eq!(selected.as_deref(), Some("world\nsec"));

    cx.dispatch_action(Copy);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("world\nsec")
    );

    // A click with no drag clears it; copying then takes everything.
    cx.simulate_click(point(px(1.), px(1.)), Modifiers::none());
    view.read_with(cx, |view, _| assert_eq!(view.selection(), None));
    cx.dispatch_action(Copy);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("hello world\nsecond")
    );
}

#[gpui::test]
fn freezing_keeps_the_rows_and_drops_the_cursor(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, 4);
    let output = format!("{}\x1b[31mred\x1b[0m\r\n", numbered(8));
    view.update(cx, |view, cx| view.write(output.as_bytes(), cx).unwrap());
    let before = view.update(cx, |view, _| view.text().unwrap());
    view.update(cx, |view, cx| view.freeze(cx).unwrap());
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert!(view.is_frozen());
        // The empty row the cursor was on goes.
        assert_eq!(view.total_rows(), 9);
        assert_eq!(view.visible_range(), 5..9);
        assert_eq!(view.text().unwrap(), before.trim_end());
        assert!(view.write(b"x", cx).is_err());
    });
}

/// A frozen view shows what the terminal did: the same rows, styled,
/// whatever the output, with no blank rows at the end.
#[hegel::test(test_cases = 40)]
fn a_replay_matches_the_terminal(tc: TestCase) {
    let lines: Vec<String> = tc
        .draw(
            gs::vecs(
                gs::vecs(gs::sampled_from(vec!['a', ' ', '日', '─']))
                    .max_size(30),
            )
            .max_size(20),
        )
        .into_iter()
        .map(|line| line.into_iter().collect())
        .collect();
    let bytes = lines.join("\r\n").into_bytes();

    let mut terminal = Terminal::new(Options {
        size: Size { cols: 20, rows: 6 },
        scrollback: 1 << 20,
        record: false,
    })
    .unwrap();
    terminal.write(&bytes).unwrap();
    let total = terminal.total_rows().unwrap();
    let mut expected = terminal.lines(0..total).unwrap();
    while expected.last().is_some_and(|line| line.is_blank()) {
        expected.pop();
    }

    let app = TestAppContext::single();
    let cx = &mut app.clone();
    let (view, cx) = cx.add_window_view(|_, cx| {
        TerminalView::replay(&bytes, options(5), cx).unwrap()
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert_eq!(view.total_rows(), expected.len());
        let rows = view.rows(0..view.total_rows()).unwrap();
        assert_eq!(*rows, expected);
        assert_eq!(view.text().unwrap(), text_of(&expected, 20));
    });
}
