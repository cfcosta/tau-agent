//! A table's cells hold all their lines: the text drawn in a cell never
//! runs past the box the layout gave it, at any width.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AnyElement,
    Context,
    IntoElement,
    Render,
    StyledText,
    TestAppContext,
    TextLayout,
    Window,
    div,
    prelude::*,
    px,
    size,
};
use tau_ui::{markdown::Align, theme::theme, ui};

const HEAD: [&str; 3] = ["Area", "Linked Pi extension", "Ours"];
const ROWS: [[&str; 3]; 3] = [
    [
        "Small savings",
        "Applies pruning even if the reduction is small; the 25% threshold \
         decides whether to replace Pi's summary compaction when compaction \
         is due.",
        "Declines the rewrite entirely below 25%, allowing our \
         summary-compaction plugin to try next.",
    ],
    [
        "What Jev sees",
        "Conversation text, tool inputs, and result size/status—not result \
         contents. Fits that state to a budget and batches questions.",
        "Same approach and default budgets.",
    ],
    [
        "Recovery and controls",
        "Rebuilds the ledger from session-branch entries, provides settings, \
         file configuration and a status command that reports what it kept.",
        "Stores ledger details with the rewrite for forks; configures the \
         Rust plugin in code and reports it.",
    ],
];

struct Table {
    cells: Rc<RefCell<Vec<(String, TextLayout)>>>,
}

impl Render for Table {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx).clone();
        self.cells.borrow_mut().clear();
        let cells = self.cells.clone();
        let cell = |text: &str| -> AnyElement {
            let styled = StyledText::new(text.to_owned());
            cells
                .borrow_mut()
                .push((text.to_owned(), styled.layout().clone()));
            styled.into_any_element()
        };
        let head = HEAD.iter().map(|text| cell(text)).collect();
        let rows = ROWS
            .iter()
            .map(|row| row.iter().map(|text| cell(text)).collect())
            .collect();
        // As the transcript nests a reply: a scrolling column, the
        // reply's column, then the table.
        div()
            .id("transcript")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .px(px(24.))
                    .py(px(20.))
                    .child(div().w_full().child(
                        div().flex().flex_col().gap(px(12.)).child(ui::table(
                            &[Align::Left; 3],
                            head,
                            rows,
                            &t,
                        )),
                    )),
            )
    }
}

#[gpui::test]
fn every_cell_holds_its_lines(cx: &mut TestAppContext) {
    cx.update(tau_ui::init);
    let cells = Rc::new(RefCell::new(Vec::new()));
    let window = cx.add_window({
        let cells = cells.clone();
        move |_, _| Table { cells }
    });
    let cx = gpui::VisualTestContext::from_window(*window, cx);
    let mut wrapped = false;
    for width in (360..1100).step_by(9) {
        cx.simulate_resize(size(px(width as f32), px(900.)));
        cx.run_until_parked();
        for (text, layout) in cells.borrow().iter() {
            let bounds = layout.bounds();
            let last = layout
                .position_for_index(text.len())
                .expect("the text was laid out");
            let bottom = last.y + layout.line_height();
            wrapped |= last.y > bounds.top() + px(1.);
            assert!(
                bottom <= bounds.bottom() + px(0.5),
                "at {width} px, {text:?} draws to {bottom:?} past its box's \
                 bottom {:?}",
                bounds.bottom()
            );
        }
    }
    assert!(wrapped, "no cell wrapped: the test measures nothing");
}
