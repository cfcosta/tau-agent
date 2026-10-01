//! A `bash` card's terminal: an inset screen with a strip on top.
//!
//! The screen is a [`TerminalView`] per card, kept by [`TermCards`]:
//! live while the command runs (fed the `term` chunks as they arrive),
//! frozen once it ends, and built frozen from the result's replay for a
//! run read back from the store. When a plugin cut the output, the strip
//! holds two tabs: the terminal, and the text the model saw.

use std::collections::{HashMap, HashSet};

use gpui::{
    App,
    AppContext as _,
    ClipboardItem,
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
use tau_ui_kit::{
    components::{Material as _, mono},
    format::grouped,
    theme::{MONO, Theme, Type, radius, sp, theme},
};
use tau_ui_plugin::{CallData, Handle, OutputCut};

use super::{
    Ui,
    term::{SeenLine, TermOutput},
};

type Key = (RunId, String);

/// One card's screen.
struct Screen {
    view: Entity<TerminalView>,
    /// How many of the card's bytes the view has been fed.
    fed: usize,
    _scrolled: Subscription,
}

/// A card's output as folded so far: what its updates and result made,
/// how many updates that took, and whether the result was in.
struct Folded {
    output: TermOutput,
    updates: usize,
    ended: bool,
}

/// The screens of the `bash` cards shown so far, and how each is open.
#[derive(Default)]
pub struct TermCards {
    screens: HashMap<Key, Screen>,
    outputs: HashMap<Key, Folded>,
    /// Cards whose screen shows every row.
    expanded: HashSet<Key>,
    /// Cut cards showing the text the model saw.
    model_tab: HashSet<Key>,
}

fn options(term: &TermOutput, expanded: bool, t: &Theme) -> ViewOptions {
    ViewOptions {
        size: Size {
            cols: term.cols,
            rows: term.rows,
        },
        font_family: MONO.into(),
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
    /// The terminal a card's call drew: its result's replay once it
    /// ended, else the chunks its updates carried; `None` for a command
    /// that ran without one.
    pub fn output(
        &mut self,
        key: &Key,
        data: &CallData,
        cut: bool,
    ) -> Option<TermOutput> {
        let folded = self.outputs.get_mut(key).filter(|folded| folded.ended);
        if let Some(folded) = folded {
            return Some(folded.output.clone());
        }
        if let Some(result) = &data.result {
            let term = result.details.as_ref()?.get("term")?;
            let mut output =
                TermOutput::from_result(term, result.text.clone())?;
            output.cut = cut;
            self.outputs.insert(
                key.clone(),
                Folded {
                    output: output.clone(),
                    updates: data.updates.len(),
                    ended: true,
                },
            );
            return Some(output);
        }
        let folded =
            self.outputs.entry(key.clone()).or_insert_with(|| Folded {
                output: TermOutput::new(Size::TOOL.cols, Size::TOOL.rows),
                updates: 0,
                ended: false,
            });
        for update in data.updates.iter().skip(folded.updates) {
            if let Some(term) = update.get("term") {
                folded.output.push_chunk(term);
            }
        }
        folded.updates = data.updates.len();
        let started = folded.output.next_seq > 0;
        started.then(|| folded.output.clone())
    }

    /// The screen of `key`'s card, made on first sight from all the bytes
    /// so far, and fed what it gained since.
    fn screen(
        &mut self,
        key: &Key,
        term: &TermOutput,
        handle: &Handle,
        t: &Theme,
        cx: &mut App,
    ) -> Option<Entity<TerminalView>> {
        if let Some(screen) = self.screens.get_mut(key) {
            let _ = feed(&screen.view, &mut screen.fed, term, cx);
            return Some(screen.view.clone());
        }
        let expanded = self.expanded.contains(key);
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
        let handle = handle.clone();
        let scrolled = cx.subscribe(&made, move |_, event, cx| {
            if *event == TerminalEvent::Scrolled {
                handle.refresh(cx);
            }
        });
        self.screens.insert(
            key.clone(),
            Screen {
                view: made.clone(),
                fed,
                _scrolled: scrolled,
            },
        );
        Some(made)
    }

    /// Opens a card's screen to every row, or closes it back.
    pub fn toggle_expanded(&mut self, key: &Key, cx: &mut App) {
        let rows = theme(cx).term.rows;
        let expanded = !self.expanded.remove(key);
        if expanded {
            self.expanded.insert(key.clone());
        }
        if let Some(screen) = self.screens.get(key) {
            screen.view.update(cx, |view, cx| {
                view.set_visible_rows((!expanded).then_some(rows), cx)
            });
        }
    }

    /// Shows the text the model saw, or the terminal, on a cut card.
    pub fn show_model_text(&mut self, key: &Key, on: bool) {
        if on {
            self.model_tab.insert(key.clone());
        } else {
            self.model_tab.remove(key);
        }
    }

    /// Copies a card's selection, or its whole screen.
    pub fn copy(&self, key: &Key, cx: &mut App) {
        let Some(view) = self.screens.get(key).map(|s| s.view.clone()) else {
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
        .child(mono(label.to_owned(), Type::MICRO, t.muted))
}

/// A tab of a cut card's strip.
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
        .child(mono(label, Type::MICRO, if on { t.text } else { t.dim }))
}

/// The card's body: the inset screen, with its strip.
#[allow(clippy::too_many_arguments)]
pub fn body(
    ui: &Entity<Ui>,
    handle: &Handle,
    key: Key,
    term: &TermOutput,
    cut: Option<&OutputCut>,
    t: &Theme,
    compact: bool,
    cx: &mut App,
) -> Div {
    let (view, expanded, model_tab) = ui.update(cx, |ui, cx| {
        let view = ui.terms.screen(&key, term, handle, t, cx);
        (
            view,
            ui.terms.expanded.contains(&key),
            ui.terms.model_tab.contains(&key),
        )
    });
    let running = term.end.is_none();
    let (total, range) = view
        .as_ref()
        .map(|view| {
            let view = view.read(cx);
            (view.total_rows(), view.visible_range())
        })
        .unwrap_or_default();
    let model_tab = model_tab && cut.is_some();
    let call_id = key.1.clone();
    let id = |what: &str| SharedString::from(format!("term-{what}-{call_id}"));
    let micro = Type::MICRO;
    // A click on the strip changes the cards' state, then draws again.
    let on_terms = |f: fn(&mut TermCards, &Key, &mut App)| {
        let (ui, handle, key) = (ui.clone(), handle.clone(), key.clone());
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| {
            ui.update(cx, |ui, cx| f(&mut ui.terms, &key, cx));
            handle.refresh(cx);
        }
    };

    let copy_expand = |row: Div| {
        row.child(
            strip_button(id("copy"), "copy", t)
                .on_click(on_terms(|terms, key, cx| terms.copy(key, cx))),
        )
        .child(
            strip_button(
                id("expand"),
                if expanded { "collapse" } else { "expand" },
                t,
            )
            .on_click(on_terms(|terms, key, cx| {
                terms.toggle_expanded(key, cx)
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
        // Cut: the terminal, or what the model saw.
        Some(cut) => {
            let path = std::path::PathBuf::from(&cut.archive);
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
                    .on_click(on_terms(|terms, key, _| {
                        terms.show_model_text(key, false)
                    })),
                )
                .child(
                    tab(
                        id("tab-seen"),
                        format!("model saw · {} lines", grouped(cut.kept)),
                        model_tab,
                        t,
                    )
                    .on_click(on_terms(|terms, key, _| {
                        terms.show_model_text(key, true)
                    })),
                )
                .child(div().flex_1());
            let strip = if !model_tab && !running && !compact {
                copy_expand(strip)
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
                    .on_click(move |_, _, cx| cx.open_with_system(&path)),
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
                    div()
                        .size(sp(1.5))
                        .rounded(radius::FULL)
                        .bg(t.accent)
                        .glow(t.accent),
                )
            } else if total > 0 {
                copy_expand(strip)
            } else {
                strip
            }
        }
    };

    let ground = Hsla::from(t.term.palette.background);
    let screen = if model_tab {
        seen(term, &call_id, expanded, t).into_any_element()
    } else {
        match view {
            Some(view) => div()
                .pl(sp(3.))
                .pt(sp(2.))
                .pb(sp(2.5))
                .child(view)
                .into_any_element(),
            None => seen(term, &call_id, expanded, t).into_any_element(),
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
        .well(t)
        .bg(ground)
        .overflow_hidden()
        .child(strip)
        .child(screen)
}

/// The text the model saw, with the cut's marks drawn: omitted lines as
/// small chips, its header and footer quiet.
fn seen(
    term: &TermOutput,
    call_id: &str,
    expanded: bool,
    t: &Theme,
) -> impl IntoElement {
    let text = t.term.text;
    let row_height = px(text.size * t.term.leading);
    let lines = term.seen_lines();
    let rows = lines.len().min(t.term.rows);
    let fg = Hsla::from(t.term.palette.foreground);
    div()
        .id(SharedString::from(format!("term-seen-{call_id}")))
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
