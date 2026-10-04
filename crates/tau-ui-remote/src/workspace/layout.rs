//! Drawing the workspace: the run screen, the desktop and phone layouts,
//! and the transcript.

use super::*;

/// The desktop sidebar's width: wider on a wide window.
pub(super) fn sidebar_width(wide: bool) -> gpui::Pixels {
    px(if wide { 264. } else { 232. })
}

impl Workspace {
    /// How wide a screen beside the sidebar is drawn: the window less
    /// the sidebar, or the whole window on a phone.
    pub fn screen_width(&self) -> gpui::Pixels {
        if self.phone_preview || self.width < PHONE_MAX {
            self.width
        } else {
            self.width - sidebar_width(self.width >= NARROW_MAX)
        }
    }
}

impl Render for Workspace {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.sync_transcript();
        self.release_focus(window, cx);
        self.plugin_focus(window, cx);
        if let Route::Setup(step) = self.route {
            screens::setup::observe(self, step, window, cx);
        }
        let t = theme(cx).clone();
        let width = match self.frame {
            Some((width, _)) => px(width),
            None => window.viewport_size().width,
        };
        let phone = self.phone_preview || width < PHONE_MAX;
        self.width = if self.phone_preview { px(390.) } else { width };
        self.inspector_shown = !phone
            && width >= NARROW_MAX
            && self.details_open
            && matches!(self.route, Route::Home | Route::Run(_));
        let body = if phone {
            self.phone(&t, cx)
        } else {
            self.desktop(width >= NARROW_MAX, &t, cx)
        };
        // Drawing found nothing in the composer's place where something
        // was: the composer takes the keys back.
        if self.composer_back.take() {
            self.composer.read(cx).focus_handle(cx).focus(window, cx);
        }
        // A dialog covers the app, wherever the app is drawn.
        let body = div()
            .relative()
            .size_full()
            .child(body)
            .when_some(self.model_overlay(phone, &t, cx), |body, overlay| {
                body.child(overlay)
            })
            .when(self.searching, |body| body.child(self.search_view(&t, cx)))
            .when(self.adding_jev_key, |body| {
                body.child(self.jev_key_view(&t, cx))
            })
            .when_some(self.plan_alert.clone(), |body, alert| {
                body.child(self.plan_alert_view(&alert, &t, cx))
            })
            .when_some(self.dialog.clone(), |body, dialog| {
                body.child(self.dialog_view(dialog, &t, cx))
            })
            .into_any_element();
        let body = if self.phone_preview {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.backdrop)
                .child(
                    div()
                        .w(px(390.))
                        .h(px(844.))
                        .max_h_full()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded(radius::DEVICE)
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.bg)
                        .child(body),
                )
                .into_any_element()
        } else if let Some((width, height)) = self.frame {
            div()
                .size_full()
                .bg(t.backdrop)
                .child(
                    div()
                        .w(px(width))
                        .h(px(height))
                        .overflow_hidden()
                        .bg(t.bg)
                        .child(body),
                )
                .into_any_element()
        } else {
            body
        };
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|ws, _: &GoBack, _, cx| ws.escape(cx)))
            // Up and down move through the slash menu, else between the
            // composer's lines.
            .on_action(cx.listener(|ws, _: &SlashUp, window, cx| {
                if !ws.slash_move(-1, cx) && !ws.composer_row(-1, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|ws, _: &SlashDown, window, cx| {
                if !ws.slash_move(1, cx) && !ws.composer_row(1, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|ws, _: &SlashComplete, _, cx| {
                if !ws.slash_complete(cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|ws, _: &NewRun, window, cx| {
                ws.start_new_run(window, cx)
            }))
            .on_action(cx.listener(|ws, _: &Search, window, cx| {
                ws.open_search(window, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowRuns, _, cx| {
                ws.switch_tab(route::Tab::Runs, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowMemory, _, cx| {
                if let Some(repo) = ws.selected_repo().map(str::to_owned) {
                    ws.open_memory(&repo, cx);
                }
            }))
            .on_action(cx.listener(|ws, _: &ShowHistory, _, cx| {
                ws.switch_tab(route::Tab::History, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowPlugins, _, cx| {
                ws.switch_tab(route::Tab::Plugins, cx)
            }))
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(crate::theme::SANS)
            .typeset(if phone { Type::PHONE } else { Type::SMALL })
            .child(body)
    }
}

impl Workspace {
    pub(super) fn transcript(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self
            .current()
            .filter(|_| self.route != Route::NewRun)
            .is_none()
        {
            return div()
                .id("transcript")
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(sp(3.))
                .text_color(t.muted)
                .child(ui::logo(t, 40.))
                .child("A new run. Describe the task below.")
                .into_any_element();
        }
        let theme = t.clone();
        gpui::list(
            self.transcript.clone(),
            cx.processor(move |ws, index: usize, window, cx| {
                let Some(run) =
                    ws.current().filter(|_| ws.route != Route::NewRun)
                else {
                    return div().into_any_element();
                };
                transcript::item(ws, run, index, &theme, compact, window, cx)
            }),
        )
        .flex_1()
        .min_h(px(0.))
        .into_any_element()
    }

    /// How wide the transcript was last laid out; `None` before it was.
    pub(crate) fn transcript_width(&self) -> Option<gpui::Pixels> {
        self.transcript_width
    }

    /// Takes the focus back from an overlay's field once the overlay is
    /// gone. Left on a field no longer drawn, keys would reach nothing,
    /// not even ctrl+k to open search again.
    /// Gives the keys where a plugin asked, unless the person is writing
    /// in the composer.
    pub(super) fn plugin_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let composer = self.composer.read(cx);
        let writing = composer.focus_handle(cx).is_focused(window)
            && !composer.text().trim().is_empty();
        if let Some(handle) = self.plugin_focus.take()
            && !writing
        {
            handle.focus(window, cx);
        }
    }

    pub(super) fn release_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stranded = (!self.searching
            && self.search.read(cx).focus_handle(cx).is_focused(window))
            || (self.picker.is_none()
                && self
                    .model_search
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window));
        if stranded {
            self.focus.focus(window, cx);
        }
    }

    /// Tells the transcript list what changed in the open run since the
    /// last frame: items added, or another run opened. An item in view is
    /// laid out again every frame, so one that grows as it streams needs
    /// no telling.
    pub(super) fn sync_transcript(&mut self) {
        let width = self.transcript.viewport_bounds().size.width;
        self.transcript_width = (width > px(0.)).then_some(width);
        // One more row, past the items, for an open landing's card, a
        // landed chat's, a queued chat's, or a main chat's queue,
        // conflicts and push cards.
        let now =
            self.current()
                .filter(|_| self.route != Route::NewRun)
                .map(|run| {
                    let pushed = self
                        .main_repo(&run.id)
                        .and_then(|repo| self.push_state(repo))
                        .is_some_and(|state| {
                            !matches!(state, PushState::Pushing { .. })
                        });
                    let card = usize::from(
                        self.landings.contains_key(&run.id)
                            || matches!(
                                run.ending,
                                Some(Ending::Landed { .. })
                            )
                            || run.status == RunStatus::Interrupted
                            || self.queued(&run.id).is_some()
                            || !run.landing_queue.is_empty()
                            || run
                                .main_conflicts
                                .as_ref()
                                .is_some_and(|c| !c.dismissed)
                            || pushed,
                    );
                    (run.id.clone(), run.items.len() + card)
                });
        match (&now, &self.listed) {
            (Some((run, count)), Some((listed, before))) if run == listed => {
                if count > before {
                    // The last known item may have changed too.
                    let from = before.saturating_sub(1);
                    self.transcript.splice(from..*before, count - from);
                } else if count < before {
                    self.transcript.reset(*count);
                }
            }
            (Some((_, count)), _) => self.transcript.reset(*count),
            (None, _) => {}
        }
        self.listed = now;
        if self.follow {
            self.transcript.scroll_to(ListOffset {
                item_ix: usize::MAX,
                offset_in_item: px(0.),
            });
        }
    }

    /// Land on the parent: in the run's bar, for a finished chat (ADR
    /// 0014). It opens the landing card at the end of the chat.
    pub(super) fn land_button(
        &self,
        run: &RunView,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let target = ui::landing::target(self, run)?;
        // A queued chat's card says where it waits instead.
        if run.status.is_live()
            || self.closed.contains(&run.id)
            || self.queued(&run.id).is_some()
        {
            return None;
        }
        let label = format!("Land on {target}");
        let id = run.id.clone();
        Some(
            div()
                .id("land")
                .child(ui::button(label, ButtonKind::Primary, t))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.follow = true;
                    ws.preview_landing(&id, cx)
                })),
        )
    }

    pub(super) fn run_header(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(run) = self.current().filter(|_| self.route != Route::NewRun)
        else {
            return div()
                .h(px(48.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(sp(6.))
                .border_b_1()
                .border_color(t.border)
                .font_weight(weight::STRONG)
                .child("New run");
        };
        // A chat that ended is read-only: its title, and a tag saying so.
        if run.ending.is_some() {
            return div()
                .h(px(48.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(sp(3.))
                .px(sp(6.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .typeset(Type::LEAD)
                        .font_weight(weight::STRONG)
                        .text_color(t.muted)
                        .child(run.title.clone()),
                )
                .child(
                    ui::mono(ui::ending::READ_ONLY, Type::MICRO, t.dim)
                        .px(sp(1.5))
                        .py(sp(0.5))
                        .rounded(radius::CONTROL)
                        .border_1()
                        .border_color(t.border),
                );
        }
        let (color, label) = ui::status_look(&run.status, t);
        let live = run.status.is_live();
        let repo = self.repo_of(run).to_owned();
        // How full the context is, as a bar beside its numbers.
        let window = run.context.window.filter(|window| *window > 0);
        let fill = window.map_or(0., |window| {
            (run.context.used as f32 / window as f32).clamp(0., 1.)
        });
        // A main chat pushes to GitHub instead (ADR 0023).
        let done = self.catalog.pull_requests
            && run.status == RunStatus::Finished(StopReason::Stop)
            && !self.is_main(&run.id);
        let id = run.id.clone();
        div()
            .h(px(48.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(sp(3.))
            .px(sp(6.))
            .border_b_1()
            .border_color(t.border)
            // Where the run is: its repository, then its title.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(1.5))
                    .min_w(px(0.))
                    .child(div().text_color(t.mark(&repo)).child(repo))
                    .child(div().text_color(t.dim).child("/"))
                    .child(
                        div()
                            .truncate()
                            .font_weight(weight::STRONG)
                            .child(run.title.clone()),
                    ),
            )
            // Who works on it, how far, and what it cost.
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(sp(1.))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(
                        div()
                            .text_color(t.roles.agent)
                            .child(run.agent.clone()),
                    )
                    .child(format!("· turn {} ·", run.turn))
                    .child(
                        div()
                            .text_color(t.roles.cost)
                            .child(crate::view::usd(run.usage.cost)),
                    )
                    .when(!matches!(run.status, RunStatus::Running), |row| {
                        row.child("·")
                            .child(div().text_color(color).child(label))
                    }),
            )
            .child(div().flex_1())
            .when_some(window, |header, window| {
                header.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(sp(2.))
                        .typeset(Type::CAPTION)
                        .text_color(t.dim)
                        .child(format!(
                            "{} of {}",
                            crate::view::tokens(run.context.used),
                            crate::view::tokens(window)
                        ))
                        .child(
                            div()
                                .w(px(80.))
                                .h(px(4.))
                                .rounded(radius::HAIRLINE)
                                .bg(t.raised)
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(fill.max(0.02)))
                                        .rounded(radius::HAIRLINE)
                                        .bg(t.roles.meter),
                                ),
                        ),
                )
            })
            .children(ui::push::header(self, run, t, cx))
            .children(
                run.children
                    .iter()
                    .find(|child| child.kind == ChildKind::Fork)
                    .map(|fork| (run.id.clone(), fork.id.clone()))
                    // A fork compares with the run it came from.
                    .or_else(|| match &run.origin {
                        Origin::Fork { from, .. } => {
                            Some((from.clone(), run.id.clone()))
                        }
                        _ => None,
                    })
                    .map(|(main, fork)| {
                        let route = Route::Compare { main, fork };
                        div()
                            .id("compare")
                            .child(ui::button(
                                "Compare forks",
                                ButtonKind::Secondary,
                                t,
                            ))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.navigate(route.clone(), cx)
                            }))
                    }),
            )
            .when(done, |header| {
                header.child(
                    div()
                        .id("pull-request")
                        .child(
                            ui::button(
                                "Pull request",
                                ButtonKind::Secondary,
                                t,
                            )
                            .child(ui::icon(
                                Icon::PullRequest,
                                IconSize::COMPACT,
                                t.text_soft,
                            )),
                        )
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.open_pull_request(&id, cx)
                        })),
                )
            })
            .children(self.land_button(run, t, cx))
            .when(self.can_fork(run), |header| {
                header.child(
                    div()
                        .id("fork")
                        .child(
                            ui::button("Fork", ButtonKind::Secondary, t).child(
                                ui::icon(
                                    Icon::Fork,
                                    IconSize::COMPACT,
                                    t.text_soft,
                                ),
                            ),
                        )
                        .on_click(cx.listener(|ws, _, window, cx| {
                            ws.start_fork(window, cx)
                        })),
                )
            })
            // The run's details beside the transcript: its plugins,
            // budget and events.
            .when(self.width >= NARROW_MAX, |header| {
                header.child(
                    div()
                        .id("details")
                        .child(ui::button(
                            if self.details_open {
                                "Hide details"
                            } else {
                                "Details"
                            },
                            ButtonKind::Secondary,
                            t,
                        ))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.toggle_details(cx)),
                        ),
                )
            })
            .when(live, |header| {
                header.child(
                    div()
                        .id("cancel")
                        .child(ui::button("Cancel", ButtonKind::Danger, t))
                        .on_click(cx.listener(|ws, _, _, cx| ws.cancel(cx))),
                )
            })
    }

    pub(super) fn inspector(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let run = self.current();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .chrome(ui::Edge::Right, t)
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px(sp(4.))
                    .border_b_1()
                    .border_color(t.border)
                    .children(
                        run.map(|run| inspector::header(self, run, t, cx)),
                    ),
            )
            .child(
                div()
                    .id("inspector")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p(sp(4.))
                    .children(
                        run.map(|run| inspector::content(self, run, t, cx)),
                    ),
            )
            .children(run.map(|run| inspector::events(self, run, t, cx)))
    }

    /// The run screen: its header, transcript and composer.
    pub(super) fn run_screen(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .when(!compact, |screen| screen.child(self.run_header(t, cx)))
            .when(self.route != Route::NewRun, |screen| {
                // What plugins put above the transcript.
                let banners = self
                    .current()
                    .map(|run| {
                        self.contributions(
                            tau_ui_plugin::points::RUN_BANNER,
                            &tau_ui_plugin::points::AtRun { run: run.info() },
                            cx,
                        )
                    })
                    .unwrap_or_default();
                screen.children(banners)
            })
            .child(self.transcript(compact, t, cx))
            .child({
                // A plugin that needs the person's input more than a
                // message draws in the composer's place; once none does,
                // the composer takes the keys back.
                let instead = self
                    .current()
                    .filter(|_| self.route != Route::NewRun)
                    .and_then(|run| {
                        let at =
                            tau_ui_plugin::points::AtRun { run: run.info() };
                        self.contributions(
                            tau_ui_plugin::points::COMPOSER,
                            &at,
                            cx,
                        )
                        .into_iter()
                        .next()
                    });
                if self.composer_replaced.replace(instead.is_some())
                    && instead.is_none()
                {
                    self.composer_back.set(true);
                }
                // A chat that ended takes no more messages.
                let ended = self
                    .current()
                    .filter(|_| self.route != Route::NewRun)
                    .and_then(|run| ui::ending::note(run, compact, t, cx));
                match (instead, ended) {
                    (_, Some(note)) => note.into_any_element(),
                    (Some(instead), None) => instead,
                    (None, None) => {
                        self.composer(compact, t, cx).into_any_element()
                    }
                }
            })
    }

    /// Any screen but a run's.
    pub(super) fn screen(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.route {
            Route::Home | Route::Run(_) | Route::NewRun => {
                self.run_screen(compact, t, cx).into_any_element()
            }
            Route::History => screens::history::render(self, compact, t, cx),
            Route::Compare { main, fork } => {
                screens::compare::render(self, main, fork, compact, t, cx)
            }
            Route::Plugins => screens::plugins::render(self, compact, t, cx),
            Route::Plan(run) => {
                screens::plan::render(self, run, compact, t, cx)
            }
            Route::Setup(_) | Route::Pair(_) | Route::PullRequest(_) => {
                self.focused(compact, t, cx)
            }
            Route::Models => screens::models::render(self, compact, t, cx),
            Route::Phones => screens::phones::render(self, compact, t, cx),
            Route::Repo(repo) => {
                screens::repo::render(self, repo, compact, t, cx)
            }
            Route::Plugin {
                plugin,
                page,
                params,
            } => {
                let page = self
                    .plugin_page(plugin, page, params, cx)
                    .unwrap_or_else(|| {
                        ui::screen(
                            "plugin-page",
                            compact,
                            ui::empty("This page's plugin is not here.", t),
                        )
                        .into_any_element()
                    });
                // A page a repository lists is one of its tabs.
                match screens::repo::owner(self, &self.route, cx) {
                    Some(repo) => {
                        screens::repo::framed(self, &repo, page, compact, t, cx)
                    }
                    None => page,
                }
            }
        }
    }

    pub(super) fn desktop(
        &self,
        wide: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.route.is_focused() {
            return self.focused(false, t, cx);
        }
        let details = self.details_open
            && matches!(self.route, Route::Home | Route::Run(_));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(
                        chrome::sidebar(self, t, cx)
                            .w(sidebar_width(wide))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(self.screen(false, t, cx)),
                    )
                    .when(wide && details, |row| {
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
            .children(chrome::status_bar(self, t))
            .into_any_element()
    }

    /// A screen that takes the whole window.
    pub(super) fn focused(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.route {
            Route::Setup(step) => {
                screens::setup::render(self, *step, compact, t, cx)
            }
            Route::Pair(step) => {
                screens::pairing::render(self, *step, compact, t, cx)
            }
            Route::PullRequest(run) => {
                screens::pull_request::render(self, run, compact, t, cx)
            }
            _ => self.screen(compact, t, cx),
        }
    }

    pub(super) fn phone(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.route.is_focused() {
            return self.focused(true, t, cx);
        }
        let screen = div().size_full().relative().flex().flex_col();
        match (&self.route, self.current()) {
            (Route::Run(_), Some(run)) => screen
                .child(chrome::phone_run_bar(run, self.is_main(&run.id), t, cx))
                .child(self.run_screen(true, t, cx))
                .when(self.sheet_open, |screen| {
                    screen.child(self.sheet(run, t, cx))
                })
                .into_any_element(),
            (Route::Home, _) => screen
                .child(chrome::phone_header(self, t, cx))
                .child(chrome::phone_run_list(self, t, cx))
                .child(chrome::phone_tab_bar(self, t, cx))
                .into_any_element(),
            _ => screen
                .child(chrome::phone_header(self, t, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .child(self.screen(true, t, cx)),
                )
                .when(self.route.is_top_level(), |screen| {
                    screen.child(chrome::phone_tab_bar(self, t, cx))
                })
                .into_any_element(),
        }
    }

    pub(super) fn sheet(
        &self,
        run: &RunView,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let live = run.status.is_live();
        let done = self.catalog.pull_requests
            && run.status == RunStatus::Finished(StopReason::Stop)
            && !self.is_main(&run.id);
        // A main chat with changes GitHub lacks pushes from here.
        let push = self
            .main_repo(&run.id)
            .filter(|repo| self.unpushed(repo).is_some())
            .map(str::to_owned);
        let id = run.id.clone();
        div()
            .absolute()
            .inset_0()
            .occlude()
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
                    .gap(sp(4.))
                    .px(sp(4.))
                    .pt(sp(2.))
                    .pb(sp(6.))
                    .raised(t)
                    .border_t_1()
                    .border_color(t.border_strong)
                    .rounded_t(radius::SHEET)
                    .child(
                        div().flex().justify_center().child(
                            div()
                                .w(px(40.))
                                .h(px(4.))
                                .rounded(radius::HAIRLINE)
                                .bg(t.border_strong),
                        ),
                    )
                    .child(inspector::header(self, run, t, cx))
                    .child(
                        div()
                            .id("sheet-body")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .child(inspector::content(self, run, t, cx)),
                    )
                    .child(inspector::events(self, run, t, cx))
                    .child(
                        div()
                            .grid()
                            .grid_cols(
                                (usize::from(self.can_fork(run))
                                    + usize::from(live || done)
                                    + usize::from(push.is_some()))
                                .max(1) as u16,
                            )
                            .gap(sp(2.))
                            .when(self.can_fork(run), |row| {
                                row.child(
                                    div()
                                        .id("sheet-fork")
                                        .child(ui::big_button(
                                            "Fork here",
                                            None,
                                            ButtonKind::Secondary,
                                            t,
                                        ))
                                        .on_click(cx.listener(
                                            |ws, _, window, cx| {
                                                ws.start_fork(window, cx)
                                            },
                                        )),
                                )
                            })
                            .when_some(push, |row, repo| {
                                row.child(
                                    div()
                                        .id("sheet-push")
                                        .child(ui::big_button(
                                            "Push to GitHub",
                                            Some(Icon::Push),
                                            ButtonKind::Primary,
                                            t,
                                        ))
                                        .on_click(cx.listener(
                                            move |ws, _, _, cx| {
                                                ws.push(&repo, false, cx)
                                            },
                                        )),
                                )
                            })
                            .when(done, |row| {
                                row.child(
                                    div()
                                        .id("sheet-pull-request")
                                        .child(ui::big_button(
                                            "Pull request",
                                            None,
                                            ButtonKind::Secondary,
                                            t,
                                        ))
                                        .on_click(cx.listener(
                                            move |ws, _, _, cx| {
                                                ws.open_pull_request(&id, cx)
                                            },
                                        )),
                                )
                            })
                            .when(live, |row| {
                                row.child(
                                    div()
                                        .id("sheet-cancel")
                                        .child(ui::big_button(
                                            "Cancel run",
                                            None,
                                            ButtonKind::Danger,
                                            t,
                                        ))
                                        .on_click(cx.listener(
                                            |ws, _, _, cx| ws.cancel(cx),
                                        )),
                                )
                            }),
                    ),
            )
    }
}
