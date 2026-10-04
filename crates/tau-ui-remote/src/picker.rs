//! Choosing models in the workspace: the picker (a popover on a desktop,
//! a sheet on a phone) over the ChatGPT account's models, the note that
//! explains a run's fixed model, and the settings the choices go to.

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
    rems,
};

use crate::{
    assets::Icon,
    models::{Effort, ModelChoice, ModelOption},
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, control, radius, sp},
    ui::{self, ButtonKind, Material as _},
    view::RunView,
    workspace::{Dialog, PickerTarget, Workspace, WorkspaceEvent},
};

impl Workspace {
    /// The model and effort a run's next message goes to, as its plan
    /// says. An effort a plugin picked counts as auto: each message is
    /// scored again; only one someone chose stays.
    pub fn model_of(run: &RunView) -> ModelChoice {
        let effort = run
            .plan
            .iter()
            .find(|field| field.name == "reasoning" && field.set_by.is_none())
            .and_then(|field| Effort::parse(&field.value))
            .unwrap_or(Effort::Auto);
        ModelChoice::new(run.model.clone(), effort)
    }

    /// What a picker for `target` starts from.
    pub fn choice_for(&self, target: &PickerTarget) -> ModelChoice {
        match target {
            PickerTarget::Next => self.next_model.clone(),
            PickerTarget::Fork => self.fork_model.clone(),
            PickerTarget::Run(run) => {
                self.run_models.get(run).cloned().unwrap_or_else(|| {
                    self.run(run)
                        .map_or_else(|| self.next_model.clone(), Self::model_of)
                })
            }
            PickerTarget::Default(agent) => {
                self.catalog.models.settings.default_for(agent)
            }
        }
    }

    /// What the composer's model chip sets: the next run's model, or the
    /// open conversation's next message's. The fork banner has its own.
    pub fn composer_target(&self) -> Option<PickerTarget> {
        if self.forking.is_some() {
            return None;
        }
        match self.current().filter(|_| self.route != Route::NewRun) {
            Some(run) => Some(PickerTarget::Run(run.id.clone())),
            None => Some(PickerTarget::Next),
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
        self.model_search.update(cx, |input, cx| input.clear(cx));
        self.model_search
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    /// Opens the picker without moving the keyboard focus to its search.
    pub fn show_picker(
        &mut self,
        target: PickerTarget,
        cx: &mut Context<Self>,
    ) {
        self.picker = Some(target);
        cx.notify();
    }

    pub fn close_picker(&mut self, cx: &mut Context<Self>) {
        self.picker = None;
        cx.notify();
    }

    /// Picks model `id`, one of the account's, for the open picker.
    pub fn pick_model(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(target) = self.picker.clone() else {
            return;
        };
        let Some(option) = self.catalog.models.find(id).cloned() else {
            return;
        };
        let choice = ModelChoice {
            model: option.id.clone(),
            ..self.choice_for(&target)
        }
        .fitted();
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
            PickerTarget::Run(run) => {
                self.run_models.insert(run, choice);
            }
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
            self.next_model = choice.clone();
        }
        // Shown here at once; the host saves the change, not this
        // interface's settings, which another may have changed since,
        // and sends every interface what it saved.
        cx.emit(WorkspaceEvent::SetDefaultModel {
            agent: agent.to_owned(),
            choice,
        });
        cx.notify();
    }

    /// Shows or hides a model in the picker, and saves it.
    pub fn toggle_model_hidden(&mut self, id: &str, cx: &mut Context<Self>) {
        let settings = &mut self.catalog.models.settings;
        settings.toggle_hidden(id);
        cx.emit(WorkspaceEvent::HideModel {
            id: id.to_owned(),
            hidden: settings.is_hidden(id),
        });
        cx.notify();
    }

    /// The title bar's model opens the picker the composer's chip does.
    pub fn title_model_clicked(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.composer_target() {
            self.open_picker(target, window, cx);
        }
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
            .px(sp(2.))
            .rounded(radius::BOX)
            .when(open, |chip| chip.bg(t.raised))
            .cursor_pointer()
            .hover(|style| style.bg(t.raised))
            .typeset(Type::CAPTION)
            .child(div().text_color(t.roles.model).child(choice.model.clone()))
            .child(div().text_color(t.dim).child("·"))
            .child(div().text_color(t.dim).child(choice.effort.label()))
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
        let live = matches!(&target, PickerTarget::Run(run)
            if self.run(run).is_some_and(|view| view.status.is_live()));
        let models = &self.catalog.models;
        let filter = self.model_search.read(cx).text().to_owned();
        let signed_in = models.access.chatgpt;
        // What auto does, as the plugin that picks the effort says.
        let auto_caption = self
            .contributions(
                tau_ui_plugin::points::PICKER_AUTO,
                &tau_ui_plugin::points::AtApp,
                cx,
            )
            .into_iter()
            .next()
            .unwrap_or_else(|| "Auto leaves the effort to the model.".into());
        let models = &self.catalog.models;
        // One list, in the account's order.
        let rows: Vec<AnyElement> = models
            .shown(&filter)
            .into_iter()
            .map(|option| model_row(option, &choice, phone, t, cx))
            .collect();
        let empty = rows.is_empty();
        let offered = Effort::offered(&choice.model);
        let columns = if phone { 4 } else { offered.len() as u16 };
        let efforts = offered.into_iter().map(|effort| {
            let on = effort == choice.effort;
            div()
                .id(SharedString::from(format!("effort-{}", effort.label())))
                .h(rems((if phone { 34. } else { 30. }) / 16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::CONTROL)
                .typeset(if phone { Type::SMALL } else { Type::CAPTION })
                .cursor_pointer()
                .text_color(if on { t.text } else { t.muted })
                .when(on, |segment| segment.key(t))
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
                Icon::Chat,
                IconSize::COMPACT,
                if signed_in { t.green } else { t.dim },
            ))
            .child(div().flex_1().min_w(rems(0.)).child(ui::prose(
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
            .raised(t)
            .border_1()
            .border_color(t.border_strong)
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
                    .child(ui::mono(
                        "on your ChatGPT plan",
                        Type::MICRO,
                        t.dim,
                    )),
            )
            .child(
                div()
                    .id("model-rows")
                    .flex()
                    .flex_col()
                    .pb(sp(1.5))
                    .max_h(rems((if phone { 380. } else { 420. }) / 16.))
                    .overflow_y_scroll()
                    .when(empty, |list| {
                        list.child(ui::empty(
                            if signed_in {
                                "No model matches."
                            } else {
                                "Sign in with ChatGPT and enable plan use to \
                                 see your models."
                            },
                            t,
                        ))
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
                                match (&target, live) {
                                    (PickerTarget::Run(_), true) => {
                                        "from your next message, once this run stops"
                                    }
                                    (PickerTarget::Run(_), false) => {
                                        "from your next message"
                                    }
                                    _ => "for the run",
                                },
                                Type::MICRO,
                                t.dim,
                            )),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(columns)
                            .gap(sp(1.))
                            .p(sp(1.))
                            .rounded(radius::BOX)
                            .well(t)
                            .children(efforts),
                    )
                    .when(choice.effort == Effort::Auto, |section| {
                        section.child(ui::text(
                            auto_caption.clone(),
                            Type::CAPTION,
                            t.muted,
                        ))
                    }),
            )
            .when(!matches!(target, PickerTarget::Default(_)), |panel| {
                panel.child(footer)
            });
        // A click outside closes the picker.
        let backdrop = div()
            .id("picker-backdrop")
            .absolute()
            .inset_0()
            // What is under it neither scrolls nor takes clicks.
            .occlude()
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
                .child(panel.w(rems(28.75)).rounded(radius::CARD)),
            // Above the composer, by its send button.
            PickerTarget::Next | PickerTarget::Run(_) | PickerTarget::Fork => {
                div()
                    .absolute()
                    .right(rems(
                        (if self.shows_inspector() { 424. } else { 96. }) / 16.,
                    ))
                    .bottom(rems(
                        (if target == PickerTarget::Fork {
                            140.
                        } else {
                            96.
                        }) / 16.,
                    ))
                    .child(panel.w(rems(28.75)).rounded(radius::CARD))
            }
        };
        backdrop.child(placed).into_any_element()
    }

    /// A dialog, with OK.
    pub(crate) fn dialog_view(
        &self,
        dialog: Dialog,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = div()
            .id("alert-ok")
            .child(ui::button("OK", ButtonKind::Primary, t))
            .on_click(cx.listener(|ws, _, _, cx| ws.dismiss_alert(cx)))
            .into_any_element();
        ui::dialog(dialog.title, dialog.message, actions, t).into_any_element()
    }
}

/// One model in the picker: chosen, or pickable.
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
    } else {
        div().w(rems(0.875)).into_any_element()
    };
    let row = div()
        .id(SharedString::from(format!("model-{}", option.id)))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .min_h(rems((if phone { 52. } else { 40. }) / 16.))
        .px(sp(3.5))
        .child(mark);
    let row = if phone {
        row.child(
            ui::mono(option.label().to_owned(), Type::BODY, t.text)
                .flex_1()
                .min_w(rems(0.)),
        )
    } else {
        row.child(
            ui::mono(option.label().to_owned(), Type::SMALL, t.text).flex_1(),
        )
        .when(option.context > 0, |row| {
            row.child(ui::mono(option.context_label(), Type::MICRO, t.dim))
        })
    };
    row.cursor_pointer()
        .hover(|style| style.bg(gpui::white().opacity(0.04)))
        .on_click(cx.listener(move |ws, _, _, cx| ws.pick_model(&id, cx)))
        .into_any_element()
}
