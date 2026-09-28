//! Choosing models in the workspace: the picker (a popover on a desktop,
//! a sheet on a phone), the price check before an expensive model, the
//! note that explains a run's fixed model, and the settings the choices
//! go to.

use gpui::{
    AnyElement,
    Context,
    Div,
    Focusable as _,
    SharedString,
    Stateful,
    Window,
    div,
    prelude::*,
    px,
};
use tau_agent::tool::RunId;

use crate::{
    assets::Icon,
    models::{Effort, ModelChoice, ModelOption, Tier},
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, control, radius, sp, weight},
    ui::{self, ButtonKind},
    view::RunView,
    workspace::{Confirm, Dialog, PickerTarget, Workspace, WorkspaceEvent},
};

impl Workspace {
    /// The model and effort a run runs on, as its plan says.
    pub fn model_of(run: &RunView) -> ModelChoice {
        let effort = run
            .plan
            .iter()
            .find(|field| field.name == "reasoning")
            .and_then(|field| {
                Effort::ALL
                    .into_iter()
                    .find(|effort| effort.label() == field.value)
            })
            .unwrap_or(Effort::Auto);
        ModelChoice::new(run.model.clone(), effort)
    }

    /// What a picker for `target` starts from.
    pub fn choice_for(&self, target: &PickerTarget) -> ModelChoice {
        match target {
            PickerTarget::Next => self.next_model.clone(),
            PickerTarget::Fork => self.fork_model.clone(),
            PickerTarget::Default(agent) => {
                self.catalog.models.settings.default_for(agent)
            }
        }
    }

    /// The model the next run starts on.
    pub fn next_model(&self) -> &ModelChoice {
        &self.next_model
    }

    /// The model the fork being written runs on.
    pub fn fork_model(&self) -> &ModelChoice {
        &self.fork_model
    }

    pub fn picker(&self) -> Option<&PickerTarget> {
        self.picker.as_ref()
    }

    pub fn open_picker(
        &mut self,
        target: PickerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker = Some(target);
        self.model_info = false;
        self.model_search.update(cx, |input, cx| input.clear(cx));
        self.model_search.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    /// Opens the picker without moving the keyboard focus to its search.
    pub fn show_picker(
        &mut self,
        target: PickerTarget,
        cx: &mut Context<Self>,
    ) {
        self.picker = Some(target);
        self.model_info = false;
        cx.notify();
    }

    /// Opens the note on the open run's fixed model.
    pub fn show_model_info(&mut self, cx: &mut Context<Self>) {
        self.model_info = true;
        cx.notify();
    }

    pub fn close_picker(&mut self, cx: &mut Context<Self>) {
        self.picker = None;
        cx.notify();
    }

    /// Picks model `id` for the open picker: at once, or after asking
    /// when it costs more than the user's threshold. A model the sign-in
    /// cannot run is not picked.
    pub fn pick_model(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(target) = self.picker.clone() else {
            return;
        };
        let Some(option) = self.catalog.models.find(id).cloned() else {
            return;
        };
        if !option.available {
            return;
        }
        let choice = ModelChoice {
            model: option.id.clone(),
            ..self.choice_for(&target)
        };
        if self.catalog.models.needs_confirm(id) {
            let limit =
                self.catalog.models.settings.ask_above.unwrap_or_default();
            self.dialog = Some(Dialog {
                title: format!("Use {}?", option.id),
                message: format!(
                    "It costs {} per million tokens in and out, above the \
                     ${limit:.0} per million output tokens you set to ask \
                     about. Each run still stops at its own cost limit.",
                    option.price()
                ),
                confirm: Some(Confirm::Model(target, choice)),
            });
            cx.notify();
            return;
        }
        self.apply_choice(target, choice, cx);
        self.close_picker(cx);
    }

    /// Sets the open picker's reasoning effort.
    pub fn pick_effort(&mut self, effort: Effort, cx: &mut Context<Self>) {
        let Some(target) = self.picker.clone() else {
            return;
        };
        let choice = ModelChoice {
            effort,
            ..self.choice_for(&target)
        };
        self.apply_choice(target, choice, cx);
    }

    /// Confirms the open dialog's choice.
    pub fn confirm_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        if let Some(Confirm::Model(target, choice)) = dialog.confirm {
            self.apply_choice(target, choice, cx);
            self.close_picker(cx);
        }
        cx.notify();
    }

    fn apply_choice(
        &mut self,
        target: PickerTarget,
        choice: ModelChoice,
        cx: &mut Context<Self>,
    ) {
        match target {
            PickerTarget::Next => {
                self.next_model = choice;
                self.next_model_picked = true;
            }
            PickerTarget::Fork => self.fork_model = choice,
            PickerTarget::Default(agent) => {
                self.set_default_model(&agent, choice, cx)
            }
        }
        cx.notify();
    }

    /// Makes `choice` `agent`'s default and saves it.
    pub fn set_default_model(
        &mut self,
        agent: &str,
        choice: ModelChoice,
        cx: &mut Context<Self>,
    ) {
        self.catalog
            .models
            .settings
            .set_default(agent, choice.clone());
        if agent == "coder" && !self.next_model_picked {
            self.next_model = choice;
        }
        self.save_model_settings(cx);
    }

    /// Shows or hides a model in the picker, and saves it.
    pub fn toggle_model_hidden(&mut self, id: &str, cx: &mut Context<Self>) {
        self.catalog.models.settings.toggle_hidden(id);
        self.save_model_settings(cx);
    }

    /// Moves the price to ask above one step, and saves it.
    pub fn step_ask_above(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.catalog.models.settings.step_ask_above(delta);
        self.save_model_settings(cx);
    }

    fn save_model_settings(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::SaveModelSettings(
            self.catalog.models.settings.clone(),
        ));
        cx.notify();
    }

    /// The title bar's model: on an open run it explains why the model is
    /// fixed; otherwise it opens the picker for the next run.
    pub fn title_model_clicked(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shows_run_model() {
            self.model_info = !self.model_info;
            cx.notify();
        } else {
            self.open_picker(PickerTarget::Next, window, cx);
        }
    }

    /// Whether the title bar shows the open run's model, not the next
    /// run's.
    pub fn shows_run_model(&self) -> bool {
        matches!(self.route, Route::Run(_) | Route::Home)
            && self.current().is_some()
    }

    /// From the fixed-model note: fork the open run on another model.
    fn fork_on_another_model(
        &mut self,
        run: &RunId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.run(run) else { return };
        let turn = Self::last_fork_turn_of(view);
        self.fork_from(run, turn, window, cx);
        self.open_picker(PickerTarget::Fork, window, cx);
    }

    // Views.

    /// A chip that shows a picker's choice and opens it.
    pub(crate) fn model_chip(
        &self,
        target: PickerTarget,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let choice = self.choice_for(&target);
        let open = self.picker.as_ref() == Some(&target);
        let id = SharedString::from(format!("model-chip-{target:?}"));
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(sp(1.5))
            .flex_shrink_0()
            .h(control::SMALL)
            .px(sp(2.5))
            .rounded(radius::CONTROL)
            .border_1()
            .border_color(if open { t.accent } else { t.border_strong })
            .when(open, |chip| chip.bg(t.raised))
            .cursor_pointer()
            .hover(|style| style.bg(gpui::white().opacity(0.04)))
            .child(ui::mono(choice.model.clone(), Type::CAPTION, t.text_soft))
            .child(ui::mono("·", Type::CAPTION, t.dim))
            .child(ui::mono(choice.effort.label(), Type::CAPTION, t.blue))
            .child(ui::icon(Icon::Down, IconSize::SMALL, t.muted))
            .on_click(cx.listener(move |ws, _, window, cx| {
                if ws.picker.as_ref() == Some(&target) {
                    ws.close_picker(cx);
                } else {
                    ws.open_picker(target.clone(), window, cx);
                }
            }))
    }

    /// What floats over the app for models: the open picker, or the note
    /// on a run's fixed model.
    pub(crate) fn model_overlay(
        &self,
        phone: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(target) = self.picker.clone() {
            return Some(self.picker_view(target, phone, t, cx));
        }
        if self.model_info && !phone {
            return self.model_info_view(t, cx);
        }
        None
    }

    fn picker_view(
        &self,
        target: PickerTarget,
        phone: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let choice = self.choice_for(&target);
        let models = &self.catalog.models;
        let filter = self.model_search.read(cx).text().to_owned();
        let shown = models.shown(&filter);
        let rows = Tier::ALL.into_iter().flat_map(|tier| {
            let in_tier: Vec<&ModelOption> =
                shown.iter().copied().filter(|m| m.tier() == tier).collect();
            let heading = (!in_tier.is_empty()).then(|| {
                div()
                    .px(sp(3.5))
                    .pt(sp(2.5))
                    .pb(sp(1.))
                    .child(ui::text(tier.label(), Type::MICRO, t.dim))
                    .into_any_element()
            });
            let rows: Vec<AnyElement> = in_tier
                .into_iter()
                .map(|option| model_row(option, &choice, phone, t, cx))
                .collect();
            heading.into_iter().chain(rows)
        });
        let rows: Vec<AnyElement> = rows.collect();
        let empty = rows.is_empty();
        let efforts = Effort::ALL.into_iter().map(|effort| {
            let on = effort == choice.effort;
            div()
                .id(SharedString::from(format!("effort-{}", effort.label())))
                .h(px(if phone { 34. } else { 30. }))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::CONTROL)
                .typeset(if phone { Type::SMALL } else { Type::CAPTION })
                .cursor_pointer()
                .text_color(if on { t.text } else { t.muted })
                .when(on, |segment| segment.bg(t.selected))
                .child(if effort == Effort::Auto {
                    "Auto"
                } else {
                    effort.label()
                })
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.pick_effort(effort, cx)),
                )
        });
        let default = models.settings.default_for("coder");
        let footer = div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .px(sp(3.5))
            .py(sp(2.5))
            .bg(t.card)
            .border_t_1()
            .border_color(t.border)
            .typeset(Type::CAPTION)
            .text_color(t.muted)
            .child(ui::icon(
                if models.access.chatgpt {
                    Icon::Chat
                } else {
                    Icon::Key
                },
                IconSize::COMPACT,
                t.green,
            ))
            .child(div().flex_1().min_w(px(0.)).child(ui::prose(
                &format!(
                    "{} · coder's default is `{}`",
                    models.access.label, default.model
                ),
                t.muted,
                t,
            )))
            .child(
                div()
                    .id("picker-settings")
                    .child(ui::text_link("Model settings", Type::CAPTION, t))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.close_picker(cx);
                        ws.navigate(Route::Models, cx);
                    })),
            );
        let panel = div()
            .id("model-picker")
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(t.panel)
            .border_1()
            .border_color(t.border_strong)
            .shadow_lg()
            // Clicks inside the panel stay in it.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .h(control::LARGE)
                    .px(sp(3.5))
                    .border_b_1()
                    .border_color(t.border)
                    .child(ui::icon(Icon::Search, IconSize::BASE, t.dim))
                    .child(self.model_search.clone())
                    .child(ui::mono("$ in / out per M", Type::MICRO, t.dim)),
            )
            .child(
                div()
                    .id("model-rows")
                    .flex()
                    .flex_col()
                    .pb(sp(1.5))
                    .max_h(px(if phone { 380. } else { 420. }))
                    .overflow_y_scroll()
                    .when(empty, |list| {
                        list.child(ui::empty("No model matches.", t))
                    })
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.5))
                    .px(sp(3.5))
                    .py(sp(3.))
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                ui::text("Reasoning", Type::CAPTION, t.muted)
                                    .flex_1(),
                            )
                            .child(ui::mono(
                                "fixed for the whole run",
                                Type::MICRO,
                                t.dim,
                            )),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(if phone { 3 } else { 6 })
                            .gap(sp(1.))
                            .p(sp(1.))
                            .rounded(radius::BOX)
                            .bg(t.bg)
                            .children(efforts),
                    )
                    .when(choice.effort == Effort::Auto, |section| {
                        section.child(ui::text(
                            "Auto leaves the effort to the model, or to a \
                             reasoning plugin when the agent has one.",
                            Type::CAPTION,
                            t.muted,
                        ))
                    }),
            )
            .when(
                matches!(target, PickerTarget::Next | PickerTarget::Fork),
                |panel| panel.child(footer),
            );
        // A click outside closes the picker.
        let backdrop = div()
            .id("picker-backdrop")
            .absolute()
            .inset_0()
            .when(phone, |backdrop| backdrop.bg(t.scrim))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|ws, _, _, cx| ws.close_picker(cx)),
            );
        if phone {
            return backdrop
                .flex()
                .flex_col()
                .justify_end()
                .child(
                    panel.w_full().rounded_t(radius::SHEET).child(
                        div().px(sp(4.)).pb(sp(5.)).pt(sp(2.)).child(
                            div()
                                .id("picker-done")
                                .child(ui::big_button(
                                    format!("Use {}", choice.label()),
                                    None,
                                    ButtonKind::Primary,
                                    t,
                                ))
                                .on_click(cx.listener(|ws, _, _, cx| {
                                    ws.close_picker(cx)
                                })),
                        ),
                    ),
                )
                .into_any_element();
        }
        let placed = match target {
            // Over the settings screen: in the middle.
            PickerTarget::Default(_) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(panel.w(px(460.)).rounded(radius::CARD)),
            // Above the composer, by its send button.
            PickerTarget::Next | PickerTarget::Fork => div()
                .absolute()
                .right(px(if self.shows_inspector() { 424. } else { 96. }))
                .bottom(px(if target == PickerTarget::Fork {
                    140.
                } else {
                    96.
                }))
                .child(panel.w(px(460.)).rounded(radius::CARD)),
        };
        backdrop.child(placed).into_any_element()
    }

    /// The note under the title bar's model: why it is fixed, and a way to
    /// another model.
    fn model_info_view(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let run = self.current()?;
        let choice = Self::model_of(run);
        let price = self.catalog.models.find(&choice.model).map_or_else(
            || "—".to_owned(),
            |option| format!("{} per M", option.price()),
        );
        let id = run.id.clone();
        let forkable = Self::last_fork_turn_of(run) >= 1;
        let rows = [
            ("model", choice.model.clone()),
            ("reasoning", choice.effort.label().to_owned()),
            ("price", price),
        ];
        Some(
            div()
                .id("model-info-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|ws, _, _, cx| {
                        ws.model_info = false;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("model-info")
                        .absolute()
                        .top(px(50.))
                        .right(px(128.))
                        .w(px(340.))
                        .flex()
                        .flex_col()
                        .gap(sp(2.5))
                        .p(sp(3.5))
                        .bg(t.panel)
                        .border_1()
                        .border_color(t.border_strong)
                        .rounded(radius::LARGE)
                        .shadow_lg()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation()
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(2.))
                                .child(ui::icon(Icon::Lock, IconSize::BASE, t.muted))
                                .child(
                                    div()
                                        .font_weight(weight::STRONG)
                                        .child("Fixed for this run"),
                                ),
                        )
                        .child(ui::text(
                            "A run keeps its model and reasoning from its first \
                             request, so its session can keep sending only what \
                             is new. To try another model, fork the run: the fork \
                             starts where you choose, on the model you pick.",
                            Type::CAPTION,
                            t.muted,
                        ).leading(1.55))
                        .child(ui::key_values(
                            rows.into_iter().map(|(key, value)| {
                                (key.into(), ui::mono(value, Type::CAPTION, t.text))
                            }),
                            t,
                        ))
                        .when(forkable, |note| {
                            note.child(
                                div()
                                    .id("fork-another-model")
                                    .child(
                                        ui::button(
                                            "Fork on another model",
                                            ButtonKind::Secondary,
                                            t,
                                        )
                                        .child(ui::icon(
                                            Icon::Fork,
                                            IconSize::COMPACT,
                                            t.text_soft,
                                        )),
                                    )
                                    .on_click(cx.listener(move |ws, _, window, cx| {
                                        ws.fork_on_another_model(&id, window, cx)
                                    })),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    /// A dialog, with OK, or Cancel and the choice to confirm.
    pub(crate) fn dialog_view(
        &self,
        dialog: Dialog,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = match &dialog.confirm {
            None => div()
                .id("alert-ok")
                .child(ui::button("OK", ButtonKind::Primary, t))
                .on_click(cx.listener(|ws, _, _, cx| ws.dismiss_alert(cx)))
                .into_any_element(),
            Some(Confirm::Model(_, choice)) => div()
                .flex()
                .gap(sp(2.))
                .child(
                    div()
                        .id("dialog-cancel")
                        .child(ui::button("Cancel", ButtonKind::Secondary, t))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.dismiss_alert(cx)),
                        ),
                )
                .child(
                    div()
                        .id("dialog-confirm")
                        .child(ui::button(
                            format!("Use {}", choice.model),
                            ButtonKind::Primary,
                            t,
                        ))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.confirm_dialog(cx)),
                        ),
                )
                .into_any_element(),
        };
        ui::dialog(dialog.title, dialog.message, actions, t).into_any_element()
    }
}

/// One model in the picker: chosen, pickable, or locked behind an API key.
fn model_row(
    option: &ModelOption,
    choice: &ModelChoice,
    phone: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let picked = option.id == choice.model;
    let id = option.id.clone();
    let mark = if picked {
        ui::icon(Icon::Check, IconSize::BASE, t.accent).into_any_element()
    } else if option.available {
        div().w(px(14.)).into_any_element()
    } else {
        ui::icon(Icon::Lock, IconSize::COMPACT, t.dim).into_any_element()
    };
    let name_color = if option.available { t.text } else { t.dim };
    let row = div()
        .id(SharedString::from(format!("model-{}", option.id)))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .min_h(px(if phone { 52. } else { 40. }))
        .px(sp(3.5))
        .child(mark);
    let row = if phone {
        row.child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child(ui::mono(option.id.clone(), Type::BODY, name_color))
                .child(ui::mono(
                    format!(
                        "{} · {} per M",
                        option.context_label(),
                        option.price()
                    ),
                    Type::MICRO,
                    t.muted,
                )),
        )
    } else {
        row.child(ui::mono(option.id.clone(), Type::SMALL, name_color).flex_1())
            .child(ui::mono(option.context_label(), Type::MICRO, t.dim))
    };
    let row = row
        .when(!option.available, |row| {
            row.child(ui::text("needs an API key", Type::MICRO, t.dim))
        })
        .when(!phone, |row| {
            row.child(div().w(px(104.)).flex().justify_end().child(ui::mono(
                option.price(),
                Type::MICRO,
                t.muted,
            )))
        });
    if option.available {
        row.cursor_pointer()
            .hover(|style| style.bg(gpui::white().opacity(0.04)))
            .on_click(cx.listener(move |ws, _, _, cx| ws.pick_model(&id, cx)))
            .into_any_element()
    } else {
        row.into_any_element()
    }
}
