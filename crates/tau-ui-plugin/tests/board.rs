//! The `board` widget (ADR 0048, the fifth seam): columns of cards side
//! by side, no card leaving its column or lying over another, a long
//! title wrapping inside its card; a card dragged to another column
//! carries out the board's action with the card and the two columns, and
//! one dropped where it was does nothing; too narrow for its columns, the
//! board shows them as tabs.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    Bounds,
    Context,
    IntoElement,
    Modifiers,
    MouseButton,
    Pixels,
    Render,
    TestAppContext,
    VisualTestContext,
    Window,
    div,
    prelude::*,
    px,
    size,
};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_ui_kit::theme::Theme;
use tau_ui_plugin::tree::{Act, BoardCard, BoardColumn, Draw, Node};

/// What clicks and drops asked, in order.
type Acted = Rc<RefCell<Vec<Act>>>;

struct Shown {
    board: Rc<RefCell<Node>>,
    width: Rc<RefCell<f32>>,
    acted: Acted,
}

impl Render for Shown {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::graphite();
        let acted = self.acted.clone();
        let width = *self.width.borrow();
        let mut draw = Draw::new(&theme, "t")
            .on_act(Rc::new(move |act, _| acted.borrow_mut().push(act)));
        draw.width = width;
        let drawn = draw.draw(&self.board.borrow(), cx);
        div().w(px(width)).child(drawn)
    }
}

struct Window_ {
    cx: VisualTestContext,
    board: Rc<RefCell<Node>>,
    width: Rc<RefCell<f32>>,
    acted: Acted,
}

fn open(cx: &mut TestAppContext) -> Window_ {
    let board = Rc::new(RefCell::new(Node::Empty {
        text: String::new(),
    }));
    let width = Rc::new(RefCell::new(1200.));
    let acted: Acted = Rc::default();
    let window = cx.add_window({
        let (board, width, acted) =
            (board.clone(), width.clone(), acted.clone());
        move |_, _| Shown {
            board,
            width,
            acted,
        }
    });
    let cx = VisualTestContext::from_window(*window, cx);
    cx.simulate_resize(size(px(1300.), px(2400.)));
    cx.run_until_parked();
    Window_ {
        cx,
        board,
        width,
        acted,
    }
}

impl Window_ {
    fn show(&mut self, board: Node, width: f32) {
        *self.board.borrow_mut() = board;
        *self.width.borrow_mut() = width;
        self.acted.borrow_mut().clear();
        self.cx.update(|window, _| window.refresh());
        self.cx.run_until_parked();
    }

    fn bounds(&mut self, kind: &str, id: &str) -> Option<Bounds<Pixels>> {
        self.cx.update(|window, cx| window.draw(cx).clear(cx));
        self.cx.run_until_parked();
        let selector = format!("{kind}:{id}");
        self.cx.debug_bounds(Box::leak(selector.into_boxed_str()))
    }

    fn drag(&mut self, from: Bounds<Pixels>, to: Bounds<Pixels>) {
        let (start, end) = (from.center(), to.center());
        self.cx.simulate_mouse_down(
            start,
            MouseButton::Left,
            Modifiers::none(),
        );
        // Past gpui's drag threshold, then over the column.
        let mid = gpui::point(start.x + px(12.), start.y + px(12.));
        self.cx.simulate_mouse_move(
            mid,
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        self.cx.simulate_mouse_move(
            end,
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        self.cx
            .simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        self.cx.run_until_parked();
    }
}

const WORDS: [&str; 5] = ["fix", "the", "phone", "board", "sync"];

fn board(tc: &TestCase) -> (Node, Vec<(String, Vec<String>)>) {
    let columns: usize = tc.draw(gs::integers().min_value(2).max_value(4));
    let mut shape = Vec::new();
    let mut next = 0;
    let columns: Vec<BoardColumn> = (0..columns)
        .map(|c| {
            let cards: usize = tc.draw(gs::integers().max_value(4));
            let cards: Vec<BoardCard> = (0..cards)
                .map(|_| {
                    let words: Vec<&str> = tc.draw(
                        gs::vecs(gs::sampled_from(WORDS.to_vec()))
                            .min_size(1)
                            .max_size(30),
                    );
                    next += 1;
                    BoardCard {
                        id: format!("td-{next}"),
                        children: vec![Node::Text {
                            text: words.join(" "),
                        }],
                    }
                })
                .collect();
            let id = format!("col{c}");
            shape.push((
                id.clone(),
                cards.iter().map(|c| c.id.clone()).collect(),
            ));
            BoardColumn {
                id,
                title: format!("Column {c}"),
                cards,
            }
        })
        .collect();
    (
        Node::Board {
            action: "move".into(),
            columns,
        },
        shape,
    )
}

fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left() - px(0.5)
        && inner.right() <= outer.right() + px(0.5)
        && inner.top() >= outer.top() - px(0.5)
        && inner.bottom() <= outer.bottom() + px(0.5)
}

/// Cards sit in their columns, none over another, whatever their titles'
/// length; a drag to another column asks the move, one back to its own
/// asks nothing.
#[gpui::test]
fn a_board_lays_out_its_cards_and_drags_them_between_columns(
    cx: &mut TestAppContext,
) {
    let mut window = open(cx);
    hegel::Hegel::new(|tc: TestCase| {
        let (node, shape) = board(&tc);
        window.show(node, 1200.);
        let mut all: Vec<(String, Bounds<Pixels>)> = Vec::new();
        for (column, cards) in &shape {
            let at = window
                .bounds("board-column", column)
                .unwrap_or_else(|| panic!("{column} is drawn"));
            for card in cards {
                let drawn = window
                    .bounds("board-card", card)
                    .unwrap_or_else(|| panic!("{card} is drawn"));
                assert!(inside(drawn, at), "{card} leaves {column}");
                all.push((card.clone(), drawn));
            }
        }
        for (n, (a, first)) in all.iter().enumerate() {
            for (b, second) in &all[n + 1..] {
                assert!(
                    first.intersect(second).size.height <= px(0.5)
                        || first.intersect(second).size.width <= px(0.5),
                    "{a} lies over {b}"
                );
            }
        }
        let Some((from, card)) = shape.iter().find_map(|(column, cards)| {
            Some((column.clone(), cards.first()?.clone()))
        }) else {
            return;
        };
        let to: usize = tc.draw(gs::integers().max_value(shape.len() - 1));
        let to = shape[to].0.clone();
        let dragged = window.bounds("board-card", &card).unwrap();
        let target = window.bounds("board-column", &to).unwrap();
        window.drag(dragged, target);
        let want: Vec<Value> = if to == from {
            Vec::new()
        } else {
            vec![json!({ "card": card, "from": from, "to": to })]
        };
        let got: Vec<Value> = window
            .acted
            .borrow()
            .iter()
            .filter_map(|act| match act {
                Act::Plugin { action, args } if action == "move" => {
                    Some(args.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(got, want, "{card} from {from} to {to}");
    })
    .settings(hegel::Settings::new().test_cases(40))
    .run();
}

/// Too narrow for its columns, as on a phone, a board shows one at a
/// time, as tabs named for each column and its count.
#[gpui::test]
fn a_narrow_board_shows_its_columns_as_tabs(cx: &mut TestAppContext) {
    let mut window = open(cx);
    let node = Node::Board {
        action: "move".into(),
        columns: vec![
            BoardColumn {
                id: "ready".into(),
                title: "Ready".into(),
                cards: vec![BoardCard {
                    id: "td-1".into(),
                    children: vec![Node::Text {
                        text: "Fix sync".into(),
                    }],
                }],
            },
            BoardColumn {
                id: "done".into(),
                title: "Done".into(),
                cards: Vec::new(),
            },
        ],
    };
    window.show(node, 360.);
    assert!(window.bounds("board-column", "ready").is_none());
    assert!(
        window.bounds("tab", "Ready 1").is_some(),
        "the column's tab"
    );
    assert!(window.bounds("tab", "Done 0").is_some(), "the column's tab");
}
