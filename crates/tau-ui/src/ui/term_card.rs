//! A `bash` card's terminal: an inset screen with a strip on top.
//!
//! The screen is a [`TerminalView`] per card, kept by [`TermCards`]:
//! live while the command runs (fed the `term` chunks as they arrive),
//! frozen once it ends, and built frozen from the result's replay for a
//! run read back from the store. When fast compaction pruned the output,
//! the strip holds two tabs: the terminal, and the text the model saw.

use std::collections::{HashMap, HashSet};

use gpui::{
    App,
    ClipboardItem,
    Context,
    Div,
    Entity,
    Hsla,
    SharedString,
    Subscription,
    div,
    prelude::*,
    px,
};
use tau_agent::tool::RunId;
use tau_terminal::{Size, TerminalEvent, TerminalView, ViewOptions};

use super::mono;
use crate::{
    theme::{Theme, radius, sp},
    view::{SeenLine, TermOutput, ToolCard, grouped},
    workspace::Workspace,
};

type Key = (RunId, String);

/// One card's screen.
struct Screen {
    view: Entity<TerminalView>,
    /// How many of the card's bytes the view has been fed.
    fed: usize,
    _scrolled: Subscription,
}

/// The screens of the `bash` cards shown so far, and how each is open.
#[derive(Default)]
pub struct TermCards {
    screens: HashMap<Key, Screen>,
    /// Cards whose screen shows every row.
    expanded: HashSet<Key>,
    /// Pruned cards showing the text the model saw.
    model_tab: HashSet<Key>,
}

fn options(term: &TermOutput, expanded: bool, t: &Theme) -> ViewOptions {
    ViewOptions {
        size: Size {
            cols: term.cols,
            rows: term.rows,
        },
        font_family: crate::theme::MONO.into(),
        font_size: px(t.term.text.size),
        line_height: t.term.leading,
        palette: t.term.palette,
        visible_rows: (!expanded).then_some(t.term.rows),
        ..ViewOptions::default()
    }
}

/// Brings `view` up to `term`: new chunks while it runs; at the end, the
/// rest of the stream and a freeze, or the snapshot replayed.
fn feed(
    view: &Entity<TerminalView>,
    fed: &mut usize,
    term: &TermOutput,
    cx: &mut App,
) -> Result<(), tau_terminal::Error> {
    view.update(cx, |view, cx| {
        if view.is_frozen() {
            return Ok(());
        }
        let snapshot = term.end.is_some_and(|end| end.snapshot);
        if snapshot || *fed > term.bytes.len() {
            // The live stream stops here: the snapshot is the end state.
            let options = view.options().clone();
            *view = TerminalView::replay(&term.bytes, options, cx)?;
            *fed = term.bytes.len();
            return Ok(());
        }
        view.write(&term.bytes[*fed..], cx)?;
        *fed = term.bytes.len();
        if term.end.is_some() {
            view.freeze(cx)?;
        }
        Ok(())
    })
}

impl TermCards {
    /// The screen of `call_id`'s card, made on first sight from all the
    /// bytes so far.
    fn screen(
        &mut self,
        run: &RunId,
        call_id: &str,
        term: &TermOutput,
        t: &Theme,
        cx: &mut Context<Workspace>,
    ) -> Option<Entity<TerminalView>> {
        let key = (run.clone(), call_id.to_owned());
        if let Some(screen) = self.screens.get(&key) {
            return Some(screen.view.clone());
        }
        let expanded = self.expanded.contains(&key);
        let options = options(term, expanded, t);
        let mut failed = false;
        let made = cx.new(|cx| match TerminalView::new(options.clone(), cx) {
            Ok(view) => view,
            Err(_) => {
                failed = true;
                TerminalView::frozen(Vec::new(), term.cols, options, cx)
            }
        });
        if failed {
            return None;
        }
        let mut fed = 0;
        if feed(&made, &mut fed, term, cx).is_err() {
            return None;
        }
        let scrolled = cx.subscribe(&made, |_, _, event, cx| {
            if *event == TerminalEvent::Scrolled {
                cx.notify();
            }
        });
        self.screens.insert(
            key,
            Screen {
                view: made.clone(),
                fed,
                _scrolled: scrolled,
            },
        );
        Some(made)
    }

    /// Feeds a card's screen what its model gained, if it has a screen.
    pub fn sync(
        &mut self,
        run: &RunId,
        call_id: &str,
        term: &TermOutput,
        cx: &mut App,
    ) {
        let key = (run.clone(), call_id.to_owned());
        if let Some(screen) = self.screens.get_mut(&key) {
            let _ = feed(&screen.view, &mut screen.fed, term, cx);
        }
    }
}

impl Workspace {
    /// Opens a card's screen to every row, or closes it back.
    pub fn toggle_term_expanded(
        &mut self,
        run: &RunId,
        call_id: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (run.clone(), call_id.to_owned());
        let rows = self.theme_rows(cx);
        let terms = self.terms.get_mut();
        let expanded = !terms.expanded.remove(&key);
        if expanded {
            terms.expanded.insert(key.clone());
        }
        if let Some(screen) = terms.screens.get(&key) {
            screen.view.update(cx, |view, cx| {
                view.set_visible_rows((!expanded).then_some(rows), cx)
            });
        }
        cx.notify();
    }

    /// Shows the text the model saw, or the terminal, on a pruned card.
    pub fn show_model_text(
        &mut self,
        run: &RunId,
        call_id: &str,
        on: bool,
        cx: &mut Context<Self>,
    ) {
        let key = (run.clone(), call_id.to_owned());
        let terms = self.terms.get_mut();
        if on {
            terms.model_tab.insert(key);
        } else {
            terms.model_tab.remove(&key);
        }
        cx.notify();
    }

    /// Copies a card's selection, or its whole screen.
    pub fn copy_term(
        &mut self,
        run: &RunId,
        call_id: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (run.clone(), call_id.to_owned());
        let Some(view) = self
            .terms
            .get_mut()
            .screens
            .get(&key)
            .map(|s| s.view.clone())
        else {
            return;
        };
        let text = view.update(cx, |view, _| match view.selected_text() {
            Ok(Some(text)) => Ok(text),
            Ok(None) => view.text(),
            Err(error) => Err(error),
        });
        if let Ok(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn theme_rows(&self, cx: &App) -> usize {
        crate::theme::theme(cx).term.rows
    }
}

/// A small button of the strip.
fn strip_button(
    id: SharedString,
    label: &str,
    t: &Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .px(sp(1.75))
        .py(sp(0.5))
        .border_1()
        .border_color(t.border)
        .rounded(radius::SMALL)
        .cursor_pointer()
        .hover(|style| style.bg(t.raised))
        .child(mono(label.to_owned(), crate::theme::Type::MICRO, t.muted))
}

/// A tab of a pruned card's strip.
fn tab(
    id: SharedString,
    label: String,
    on: bool,
    t: &Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .px(sp(2.))
        .py(sp(0.75))
        .rounded(radius::SMALL)
        .cursor_pointer()
        .when(on, |tab| tab.bg(t.term.tab))
        .child(mono(
            label,
            crate::theme::Type::MICRO,
            if on { t.text } else { t.dim },
        ))
}

/// The card's body: the inset screen, with its strip.
pub fn body(
    ws: &Workspace,
    run: &RunId,
    card: &ToolCard,
    term: &TermOutput,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let key = (run.clone(), card.call_id.clone());
    let (view, expanded, model_tab) = {
        let mut terms = ws.terms.borrow_mut();
        (
            terms.screen(run, &card.call_id, term, t, cx),
            terms.expanded.contains(&key),
            terms.model_tab.contains(&key),
        )
    };
    let running = term.end.is_none();
    let (total, range) = view
        .as_ref()
        .map(|view| {
            let view = view.read(cx);
            (view.total_rows(), view.visible_range())
        })
        .unwrap_or_default();
    let cut = card.cut.as_ref();
    let model_tab = model_tab && cut.is_some();
    let id = |what: &str| {
        SharedString::from(format!("term-{what}-{}", card.call_id))
    };
    let micro = crate::theme::Type::MICRO;

    let copy_expand = |row: Div, cx: &mut Context<Workspace>| {
        let (copy_run, copy_call) = (run.clone(), card.call_id.clone());
        let (expand_run, expand_call) = (run.clone(), card.call_id.clone());
        row.child(strip_button(id("copy"), "copy", t).on_click(cx.listener(
            move |ws, _, _, cx| ws.copy_term(&copy_run, &copy_call, cx),
        )))
        .child(
            strip_button(
                id("expand"),
                if expanded { "collapse" } else { "expand" },
                t,
            )
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.toggle_term_expanded(&expand_run, &expand_call, cx)
            })),
        )
    };

    let strip = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .min_w(px(0.))
        .border_b_1()
        .border_color(t.term.divider);
    let strip = match cut {
        // Pruned: the terminal, or what the model saw.
        Some(cut) => {
            let path = std::path::PathBuf::from(&cut.archive);
            let (to_term_run, to_term_call) =
                (run.clone(), card.call_id.clone());
            let (to_seen_run, to_seen_call) =
                (run.clone(), card.call_id.clone());
            let strip = strip
                .gap(sp(1.))
                .px(sp(2.))
                .py(sp(1.25))
                .child(
                    tab(
                        id("tab-term"),
                        format!("terminal · {} lines", grouped(total)),
                        !model_tab,
                        t,
                    )
                    .on_click(cx.listener(
                        move |ws, _, _, cx| {
                            ws.show_model_text(
                                &to_term_run,
                                &to_term_call,
                                false,
                                cx,
                            )
                        },
                    )),
                )
                .child(
                    tab(
                        id("tab-seen"),
                        format!("model saw · {} lines", grouped(cut.kept)),
                        model_tab,
                        t,
                    )
                    .on_click(cx.listener(
                        move |ws, _, _, cx| {
                            ws.show_model_text(
                                &to_seen_run,
                                &to_seen_call,
                                true,
                                cx,
                            )
                        },
                    )),
                )
                .child(div().flex_1());
            let strip = if !model_tab && !running && !compact {
                copy_expand(strip, cx)
            } else {
                strip
            };
            strip.child(
                div()
                    .id(id("archive"))
                    .flex_shrink_0()
                    .ml(sp(1.))
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .child(mono("open full output", micro, t.blue))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.open_with_system(&path)
                    })),
            )
        }
        None => {
            let strip = strip.px(sp(2.5)).py(sp(1.5));
            let label = if running {
                format!("{} · xterm-256color", term.size_label())
            } else if total == 0 {
                format!("{} · no output", term.size_label())
            } else {
                format!(
                    "{} · {} lines · {}–{}",
                    term.size_label(),
                    grouped(total),
                    grouped(range.start + 1),
                    grouped(range.end)
                )
            };
            let strip = strip.child(
                mono(label, micro, t.term.label)
                    .flex_1()
                    .min_w(px(0.))
                    .truncate(),
            );
            if running {
                strip.child(mono("live", micro, t.term.label)).child(
                    div().size(sp(1.5)).rounded(radius::FULL).bg(t.accent),
                )
            } else if total > 0 {
                copy_expand(strip, cx)
            } else {
                strip
            }
        }
    };

    let ground = Hsla::from(t.term.palette.background);
    let screen = if model_tab {
        seen(term, card, expanded, t).into_any_element()
    } else {
        match view {
            Some(view) => div()
                .pl(sp(3.))
                .pt(sp(2.))
                .pb(sp(2.5))
                .child(view)
                .into_any_element(),
            None => seen(term, card, expanded, t).into_any_element(),
        }
    };

    div()
        .mx(sp(3.))
        .mb(sp(3.))
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.term.border)
        .rounded(radius::CONTROL)
        .bg(ground)
        .overflow_hidden()
        .child(strip)
        .child(screen)
}

/// The text the model saw, with fast compaction's marks drawn: omitted
/// lines as small chips, its header and footer quiet.
fn seen(
    term: &TermOutput,
    card: &ToolCard,
    expanded: bool,
    t: &Theme,
) -> impl IntoElement {
    let text = t.term.text;
    let row_height = px(text.size * t.term.leading);
    let lines = term.seen_lines();
    let rows = lines.len().min(t.term.rows);
    let fg = Hsla::from(t.term.palette.foreground);
    div()
        .id(SharedString::from(format!("term-seen-{}", card.call_id)))
        .px(sp(3.))
        .pt(sp(2.))
        .pb(sp(2.5))
        .when(!expanded, |list| {
            list.max_h(row_height * rows + sp(4.5)).overflow_y_scroll()
        })
        .flex()
        .flex_col()
        .children(lines.into_iter().map(move |line| {
            let row = div()
                .h(row_height)
                .flex()
                .items_center()
                .whitespace_nowrap();
            match line {
                SeenLine::Text(line) => row.child(mono(line, text, fg)),
                SeenLine::Note(line) => row.child(mono(line, text, t.dim)),
                SeenLine::Omitted(count) => row.child(
                    div()
                        .px(sp(1.5))
                        .rounded(radius::BAR)
                        .bg(t.info_surface)
                        .child(mono(
                            format!("{} lines omitted", grouped(count)),
                            text,
                            t.blue,
                        )),
                ),
            }
        }))
}
