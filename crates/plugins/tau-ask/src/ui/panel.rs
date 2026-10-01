//! The panel that takes the composer's place while a call waits for the
//! person: a tab per question and one to review, each question's
//! choices, a field for the person's own answer, a note per answer, and
//! the keys that work them.
//!
//! The panel takes the focus when it appears. With the focus on it:
//! ↑↓ move, a digit picks a row, Enter picks (and goes on, where one
//! answer is picked), Space toggles, ←→ change questions, and `n` opens
//! the note of the question in view. In a field, Esc goes back to the
//! panel; Enter leaves the note, or takes the person's own answer
//! ([`subscribe`]).

use gpui::{
    AnyElement,
    App,
    Div,
    ElementId,
    Entity,
    Focusable as _,
    KeyDownEvent,
    SharedString,
    Window,
    div,
    prelude::*,
    px,
};
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, ButtonKind, Material as _, mono},
    input::{InputEvent, TextInput},
    theme::{Design as _, IconSize, Theme, Type, control, radius, sp, weight},
};
use tau_ui_plugin::{Handle, ViewCx, points::AtRun};

use super::{Act, AskUi, Call, CallKey, Draft, Key, Then, Ui};
use crate::{Ask, Reply};

/// The keys a row shows, by its place: rows past the ninth take none.
const DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// The panel for the run's waiting call, in the composer's place; none
/// when no call waits or the run has ended.
pub fn panel(at: &AtRun, view: &mut ViewCx<'_, AskUi>) -> Option<AnyElement> {
    if !at.run.live {
        return None;
    }
    let call = view.state?.waiting()?.clone();
    let run = at.run.id.0.to_string();
    let panel = Panel {
        ui: view.ui.clone(),
        ask: call.ask.clone(),
        key: (run.clone(), call.call.clone()),
        handle: view.handle.clone(),
    };
    let take_focus = panel.ui.update(view.cx, |ui, cx| {
        // The run waits on this call only: its others' drafts are done.
        ui.drafts.retain(|(r, c), _| *r != run || *c == call.call);
        ui.draft_for(&panel.key, &call.ask);
        ui.show(&panel.key, cx);
        // The keys come to a panel as it first appears, not each time
        // its run is opened again.
        ui.focused.insert(panel.key.clone())
    });
    if take_focus {
        // The composer it replaces had the keys.
        let focus = panel.ui.read(view.cx).focus.clone();
        panel.handle.focus(&focus, view.cx);
    }
    let t = view.theme().clone();
    Some(
        panel
            .draw(&call, view.compact, &t, view.cx)
            .into_any_element(),
    )
}

/// What the panel's handlers reach: its window state, the call it
/// answers, and the way back to the host.
#[derive(Clone)]
struct Panel {
    ui: Entity<Ui>,
    ask: Ask,
    key: CallKey,
    handle: Handle,
}

impl Panel {
    /// Whether its answers went: then it takes nothing more.
    fn sent(&self, cx: &App) -> bool {
        self.ui.read(cx).sent.contains(&self.key)
    }

    /// Changes the draft, with the fields' text taken in first, and fills
    /// the fields again if the question in view changed. Nothing changes
    /// once the answers went.
    fn change(
        &self,
        cx: &mut App,
        change: impl FnOnce(&mut Draft, &Ask) -> Then,
    ) -> Then {
        if self.sent(cx) {
            return Then::Stay;
        }
        self.ui.update(cx, |ui, cx| {
            ui.show(&self.key, cx);
            let draft = ui.draft_for(&self.key, &self.ask);
            let tab = draft.tab;
            let then = change(draft, &self.ask);
            if draft.tab != tab {
                ui.fill_fields(cx);
            }
            then
        })
    }

    /// Does what follows a change: a field takes the focus, or the
    /// answers go.
    fn then(&self, then: Then, window: &mut Window, cx: &mut App) {
        let ui = self.ui.read(cx);
        let (focus, other, note) =
            (ui.focus.clone(), ui.other.clone(), ui.note.clone());
        match then {
            Then::Stay => window.focus(&focus, cx),
            Then::WriteOther => {
                window.focus(&other.read(cx).focus_handle(cx), cx)
            }
            Then::WriteNote => {
                window.focus(&note.read(cx).focus_handle(cx), cx)
            }
            Then::Send => {
                let reply = ui
                    .drafts
                    .get(&self.key)
                    .and_then(|(ask, draft)| draft.reply(ask));
                window.focus(&focus, cx);
                if let Some(reply) = reply {
                    self.send(reply, cx);
                }
            }
        }
        self.handle.refresh(cx);
    }

    /// Sends the reply, once: until the call ends or the host refuses
    /// it, the panel shows it going.
    fn send(&self, reply: Reply, cx: &mut App) {
        let first = self.ui.update(cx, |ui, _| {
            ui.refused.remove(&self.key);
            ui.sent.insert(self.key.clone())
        });
        if !first {
            return;
        }
        self.handle.act(
            Act {
                run: self.key.0.clone(),
                call: self.key.1.clone(),
                reply,
            },
            cx,
        );
        self.handle.refresh(cx);
    }

    /// A key, wherever the focus is in the panel.
    fn key_down(
        &self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        let stroke = &event.keystroke;
        let key = stroke.key.as_str();
        let held = stroke.modifiers.control
            || stroke.modifiers.alt
            || stroke.modifiers.platform
            || stroke.modifiers.function;
        let ui = self.ui.read(cx);
        let (focus, other, note) =
            (ui.focus.clone(), ui.other.clone(), ui.note.clone());
        // Enter in a field is its Submit, which `subscribe` takes; Esc
        // and the arrows up and down leave it for the panel.
        let in_note = note.read(cx).focus_handle(cx).is_focused(window);
        let in_other = other.read(cx).focus_handle(cx).is_focused(window);
        if in_note || in_other {
            if key == "escape" || (in_other && matches!(key, "up" | "down")) {
                cx.stop_propagation();
                let then = self.change(cx, |draft, _| {
                    draft.note_open = false;
                    Then::Stay
                });
                self.then(then, window, cx);
            }
            return;
        }
        if held || !focus.is_focused(window) {
            return;
        }
        let Some(key) = Key::of(key) else {
            return;
        };
        cx.stop_propagation();
        let then = self.change(cx, |draft, ask| draft.key(ask, key));
        self.then(then, window, cx);
    }

    fn draw(&self, call: &Call, compact: bool, t: &Theme, cx: &mut App) -> Div {
        let ui = self.ui.read(cx);
        let draft = ui
            .drafts
            .get(&self.key)
            .map(|(_, draft)| draft.clone())
            .expect("made as the panel is drawn");
        let sent = ui.sent.contains(&self.key);
        let refused = ui.refused.get(&self.key).cloned();
        let (focus, other, note) =
            (ui.focus.clone(), ui.other.clone(), ui.note.clone());
        let ask = &call.ask;
        let reviewing = draft.reviewing(ask);
        let keys = self.clone();
        let click = self.clone();
        let body = if reviewing {
            self.review(&draft, t).into_any_element()
        } else {
            self.question(&draft, &other, &note, compact, t)
                .into_any_element()
        };
        div()
            .track_focus(&focus)
            .on_key_down(move |event, window, cx| {
                keys.key_down(event, window, cx)
            })
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                // A click on the panel, outside its fields, gives it the
                // keys; a field's own click keeps them.
                let ui = click.ui.read(cx);
                let in_field =
                    ui.other.read(cx).focus_handle(cx).is_focused(window)
                        || ui.note.read(cx).focus_handle(cx).is_focused(window);
                if !in_field {
                    let focus = ui.focus.clone();
                    window.focus(&focus, cx);
                }
            })
            .flex_shrink_0()
            .mx(sp(if compact { 2. } else { 6. }))
            .mb(sp(if compact { 2. } else { 5. }))
            .flex()
            .flex_col()
            .rounded(radius::CARD)
            .border_1()
            .border_color(t.border_strong)
            .raised(t)
            .child(self.tabs(&draft, compact, t))
            .child(body)
            .children(refused.map(|why| {
                ui::text(why, Type::CAPTION, t.red)
                    .px(sp(4.))
                    .py(sp(2.))
                    .border_t_1()
                    .border_color(t.border)
            }))
            .child(self.foot(&draft, sent, compact, t))
    }

    /// A tab per question, by its header, then the review's.
    /// On a phone the tabs wrap, and what the question takes is left
    /// to its rows.
    fn tabs(&self, draft: &Draft, compact: bool, t: &Theme) -> Div {
        let ask = &self.ask;
        let tab = |id: ElementId,
                   label: String,
                   on: bool,
                   mark: AnyElement,
                   noted: bool,
                   to: usize| {
            let this = self.clone();
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(sp(1.5))
                .h(px(28.))
                .px(sp(2.5))
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .typeset(Type::CAPTION)
                .text_color(if on { t.text } else { t.muted })
                .when(on, |tab| {
                    tab.bg(t.selected).font_weight(weight::EMPHASIS)
                })
                .hover(|tab| tab.text_color(t.text))
                .child(mark)
                .child(label)
                .when(noted, |tab| {
                    tab.child(ui::icon(Icon::Pencil, IconSize::TINY, t.accent))
                })
                .on_click(move |_, window, cx| {
                    let then = this.change(cx, |draft, ask| {
                        draft.go(ask, to);
                        Then::Stay
                    });
                    this.then(then, window, cx);
                })
        };
        let questions =
            ask.questions.iter().enumerate().map(|(i, question)| {
                let on = draft.tab == i;
                let mark = if draft.answered(i) {
                    ui::icon(Icon::Check, IconSize::SMALL, t.green)
                        .into_any_element()
                } else {
                    div()
                        .size(px(10.))
                        .rounded(radius::HAIRLINE)
                        .border_1()
                        .border_color(if on {
                            t.accent
                        } else {
                            t.border_strong
                        })
                        .into_any_element()
                };
                tab(
                    ElementId::Name(format!("ask-tab-{i}").into()),
                    question.header.clone(),
                    on,
                    mark,
                    !draft.notes[i].trim().is_empty(),
                    i,
                )
            });
        let last = ask.questions.len();
        let mode = if draft.reviewing(ask) {
            format!("{} of {last} answered", draft.count())
        } else if ask.questions[draft.tab].multi_select {
            format!("pick any · {} chosen", draft.picked[draft.tab].len())
        } else {
            "pick one".to_owned()
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(0.5))
            .px(sp(2.5))
            .py(sp(2.))
            .border_b_1()
            .border_color(t.border)
            .children(questions)
            .child(mono("›", Type::CAPTION, t.border_strong).px(sp(1.)))
            .child(tab(
                "ask-tab-review".into(),
                "Submit".into(),
                draft.reviewing(ask),
                div().into_any_element(),
                false,
                last,
            ))
            .when(!compact, |tabs| {
                tabs.child(div().flex_1())
                    .child(ui::text(mode, Type::MICRO, t.dim).pr(sp(1.5)))
            })
    }

    /// The question in view: its rows, with the preview of the row in
    /// focus beside them when its choices have previews, and its note.
    fn question(
        &self,
        draft: &Draft,
        other: &Entity<TextInput>,
        note: &Entity<TextInput>,
        compact: bool,
        t: &Theme,
    ) -> Div {
        let i = draft.tab;
        let question = &self.ask.questions[i];
        let previews = !question.multi_select
            && question.options.iter().any(|c| c.preview.is_some());
        let rows = div()
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .p(sp(2.))
            .children(question.options.iter().enumerate().map(|(k, choice)| {
                self.row(
                    draft,
                    k,
                    &choice.label,
                    (!previews).then_some(choice.description.as_str()),
                    t,
                )
            }))
            .child(self.other_row(draft, other, t));
        let focused = question.options.get(draft.cursor[i]);
        let side = previews.then(|| {
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .p(sp(3.5))
                .bg(t.card)
                .child(ui::text(
                    focused
                        .map_or("Write your own answer.", |c| {
                            c.description.as_str()
                        })
                        .to_owned(),
                    Type::CAPTION,
                    t.muted,
                ))
                .children(
                    focused
                        .and_then(|c| c.preview.as_deref())
                        .map(|preview| ui::code_block(None, preview, t)),
                )
        });
        div()
            .flex()
            .flex_col()
            .child(
                ui::text(question.question.clone(), Type::LEAD, t.text)
                    .font_weight(weight::EMPHASIS)
                    .px(sp(4.))
                    .pt(sp(3.5))
                    .pb(sp(3.))
                    .border_b_1()
                    .border_color(t.border),
            )
            .child(match side {
                Some(side) if !compact => div()
                    .flex()
                    .child(
                        rows.w(px(300.))
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(t.border),
                    )
                    .child(side),
                Some(side) => div().flex().flex_col().child(rows).child(side),
                None => rows,
            })
            .when(draft.note_open, |body| {
                body.child(self.note(draft, note, compact, t))
            })
            .when(
                !draft.note_open && !draft.notes[i].trim().is_empty(),
                |body| body.child(self.note_line(&draft.notes[i], t)),
            )
    }

    /// Row `k` of the question in view: its key or box, its label, and
    /// its description when no preview shows it.
    fn row(
        &self,
        draft: &Draft,
        k: usize,
        label: &str,
        description: Option<&str>,
        t: &Theme,
    ) -> impl IntoElement {
        let i = draft.tab;
        let multi = self.ask.questions[i].multi_select;
        let picked = draft.picked[i].contains(&k);
        let this = self.clone();
        let lead = if multi {
            ui::checkbox(picked, false, t).into_any_element()
        } else {
            self.digit(k, picked, t).into_any_element()
        };
        let recommended = k == 0 && label.ends_with("(Recommended)");
        div()
            .id(ElementId::Name(format!("ask-row-{i}-{k}").into()))
            .flex()
            .items_start()
            .gap(sp(3.))
            .min_h(px(40.))
            .px(sp(2.5))
            .py(sp(2.25))
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .when(draft.cursor[i] == k, |row| row.bg(t.selected))
            .hover(|row| row.bg(t.selected))
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.5))
                    .child(
                        ui::text(
                            label.to_owned(),
                            Type::SMALL,
                            if picked { t.text } else { t.text_soft },
                        )
                        .font_weight(weight::EMPHASIS)
                        .when(recommended, |label| {
                            label.text_color(if picked {
                                t.accent
                            } else {
                                t.text_soft
                            })
                        }),
                    )
                    .children(description.map(|d| {
                        ui::text(d.to_owned(), Type::CAPTION, t.muted)
                    })),
            )
            .on_click(move |_, window, cx| {
                let then =
                    this.change(cx, |draft, ask| draft.choose(ask, k, false));
                this.then(then, window, cx);
            })
    }

    /// The row of the person's own answer: its key or box, and its field.
    fn other_row(
        &self,
        draft: &Draft,
        other: &Entity<TextInput>,
        t: &Theme,
    ) -> impl IntoElement {
        let i = draft.tab;
        let k = self.ask.questions[i].options.len();
        let multi = self.ask.questions[i].multi_select;
        let on = draft.other_on[i];
        let this = self.clone();
        let lead = if multi {
            ui::checkbox(on, false, t).into_any_element()
        } else {
            self.digit(k, on && draft.answered(i), t).into_any_element()
        };
        div()
            .id(ElementId::Name(format!("ask-other-{i}").into()))
            .flex()
            .items_center()
            .gap(sp(3.))
            .min_h(px(40.))
            .px(sp(2.5))
            .rounded(radius::CONTROL)
            .when(draft.cursor[i] == k, |row| row.bg(t.selected))
            .child(div().cursor_pointer().child(lead).on_mouse_down(
                gpui::MouseButton::Left,
                move |_, window, cx| {
                    cx.stop_propagation();
                    let then = this
                        .change(cx, |draft, ask| draft.choose(ask, k, false));
                    this.then(then, window, cx);
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .px(sp(2.))
                    .rounded(radius::CONTROL)
                    .well(t)
                    .typeset(Type::SMALL)
                    .text_color(t.text)
                    .child(other.clone()),
            )
    }

    /// A row's key, lit when it is the one picked.
    fn digit(&self, k: usize, picked: bool, t: &Theme) -> Div {
        let label = DIGITS.get(k).copied().unwrap_or("·");
        let hint = ui::key_hint(label, t);
        if picked {
            hint.border_color(t.accent_border)
                .bg(t.accent_soft)
                .text_color(t.accent)
        } else {
            hint
        }
    }

    /// The note of the question in view, open to write; its keys, on a
    /// computer.
    fn note(
        &self,
        draft: &Draft,
        note: &Entity<TextInput>,
        compact: bool,
        t: &Theme,
    ) -> Div {
        let i = draft.tab;
        let answer = draft.answer(&self.ask, i).text();
        let label = if answer.is_empty() {
            format!("Note on {}", self.ask.questions[i].header)
        } else {
            format!("Note on “{answer}”")
        };
        div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .px(sp(4.))
            .py(sp(3.))
            .border_t_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(ui::icon(Icon::Pencil, IconSize::SMALL, t.accent))
                    .child(ui::text(label, Type::CAPTION, t.muted))
                    .child(div().flex_1())
                    .when(!compact, |head| {
                        head.child(hint_row(
                            &[
                                ("Enter", "save"),
                                ("Shift+Enter", "new line"),
                                ("Esc", "close"),
                            ],
                            t,
                        ))
                    }),
            )
            .child(
                div()
                    .min_h(px(52.))
                    .px(sp(2.5))
                    .py(sp(2.))
                    .rounded(radius::BOX)
                    .well(t)
                    .border_1()
                    .border_color(t.accent_border)
                    .typeset(Type::SMALL)
                    .text_color(t.text)
                    .child(note.clone()),
            )
    }

    /// A saved note, closed: a click opens it.
    fn note_line(&self, text: &str, t: &Theme) -> impl IntoElement {
        let this = self.clone();
        div()
            .id("ask-note-line")
            .flex()
            .items_start()
            .gap(sp(2.))
            .px(sp(4.))
            .py(sp(2.5))
            .border_t_1()
            .border_color(t.border)
            .cursor_pointer()
            .hover(|line| line.bg(t.selected))
            .child(ui::icon(Icon::Pencil, IconSize::TINY, t.accent).mt(sp(0.5)))
            .child(ui::text(text.trim().to_owned(), Type::CAPTION, t.muted))
            .on_click(move |_, window, cx| {
                let then =
                    this.change(cx, |draft, ask| draft.key(ask, Key::Note));
                this.then(then, window, cx);
            })
    }

    /// Every answer with its note; a click goes back to its question.
    fn review(&self, draft: &Draft, t: &Theme) -> Div {
        let rows =
            self.ask.questions.iter().enumerate().map(|(i, question)| {
                let answer = draft.answer(&self.ask, i);
                let text = answer.text();
                let this = self.clone();
                div()
                    .id(ElementId::Name(format!("ask-review-{i}").into()))
                    .flex()
                    .items_start()
                    .gap(sp(3.))
                    .px(sp(3.))
                    .py(sp(2.5))
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .hover(|row| row.bg(t.selected))
                    .child(
                        ui::text(question.header.clone(), Type::CAPTION, t.dim)
                            .w(px(96.))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(sp(1.5))
                            .child(if text.is_empty() {
                                ui::text("Not answered", Type::SMALL, t.dim)
                            } else {
                                ui::text(text, Type::SMALL, t.text)
                                    .font_weight(weight::EMPHASIS)
                            })
                            .children(answer.note.map(|note| {
                                div()
                                    .flex()
                                    .items_start()
                                    .gap(sp(2.))
                                    .child(
                                        ui::icon(
                                            Icon::Pencil,
                                            IconSize::TINY,
                                            t.accent,
                                        )
                                        .mt(sp(0.5)),
                                    )
                                    .child(ui::text(
                                        note,
                                        Type::CAPTION,
                                        t.muted,
                                    ))
                            })),
                    )
                    .child(ui::text("edit", Type::MICRO, t.dim))
                    .on_click(move |_, window, cx| {
                        let then = this.change(cx, |draft, ask| {
                            draft.go(ask, i);
                            Then::Stay
                        });
                        this.then(then, window, cx);
                    })
            });
        div()
            .flex()
            .flex_col()
            .child(
                ui::text("Review your answers", Type::LEAD, t.text)
                    .font_weight(weight::EMPHASIS)
                    .px(sp(4.))
                    .pt(sp(3.5))
                    .pb(sp(3.))
                    .border_b_1()
                    .border_color(t.border),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(0.5))
                    .p(sp(2.))
                    .children(rows),
            )
    }

    /// The keys, and the buttons: Note and Next on a question; Decline
    /// and Send on the review.
    fn foot(&self, draft: &Draft, sent: bool, compact: bool, t: &Theme) -> Div {
        let ask = &self.ask;
        let reviewing = draft.reviewing(ask);
        let keys: &[(&'static str, &'static str)] = if reviewing {
            &[("←", "back"), ("Enter", "send")]
        } else if ask.questions[draft.tab].multi_select {
            &[
                ("↑↓", "move"),
                ("Space", "toggle"),
                ("n", "note"),
                ("←→", "questions"),
            ]
        } else {
            &[
                ("↑↓", "move"),
                ("Enter", "choose"),
                ("n", "note"),
                ("←→", "questions"),
            ]
        };
        let button = |id: &'static str,
                      label: String,
                      kind: ButtonKind,
                      then: fn(&mut Draft, &Ask) -> Then| {
            let this = self.clone();
            div().id(id).child(ui::button(label, kind, t)).on_click(
                move |_, window, cx| {
                    let then = this.change(cx, then);
                    this.then(then, window, cx);
                },
            )
        };
        let left = ask.questions.len() - draft.count();
        // A phone has no Cancel beside the panel: the run's header has
        // it on a computer.
        let cancel = (compact && !sent).then(|| {
            let (handle, run) = (self.handle.clone(), self.key.0.clone());
            div()
                .id("ask-cancel")
                .child(ui::button("Cancel run", ButtonKind::Danger, t))
                .on_click(move |_, _, cx| {
                    handle.cancel(
                        &tau_agent::tool::RunId(run.as_str().into()),
                        cx,
                    )
                })
        });
        let actions: Vec<AnyElement> = if sent {
            vec![
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(ui::icon(Icon::Spinner, IconSize::SMALL, t.accent))
                    .child(ui::text(
                        "Sending your answers…",
                        Type::CAPTION,
                        t.muted,
                    ))
                    .into_any_element(),
            ]
        } else if reviewing {
            let decline = self.clone();
            vec![
                div()
                    .id("ask-decline")
                    .child(ui::button(
                        "Decline to answer",
                        ButtonKind::Secondary,
                        t,
                    ))
                    .on_click(move |_, _, cx| decline.send(Reply::Declined, cx))
                    .into_any_element(),
                if left == 0 {
                    button(
                        "ask-send",
                        "Send answers".into(),
                        ButtonKind::Primary,
                        |_, _| Then::Send,
                    )
                    .into_any_element()
                } else {
                    ui::button(
                        format!("Send answers · {left} left"),
                        ButtonKind::Secondary,
                        t,
                    )
                    .opacity(0.5)
                    .into_any_element()
                },
            ]
        } else {
            let noted = !draft.notes[draft.tab].trim().is_empty();
            vec![
                button(
                    "ask-note",
                    if noted {
                        "Edit note".into()
                    } else {
                        "Add note".into()
                    },
                    ButtonKind::Secondary,
                    |draft, ask| draft.key(ask, Key::Note),
                )
                .into_any_element(),
                button(
                    "ask-next",
                    "Next".into(),
                    ButtonKind::Primary,
                    |draft, ask| {
                        draft.go(ask, draft.tab + 1);
                        Then::Stay
                    },
                )
                .into_any_element(),
            ]
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(2.5))
            .min_h(control::MEDIUM)
            .px(sp(3.))
            .py(sp(2.5))
            .border_t_1()
            .border_color(t.border)
            .when(!compact, |foot| foot.child(hint_row(keys, t)))
            .children(cancel)
            .child(div().flex_1())
            .children(actions)
    }
}

/// Keys and what they do, in a line.
fn hint_row(keys: &[(&'static str, &'static str)], t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .children(keys.iter().map(|(key, does)| {
            div()
                .flex()
                .items_center()
                .gap(sp(1.25))
                .child(ui::key_hint(key, t))
                .child(ui::text(SharedString::from(*does), Type::MICRO, t.dim))
        }))
}

/// Enter in the fields: the note closes, and the person's own answer is
/// taken (a one-answer question goes on); either way the keys go back
/// to the panel.
pub fn subscribe(
    other: &Entity<TextInput>,
    note: &Entity<TextInput>,
    handle: Handle,
    cx: &mut gpui::Context<Ui>,
) {
    let back = handle.clone();
    cx.subscribe(note, move |ui: &mut Ui, _, event: &InputEvent, cx| {
        let InputEvent::Submit(text) = event;
        let shown = ui.shown.clone();
        if let Some((ask, draft)) =
            shown.and_then(|key| ui.drafts.get_mut(&key))
        {
            draft.write_note(ask, text);
            draft.note_open = false;
        }
        back.focus(&ui.focus, cx);
        back.refresh(cx);
    })
    .detach();
    cx.subscribe(other, move |ui: &mut Ui, _, event: &InputEvent, cx| {
        let InputEvent::Submit(text) = event;
        let Some(key) = ui.shown.clone() else {
            return;
        };
        if ui.sent.contains(&key) {
            return;
        }
        let Some((ask, draft)) = ui.drafts.get_mut(&key) else {
            return;
        };
        let tab = draft.tab;
        draft.write_other(ask, text);
        draft.other_done(ask);
        if draft.tab != tab {
            ui.fill_fields(cx);
        }
        handle.focus(&ui.focus, cx);
        handle.refresh(cx);
    })
    .detach();
}
