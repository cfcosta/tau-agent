//! The composer: where a message is written, and the fork banner over it.

use super::*;

impl Workspace {
    /// Moves the composer's caret a row, when it has focus and a row
    /// there.
    pub(super) fn composer_row(
        &mut self,
        rows: i32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.composer.read(cx).focus_handle(cx).is_focused(window)
            && self
                .composer
                .update(cx, |input, cx| input.move_vertical(rows, cx))
    }

    pub(super) fn composer(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // On New Run the message starts a run, whatever the open run does.
        let live = self.route != Route::NewRun && self.is_live();
        let continues = self.continues_chat();
        let queued = self
            .current()
            .and_then(|run| self.queued.get(&run.id))
            .cloned();
        let send = div()
            .id("send")
            .on_click(cx.listener(|ws, _, _, cx| ws.submit_from_button(cx)));
        let send = if compact {
            send.child(ui::round_button(
                "phone-send",
                Icon::Send,
                t.accent,
                None,
                t,
            ))
        } else {
            // Inside the field, at its end: what sending does.
            send.child(
                div()
                    .h(control::SMALL)
                    .flex()
                    .items_center()
                    .gap(sp(1.5))
                    .px(sp(3.))
                    .rounded(radius::BOX)
                    .bg(t.roles.primary)
                    .text_color(t.roles.on_primary)
                    .font_weight(weight::EMPHASIS)
                    .cursor_pointer()
                    .child(ui::icon(
                        if self.forking.is_some() {
                            Icon::Fork
                        } else {
                            Icon::Send
                        },
                        IconSize::COMPACT,
                        t.roles.on_primary,
                    ))
                    .child(if self.forking.is_some() {
                        "Fork"
                    } else if live {
                        "Steer"
                    } else if continues {
                        "Send"
                    } else {
                        "Start"
                    }),
            )
        };

        // On a desktop the button sits inside the field; on a phone,
        // beside it.
        let (inside, beside) = if compact {
            (None, Some(send))
        } else {
            (Some(send), None)
        };
        div()
            .relative()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .children(self.slash_popover(compact, t, cx))
            .px(sp(if compact { 3. } else { 6. }))
            .pt(sp(if compact { 2.5 } else { 3. }))
            .pb(sp(if compact { 4.5 } else { 5.5 }))
            .when(compact, |bar| bar.chrome(ui::Edge::Bottom, t))
            // On a desktop, the transcript's column.
            .when(!compact, |bar| bar.w_full().max_w(px(808.)).mx_auto())
            .when_some(self.composer_target().filter(|_| compact), |bar, target| {
                bar.child(div().flex().child(self.model_chip(target, t, cx)))
            })
            .when_some(self.fork_banner(t, cx), |bar, banner| bar.child(banner))
            .when(!self.attachments.is_empty(), |bar| {
                bar.child(
                    div().flex().flex_wrap().gap(sp(1.5)).children(
                        self.attachments.iter().enumerate().map(|(n, file)| {
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(1.5))
                                .px(sp(2.))
                                .py(sp(1.))
                                .rounded(radius::CONTROL)
                                .key(t)
                                .typeset(Type::CAPTION)
                                .child(ui::icon(Icon::Paperclip, IconSize::SMALL, t.muted))
                                .child(file.name.clone())
                                .child(
                                    div()
                                        .id(("detach", n))
                                        .cursor_pointer()
                                        .child(ui::icon(Icon::Close, IconSize::SMALL, t.dim))
                                        .on_click(cx.listener(move |ws, _, _, cx| {
                                            ws.remove_attachment(n, cx)
                                        })),
                                )
                        }),
                    ),
                )
            })
            .when_some(queued, |bar, text| {
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .px(sp(3.))
                        .py(sp(1.75))
                        .rounded(radius::CONTROL)
                        .bg(t.blue_soft)
                        .border_1()
                        .border_color(t.blue_border)
                        .typeset(Type::CAPTION)
                        .child(ui::icon(Icon::Chevron, IconSize::COMPACT, t.blue))
                        .child(div().text_color(t.blue).child("Steer queued"))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(t.text_soft)
                                .child(format!(
                                    "\u{201c}{text}\u{201d}. Lands after this tool batch."
                                )),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    // A taller field keeps the buttons by its last line.
                    .items_end()
                    .gap(sp(2.5))
                    .when(compact && live, |row| {
                        row.child(
                            ui::round_button(
                                "phone-cancel",
                                Icon::Stop,
                                t.red,
                                Some(t.red_border),
                                t,
                            )
                            .on_click(cx.listener(|ws, _, _, cx| ws.cancel(cx))),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_end()
                            .gap(sp(2.5))
                            .min_h(control::LARGE)
                            .px(sp(3.5))
                            .py(sp(if compact { 2. } else { 1.5 }))
                            .when(compact, |field| field.well(t))
                            .when(!compact, |field| field.bg(t.panel).pr(sp(1.5)))
                            .border_1()
                            .border_color(t.border_strong)
                            .rounded(if compact { radius::FULL } else { radius::TILE })
                            .when(!compact, |field| {
                                field.child(
                                    // As tall as the chip, so it lines up
                                    // with the first line of text.
                                    div()
                                        .id("attach")
                                        .h(control::SMALL)
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .child(ui::icon(Icon::Paperclip, IconSize::BASE, t.muted))
                                        .on_click(cx.listener(|ws, _, _, cx| {
                                            ws.pick_attachments(cx)
                                        })),
                                )
                            })
                            // Its first line level with the chip beside it.
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .py(sp(0.75))
                                    .child(self.composer.clone()),
                            )
                            .when_some(
                                self.composer_target().filter(|_| !compact),
                                |field, target| {
                                    field.child(self.model_chip(target, t, cx))
                                },
                            )
                            .children(inside),
                    )
                    .children(beside),
            )
    }

    /// Above the composer while it writes a fork: the turn it forks
    /// after, with steps back and forward, and a way out.
    pub(super) fn fork_banner(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let (run, turn) = self.forking.as_ref()?;
        let run = self.run(run)?;
        let last = Self::last_fork_turn(run).max(1);
        let step = |id: &'static str, glyph: Icon, delta: i32, on: bool| {
            div()
                .id(id)
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::TAG)
                .when(on, |button| {
                    button
                        .cursor_pointer()
                        .hover(|style| style.bg(gpui::white().opacity(0.06)))
                })
                .child(ui::icon(
                    glyph,
                    IconSize::COMPACT,
                    if on { t.blue } else { t.dim },
                ))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.step_fork(delta, cx)),
                )
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .px(sp(3.))
                .py(sp(1.25))
                .rounded(radius::CONTROL)
                .bg(t.blue_soft)
                .border_1()
                .border_color(t.blue_border)
                .typeset(Type::CAPTION)
                .child(ui::icon(Icon::Fork, IconSize::COMPACT, t.blue))
                .child(div().text_color(t.blue).child("Fork"))
                .child(
                    div()
                        .text_color(t.text_soft)
                        .child(format!("{} after turn", run.title)),
                )
                .child(step("fork-earlier", Icon::Back, -1, *turn > 1))
                .child(ui::mono(
                    format!("{turn} of {last}"),
                    Type::CAPTION,
                    t.text,
                ))
                .child(step("fork-later", Icon::Chevron, 1, *turn < last))
                .child(div().text_color(t.text_soft).child("on"))
                .child(self.model_chip(PickerTarget::Fork, t, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(t.muted)
                        .child(
                            "The fork gets this turn's conversation and code.",
                        ),
                )
                .child(
                    div()
                        .id("fork-cancel")
                        .text_color(t.blue)
                        .cursor_pointer()
                        .hover(|style| style.underline())
                        .child("Cancel")
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.cancel_fork(cx)),
                        ),
                ),
        )
    }
}
