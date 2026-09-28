//! The window's root view: the runs, which one is open, and the layout
//! for the window's width.
//!
//! The workspace never talks to an agent. Runs come in through
//! [`Workspace::apply_event`] and [`Workspace::update_run`]; what the user
//! asks for goes out as a [`WorkspaceEvent`], for the host to carry out.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use gpui::{
    AnyElement,
    App,
    Context,
    Entity,
    EventEmitter,
    FocusHandle,
    Focusable,
    ScrollHandle,
    ScrollWheelEvent,
    Subscription,
    Task,
    Window,
    div,
    prelude::*,
    px,
};
use tau_agent::{event::RunEvent, tool::RunId};

use crate::{
    assets::Icon,
    input::{InputEvent, TextInput},
    theme::{NARROW_MAX, PHONE_MAX, Theme, theme},
    ui::{
        self,
        chrome,
        inspector::{self, Tab},
        transcript,
    },
    view::{RunStatus, RunUpdate, RunView},
};

/// What the user asked for. The host subscribes and acts on these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceEvent {
    /// Start a new run with this prompt.
    NewRun {
        prompt: String,
    },
    /// Queue a message for a running run (`Run::steer`).
    Steer {
        run: RunId,
        text: String,
    },
    Cancel {
        run: RunId,
    },
    /// Fork the run at its latest checkpoint.
    Fork {
        run: RunId,
    },
    /// Keep a note a memory plugin suggested.
    KeepNote {
        run: RunId,
        title: String,
    },
    /// Open a call a plugin flagged.
    ReviewCall {
        run: RunId,
        call_id: String,
    },
}

/// Which screen the phone layout shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhoneScreen {
    Runs,
    Chat,
}

pub struct Workspace {
    name: String,
    runs: Vec<RunView>,
    selected: usize,
    composer: Entity<TextInput>,
    tab: Tab,
    sheet_open: bool,
    phone_screen: PhoneScreen,
    /// Steering messages sent but not yet seen by the run, per run.
    queued: HashMap<RunId, String>,
    /// Notes kept, per run, by title.
    kept: HashMap<RunId, HashSet<String>>,
    scroll: ScrollHandle,
    /// Keep the transcript at its bottom as the run grows. Scrolling up
    /// turns it off; scrolling back down turns it on.
    follow: bool,
    focus: FocusHandle,
    replay: Option<Task<()>>,
    /// Draw the phone layout in a phone-sized frame, whatever the width.
    phone_preview: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(
        name: impl Into<String>,
        runs: Vec<RunView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| TextInput::new("Start a new run", cx));
        let subscription = cx.subscribe_in(
            &composer,
            window,
            |ws, _, event: &InputEvent, _, cx| match event {
                InputEvent::Submit(text) => ws.submit(text.clone(), cx),
            },
        );
        composer.read(cx).focus_handle(cx).focus(window);
        let mut workspace = Self {
            name: name.into(),
            runs,
            selected: 0,
            composer,
            tab: Tab::Run,
            sheet_open: false,
            phone_screen: PhoneScreen::Chat,
            queued: HashMap::new(),
            kept: HashMap::new(),
            scroll: ScrollHandle::new(),
            follow: true,
            focus: cx.focus_handle(),
            replay: None,
            phone_preview: false,
            _subscriptions: vec![subscription],
        };
        workspace.sync_placeholder(cx);
        workspace
    }

    pub fn runs(&self) -> &[RunView] {
        &self.runs
    }

    pub fn selected(&self) -> Option<&RunView> {
        self.runs.get(self.selected)
    }

    /// Adds a run at the top of the list and opens it.
    pub fn push_run(&mut self, run: RunView, cx: &mut Context<Self>) {
        self.runs.insert(0, run);
        self.selected = 0;
        self.phone_screen = PhoneScreen::Chat;
        self.sync_placeholder(cx);
        cx.notify();
    }

    /// Feeds a run event to every run it belongs to: its own run, and
    /// the parent that lists it as a child.
    pub fn apply_event(&mut self, event: &RunEvent, cx: &mut Context<Self>) {
        for run in &mut self.runs {
            run.apply(event);
        }
        if let RunEvent::TurnStart { run, .. } = event {
            // The run has read what was queued.
            self.queued.remove(run);
        }
        self.after_update(cx);
    }

    /// Applies an update to one run. Returns false if no run has that id.
    pub fn update_run(
        &mut self,
        run: &RunId,
        update: RunUpdate,
        cx: &mut Context<Self>,
    ) -> bool {
        if let RunUpdate::Event(event) = update {
            self.apply_event(&event, cx);
            return true;
        }
        let Some(view) = self.runs.iter_mut().find(|view| &view.id == run)
        else {
            return false;
        };
        view.update(update);
        self.after_update(cx);
        true
    }

    /// Plays scripted updates into a run, waiting before each one. For
    /// demos and for looking at the interface without an agent.
    pub fn replay(
        &mut self,
        run: RunId,
        steps: Vec<(Duration, RunUpdate)>,
        cx: &mut Context<Self>,
    ) {
        self.replay = Some(cx.spawn(async move |this, cx| {
            for (wait, update) in steps {
                if !wait.is_zero() {
                    cx.background_executor().timer(wait).await;
                }
                let applied = this.update(cx, |ws, cx| {
                    ws.update_run(&run, update, cx);
                });
                if applied.is_err() {
                    return;
                }
            }
        }));
    }

    pub fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.runs.len() {
            self.selected = index;
            self.phone_screen = PhoneScreen::Chat;
            self.sheet_open = false;
            self.sync_placeholder(cx);
            self.follow = true;
            cx.notify();
        }
    }

    /// Shows the phone layout in a 390×844 frame, for previewing it on a
    /// desktop.
    pub fn set_phone_preview(&mut self, on: bool, cx: &mut Context<Self>) {
        self.phone_preview = on;
        cx.notify();
    }

    pub fn show_run_list(&mut self, cx: &mut Context<Self>) {
        self.phone_screen = PhoneScreen::Runs;
        self.sheet_open = false;
        cx.notify();
    }

    pub fn toggle_sheet(&mut self, cx: &mut Context<Self>) {
        self.sheet_open = !self.sheet_open;
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Clears the composer and focuses it for a new prompt.
    pub fn start_new_run(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = usize::MAX;
        self.phone_screen = PhoneScreen::Chat;
        self.composer.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder("Describe the task for a new run");
        });
        self.composer.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    pub fn keep_note(
        &mut self,
        run: &RunId,
        title: &str,
        cx: &mut Context<Self>,
    ) {
        self.kept
            .entry(run.clone())
            .or_default()
            .insert(title.to_owned());
        cx.emit(WorkspaceEvent::KeepNote {
            run: run.clone(),
            title: title.to_owned(),
        });
        cx.notify();
    }

    pub fn review_call(
        &mut self,
        run: &RunId,
        call_id: &str,
        cx: &mut Context<Self>,
    ) {
        cx.emit(WorkspaceEvent::ReviewCall {
            run: run.clone(),
            call_id: call_id.to_owned(),
        });
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.selected() {
            cx.emit(WorkspaceEvent::Cancel {
                run: run.id.clone(),
            });
        }
    }

    fn fork(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.selected() {
            cx.emit(WorkspaceEvent::Fork {
                run: run.id.clone(),
            });
        }
    }

    fn submit(&mut self, text: String, cx: &mut Context<Self>) {
        match self.selected() {
            Some(run) if run.status.is_live() => {
                let run = run.id.clone();
                self.queued.insert(run.clone(), text.clone());
                cx.emit(WorkspaceEvent::Steer { run, text });
            }
            _ => cx.emit(WorkspaceEvent::NewRun { prompt: text }),
        }
        cx.notify();
    }

    fn submit_from_button(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().trim().to_owned();
        if !text.is_empty() {
            self.composer.update(cx, |input, cx| input.clear(cx));
            self.submit(text, cx);
        }
    }

    fn after_update(&mut self, cx: &mut Context<Self>) {
        self.sync_placeholder(cx);
        cx.notify();
    }

    fn sync_placeholder(&mut self, cx: &mut Context<Self>) {
        let live = self.selected().is_some_and(|run| run.status.is_live());
        self.composer.update(cx, |input, _| {
            input.set_placeholder(if live {
                "Steer the run, or @ a file, agent or checkpoint"
            } else {
                "Start a new run"
            })
        });
    }

    fn is_live(&self) -> bool {
        self.selected().is_some_and(|run| run.status.is_live())
    }

    // Layouts.

    fn transcript(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = HashSet::new();
        let items = match self.selected() {
            Some(run) => transcript::items(
                run,
                self.kept.get(&run.id).unwrap_or(&empty),
                t,
                compact,
                cx,
            ),
            None => vec![
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .pt(px(120.))
                    .text_color(t.muted)
                    .child(chrome::logo(t, 40.))
                    .child("A new run. Describe the task below.")
                    .into_any_element(),
            ],
        };
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        div()
            .id("transcript")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(
                |ws, event: &ScrollWheelEvent, _, _| {
                    let up = event.delta.pixel_delta(px(20.)).y > px(0.);
                    let bottom = -ws.scroll.max_offset().height;
                    ws.follow = !up && ws.scroll.offset().y <= bottom + px(40.);
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .px(px(if compact { 16. } else { 24. }))
                    .py(px(if compact { 16. } else { 20. }))
                    .children(items),
            )
    }

    fn run_header(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(run) = self.selected() else {
            return div()
                .h(px(48.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(24.))
                .border_b_1()
                .border_color(t.border)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("New run");
        };
        let (color, label) = ui::status_look(&run.status, t);
        let live = run.status.is_live();
        div()
            .h(px(48.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(24.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(run.title.clone()),
            )
            .child(ui::pill(
                match run.status {
                    RunStatus::Running => {
                        format!("{label} · turn {}", run.turn)
                    }
                    _ => label.to_string(),
                },
                color,
                t.raised,
            ))
            .child(ui::mono(
                format!(
                    "{} · {}",
                    run.agent,
                    crate::view::clock(run.limits.elapsed)
                ),
                12.,
                t.dim,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .id("fork")
                    .child(ui::button("Fork here", t).child(ui::icon(
                        Icon::Fork,
                        13.,
                        t.text_soft,
                    )))
                    .on_click(cx.listener(|ws, _, _, cx| ws.fork(cx))),
            )
            .when(live, |header| {
                header.child(
                    div()
                        .id("cancel")
                        .child(
                            ui::button("Cancel", t)
                                .border_color(t.red_border)
                                .text_color(t.red),
                        )
                        .on_click(cx.listener(|ws, _, _, cx| ws.cancel(cx))),
                )
            })
    }

    fn composer(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let live = self.is_live();
        let queued = self
            .selected()
            .and_then(|run| self.queued.get(&run.id))
            .cloned();
        let send = div()
            .id("send")
            .on_click(cx.listener(|ws, _, _, cx| ws.submit_from_button(cx)));
        let send = if compact {
            send.size(px(44.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(22.))
                .bg(t.accent)
                .cursor_pointer()
                .child(ui::icon(Icon::Send, 18., t.bg))
        } else {
            send.child(ui::primary_button(
                if live { "Steer" } else { "Start" },
                t,
            ))
        };

        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(if compact { 12. } else { 24. }))
            .pt(px(if compact { 10. } else { 0. }))
            .pb(px(if compact { 18. } else { 16. }))
            .when(compact, |bar| bar.border_t_1().border_color(t.border))
            .when_some(queued, |bar, text| {
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .bg(t.blue_soft)
                        .border_1()
                        .border_color(t.blue_border)
                        .text_size(px(12.))
                        .child(ui::icon(Icon::Chevron, 13., t.blue))
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
                    .items_center()
                    .gap(px(10.))
                    .when(compact && live, |row| {
                        row.child(
                            div()
                                .id("phone-cancel")
                                .size(px(44.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(22.))
                                .border_1()
                                .border_color(t.red_border)
                                .cursor_pointer()
                                .child(ui::icon(Icon::Stop, 14., t.red))
                                .on_click(cx.listener(|ws, _, _, cx| ws.cancel(cx))),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .min_h(px(44.))
                            .px(px(14.))
                            .border_1()
                            .border_color(t.border_strong)
                            .rounded(px(if compact { 22. } else { 10. }))
                            .bg(t.panel)
                            .when(!compact, |field| {
                                field.child(ui::icon(Icon::Paperclip, 14., t.muted))
                            })
                            .child(self.composer.clone())
                            .when(!compact, |field| {
                                field.child(ui::mono(
                                    if live { "Enter steers" } else { "Enter starts" },
                                    11.,
                                    t.dim,
                                ))
                            }),
                    )
                    .child(send),
            )
    }

    fn inspector(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = Tab::ALL.into_iter().map(|tab| {
            div()
                .id(tab.label())
                .child(inspector::tab_label(tab, tab == self.tab, t))
                .on_click(cx.listener(move |ws, _, _, cx| ws.set_tab(tab, cx)))
        });
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .bg(t.panel)
            .border_l_1()
            .border_color(t.border)
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .flex()
                    .items_end()
                    .gap(px(18.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(t.border)
                    .children(tabs),
            )
            .child(
                div()
                    .id("inspector")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p(px(16.))
                    .children(
                        self.selected()
                            .map(|run| inspector::content(run, self.tab, t)),
                    ),
            )
    }

    fn desktop(
        &self,
        wide: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total: f64 = self.runs.iter().map(|run| run.usage.cost).sum();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(chrome::title_bar(&self.name, self.selected(), total, t))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(
                        chrome::sidebar(&self.runs, self.selected, t, cx)
                            .w(px(if wide { 264. } else { 232. }))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(self.run_header(t, cx))
                            .child(self.transcript(false, t, cx))
                            .child(self.composer(false, t, cx)),
                    )
                    .when(wide, |row| {
                        row.child(
                            div()
                                .w(px(328.))
                                .flex_shrink_0()
                                .flex()
                                .flex_col()
                                .child(self.inspector(t, cx)),
                        )
                    }),
            )
            .child(chrome::status_bar(self.selected(), self.runs.len(), t))
            .into_any_element()
    }

    fn phone(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let chat = match (self.phone_screen, self.selected()) {
            (PhoneScreen::Chat, Some(run)) => Some(run),
            _ => None,
        };
        let Some(run) = chat else {
            if self.phone_screen == PhoneScreen::Chat {
                // A new run on the phone: just the composer.
                return div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(self.transcript(true, t, cx))
                    .child(self.composer(true, t, cx))
                    .into_any_element();
            }
            return chrome::phone_run_list(&self.name, &self.runs, t, cx)
                .into_any_element();
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(chrome::phone_run_bar(run, t, cx))
            .child(self.transcript(true, t, cx))
            .child(self.composer(true, t, cx))
            .when(self.sheet_open, |screen| {
                screen.child(self.sheet(run, t, cx))
            })
            .into_any_element()
    }

    fn sheet(
        &self,
        run: &RunView,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let segments = Tab::ALL.into_iter().map(|tab| {
            let active = tab == self.tab;
            div()
                .id(("sheet-tab", tab as usize))
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .text_size(px(13.))
                .cursor_pointer()
                .text_color(if active { t.text } else { t.muted })
                .when(active, |segment| segment.bg(t.selected))
                .child(tab.label())
                .on_click(cx.listener(move |ws, _, _, cx| ws.set_tab(tab, cx)))
        });
        let live = run.status.is_live();
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("scrim")
                    .flex_1()
                    .bg(t.scrim)
                    .on_click(cx.listener(|ws, _, _, cx| ws.toggle_sheet(cx))),
            )
            .child(
                div()
                    .h(gpui::relative(0.72))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .px(px(16.))
                    .pt(px(8.))
                    .pb(px(24.))
                    .bg(t.panel)
                    .border_t_1()
                    .border_color(t.border_strong)
                    .rounded_t(px(18.))
                    .child(
                        div().flex().justify_center().child(
                            div()
                                .w(px(40.))
                                .h(px(4.))
                                .rounded(px(2.))
                                .bg(t.border_strong),
                        ),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(3)
                            .gap(px(4.))
                            .p(px(4.))
                            .rounded(px(10.))
                            .bg(t.bg)
                            .children(segments),
                    )
                    .child(
                        div()
                            .id("sheet-body")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .child(inspector::content(run, self.tab, t)),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(if live { 2 } else { 1 })
                            .gap(px(8.))
                            .child(
                                div()
                                    .id("sheet-fork")
                                    .child(
                                        ui::button("Fork here", t)
                                            .h(px(44.))
                                            .rounded(px(10.)),
                                    )
                                    .on_click(
                                        cx.listener(|ws, _, _, cx| ws.fork(cx)),
                                    ),
                            )
                            .when(live, |row| {
                                row.child(
                                    div()
                                        .id("sheet-cancel")
                                        .child(
                                            ui::button("Cancel run", t)
                                                .h(px(44.))
                                                .rounded(px(10.))
                                                .border_color(t.red_border)
                                                .text_color(t.red),
                                        )
                                        .on_click(cx.listener(
                                            |ws, _, _, cx| ws.cancel(cx),
                                        )),
                                )
                            }),
                    ),
            )
    }
}

impl Render for Workspace {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx).clone();
        let width = window.viewport_size().width;
        let phone = self.phone_preview || width < PHONE_MAX;
        let body = if phone {
            self.phone(&t, cx)
        } else {
            self.desktop(width >= NARROW_MAX, &t, cx)
        };
        let body = if self.phone_preview {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgb(0x0b0c0e))
                .child(
                    div()
                        .w(px(390.))
                        .h(px(844.))
                        .max_h_full()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded(px(28.))
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.bg)
                        .child(body),
                )
                .into_any_element()
        } else {
            body
        };
        div()
            .track_focus(&self.focus)
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(crate::theme::SANS)
            .text_size(px(if phone { 14.5 } else { 13. }))
            .child(body)
    }
}
