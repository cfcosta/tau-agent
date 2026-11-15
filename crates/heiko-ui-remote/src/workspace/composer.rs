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
        let queued = self
            .shown_run()
            .and_then(|run| self.queued.get(&run.id))
            .map(|texts| texts.join(" · "));
        let send = div()
            .id("send")
            .on_click(cx.listener(|ws, _, _, cx| ws.submit_from_button(cx)));
        let send = if compact {
            // The phone's one printed block, as tall as a finger needs.
            send.child(
                div().id("phone-send").child(
                    ui::primary_block(
                        if self.forking.is_some() {
                            "Fork"
                        } else {
                            "Send"
                        },
                        ui::BlockSize::Large,
                        t,
                    )
                    .h(control::LARGE),
                ),
            )
        } else {
            // The one printed block of the chat. The field's placeholder
            // says what sending does.
            send.child(
                ui::primary_block(
                    if self.forking.is_some() {
                        "Fork"
                    } else {
                        "Send"
                    },
                    ui::BlockSize::Medium,
                    t,
                )
                .child(ui::mono(
                    "\u{21b5}",
                    Type::CAPTION,
                    t.roles.on_primary,
                )),
            )
        };
        // What is attached, a ruled row each: an image shows itself, a
        // file its name.
        let attached: Vec<_> = self
            .attachments
            .iter()
            .enumerate()
            .map(|(n, file)| {
                let picture = match &file.content {
                    crate::attach::Attached::Image { shown, .. } => Some(
                        gpui::img(shown.clone())
                            .h(rems(2.25))
                            .max_w(rems(4.))
                            .object_fit(gpui::ObjectFit::Cover),
                    ),
                    crate::attach::Attached::Text(_) => None,
                };
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.5))
                    .py(sp(2.5))
                    .border_b_1()
                    .border_color(t.hairline)
                    .children(picture)
                    .child(
                        ui::mono(file.name.clone(), Type::SMALL, t.text)
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate(),
                    )
                    .child(
                        div()
                            .id(("detach", n))
                            .typeset(Type::SMALL)
                            .child(ui::link_action("remove", t))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.remove_attachment(n, cx)
                            })),
                    )
            })
            .collect();
        let field = if compact {
            // On a phone: the message in large type under the rule, then
            // the model, the paperclip, stop while the run goes, and
            // Send.
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div()
                        .typeset(Type::LEAD.sized(17.).leading(1.4))
                        .py(sp(1.))
                        .child(self.composer.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(3.))
                        .when_some(self.composer_target(), |row, target| {
                            row.child(self.model_chip(target, t, cx))
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("attach")
                                .size(control::LARGE)
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .child(ui::icon(
                                    Icon::Paperclip,
                                    IconSize::LARGE,
                                    t.muted,
                                ))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.pick_attachments(cx)
                                })),
                        )
                        .when(live, |row| {
                            row.child(
                                ui::square_button(
                                    "phone-cancel",
                                    Icon::Stop,
                                    t.red,
                                    Some(t.red_border),
                                    t,
                                )
                                .on_click(
                                    cx.listener(|ws, _, _, cx| ws.cancel(cx)),
                                ),
                            )
                        })
                        .child(send),
                )
        } else {
            // On a desktop: the message in large type, then who it goes
            // to, what else the field takes, and Send.
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(
                    div()
                        .typeset(Type::LEAD.sized(19.).leading(1.4))
                        .py(sp(1.))
                        .child(self.composer.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        // The model and effort, as Home words them.
                        .when_some(self.composer_target(), |row, target| {
                            row.child(
                                div()
                                    .typeset(Type::LEAD)
                                    .text_color(t.dim)
                                    .child("with"),
                            )
                            .child(
                                self.model_chip(target, t, cx)
                                    .typeset(Type::LEAD),
                            )
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("attach")
                                .cursor_pointer()
                                .child(ui::mono("@ file", Type::CAPTION, t.dim))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.pick_attachments(cx)
                                })),
                        )
                        .child(ui::mono("· / command", Type::CAPTION, t.dim))
                        .child(div().pl(sp(2.)).child(send)),
                )
        };

        div()
            .relative()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(sp(2.))
            // Files dropped on it are attached: images as images.
            .on_drop(cx.listener(|ws, paths: &gpui::ExternalPaths, _, cx| {
                for path in paths.paths() {
                    ws.attach_path(path, cx);
                }
            }))
            .drag_over::<gpui::ExternalPaths>(|style, _, _, _| {
                style.opacity(0.7)
            })
            .children(self.slash_popover(compact, t, cx))
            // On a phone, in from the window's sides as the rest of
            // the screen is.
            .px(sp(if compact { 4.5 } else { 6. }))
            .pt(sp(if compact { 2. } else { 3. }))
            .pb(sp(if compact { 4.5 } else { 6. }))
            // On a desktop, the transcript's column.
            .when(!compact, |bar| bar.w_full().max_w(rems(50.5)).mx_auto())
            .when_some(self.fork_banner(t, cx), |bar, banner| bar.child(banner))
            .when_some(self.composer_note(), |bar, text| {
                bar.child(
                    div()
                        .id("composer-note")
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .typeset(Type::CAPTION)
                        .child(ui::mark(crate::assets::State::Met, t))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(t.text_soft)
                                .child(text.to_owned()),
                        ),
                )
            })
            .when_some(queued, |bar, text| {
                bar.child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(sp(2.5))
                        .typeset(Type::CAPTION)
                        .child(ui::mono("steer queued", Type::CAPTION, t.blue))
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
                // A heavy rule, not a box, sets the composer off from
                // the transcript.
                div()
                    .flex()
                    .flex_col()
                    .pt(sp(2.))
                    .border_t_2()
                    .border_color(t.rule)
                    .children(attached)
                    .child(div().pt(sp(1.5)).child(field)),
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
                .size(rems(1.375))
                .flex()
                .items_center()
                .justify_center()
                .when(on, |button| button.cursor_pointer())
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
                .pb(sp(1.5))
                .typeset(Type::CAPTION)
                .child(ui::mono("fork", Type::CAPTION, t.blue))
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
                .child(div().text_color(t.text_soft).child("with"))
                .child(self.model_chip(PickerTarget::Fork, t, cx))
                .child(div().flex_1())
                .child(
                    div()
                        .id("fork-cancel")
                        .child(ui::link_action("cancel", t))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.cancel_fork(cx)),
                        ),
                ),
        )
    }
}
