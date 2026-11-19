//! Drawing the workspace: Home, the run screen, the desktop and phone
//! layouts, and the transcript.

use super::*;
use crate::attention::{Happened, Sign};

/// The desktop sidebar's width, its rail and its tree: wider on a wide
/// window.
pub(super) fn sidebar_width(wide: bool) -> gpui::Rems {
    rems(if wide { 19. } else { 17. })
}

/// How far a desktop screen stands in from the sidebar, and from the
/// window's right edge, in spacing steps.
const INSET: (f32, f32) = (13., 11.);

/// How far a desktop pane's bar and body stand in from its left edge,
/// in spacing steps.
const PANE_INSET: f32 = 7.;

/// How far a phone's screen stands in from the window's sides, in
/// spacing steps: 18 px, as its header and lists do.
const PHONE_GUTTER: f32 = 4.5;

impl Workspace {
    /// How wide a screen beside the sidebar is drawn: the window less
    /// the sidebar, or the whole window on a phone.
    pub fn screen_width(&self) -> gpui::Pixels {
        if self.phone_preview || self.width < PHONE_MAX {
            self.width
        } else {
            let rem = px(16. * self.zoom);
            self.width
                - sidebar_width(self.width / self.zoom >= NARROW_MAX)
                    .to_pixels(rem)
        }
    }
}

impl Render for Workspace {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let _span = heiko_perf::span("render", "workspace");
        self.sync_transcript();
        self.release_focus(window, cx);
        self.plugin_focus(window, cx);
        // A phone asks for the views it draws without, once drawn.
        if self.draws_remote_views() {
            let this = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = this.update(cx, |ws, cx| ws.ask_for_views(cx));
            });
        }
        if let Route::Setup(step) = self.route {
            screens::setup::observe(self, step, window, cx);
        }
        let t = theme(cx).clone();
        // Every size is in rems: the zoom sets the rem.
        window.set_rem_size(px(16. * self.zoom));
        let (width, height) = match self.frame {
            Some((width, height)) => (px(width), px(height)),
            None => {
                let size = window.viewport_size();
                (size.width, size.height)
            }
        };
        // The system's bars take their share of the height.
        let bars = self
            .system_bars
            .as_ref()
            .map(|bars| bars())
            .unwrap_or_default();
        let height = height - px(bars.top + bars.bottom);
        self.height = height;
        // Zoomed in, the window holds less: the layouts change at the
        // widths they would for a window that much smaller.
        let phone = self.phone_preview || width / self.zoom < PHONE_MAX;
        self.width = if self.phone_preview { px(390.) } else { width };
        // A run's pane, while open, takes the inspector's place.
        let pane = self.run_pane(cx);
        self.pane_title = pane.as_ref().map(|pane| pane.title.clone());
        let pane = pane.filter(|_| {
            self.current().is_some_and(|run| self.pane_is_open(&run.id))
        });
        self.inspector_shown = !phone
            && width / self.zoom >= NARROW_MAX
            && self.details_open
            && pane.is_none()
            && matches!(self.route, Route::Run(_));
        let body = if phone {
            self.phone(pane, &t, cx)
        } else {
            self.desktop(width / self.zoom >= NARROW_MAX, pane, &t, cx)
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
        let body = if bars == SystemBars::default() {
            body
        } else {
            let band = |id: &'static str, height: f32| {
                div()
                    .debug_selector(move || id.to_owned())
                    .flex_shrink_0()
                    .h(px(height))
                    .bg(gpui::black())
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(band("system-bar-top", bars.top))
                .child(
                    div()
                        .debug_selector(|| "system-bars-between".to_owned())
                        .flex_1()
                        .min_h(px(0.))
                        .relative()
                        .overflow_hidden()
                        .child(body),
                )
                .child(band("system-bar-bottom", bars.bottom))
                .into_any_element()
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
            .on_action(cx.listener(|ws, _: &SlashComplete, window, cx| {
                if !ws.slash_complete(cx) && !ws.take_composer_hint(window, cx)
                {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|ws, _: &ZoomIn, _, cx| ws.zoom_by(1, cx)))
            .on_action(cx.listener(|ws, _: &ZoomOut, _, cx| ws.zoom_by(-1, cx)))
            .on_action(
                cx.listener(|ws, _: &ZoomReset, _, cx| ws.zoom_by(0, cx)),
            )
            .on_action(cx.listener(|ws, _: &NewRun, window, cx| {
                ws.start_new_run(window, cx)
            }))
            .on_action(cx.listener(|ws, _: &Search, window, cx| {
                ws.open_search(window, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowRuns, _, cx| {
                ws.switch_tab(route::Tab::Runs, cx)
            }))
            .on_action(cx.listener(|ws, key: &PluginKey, _, cx| {
                ws.press_plugin_key(key.plugin, key.key, cx)
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
        if self.shown_run().is_none() {
            return div()
                .id("transcript")
                .flex_1()
                .min_h(rems(0.))
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
        let list = gpui::list(
            self.transcript.clone(),
            cx.processor(move |ws, index: usize, _, cx| {
                let Some(run) = ws.shown_run() else {
                    return div().into_any_element();
                };
                transcript::item(ws, run, index, &theme, compact, cx)
            }),
        )
        .size_full();
        // The transcript reads as one document: drag across it to select,
        // ctrl+c to copy.
        div()
            .flex_1()
            .min_h(rems(0.))
            .child(heiko_ui_kit::select::selection_scope(
                "transcript-text",
                t.blue_soft,
                list,
            ))
            .into_any_element()
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
        // One more row, past the items, for an open landing's card, a
        // landed chat's, a queued chat's, or a main chat's queue,
        // conflicts and push cards.
        let now = self.shown_run().map(|run| {
            let pushed = self
                .main_repo(&run.id)
                .and_then(|repo| self.push_state(repo))
                .is_some_and(|state| {
                    !matches!(state, PushState::Pushing { .. })
                });
            let card = usize::from(
                self.landings.contains_key(&run.id)
                    || matches!(run.ending, Some(Ending::Landed { .. }))
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

    /// Land on the parent: among the header's actions, for a finished
    /// chat (ADR 0014). It opens the landing card at the end of the chat.
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
        let label = format!("land on {target}");
        let id = run.id.clone();
        Some(div().id("land").child(ui::link_action(label, t)).on_click(
            cx.listener(move |ws, _, _, cx| {
                ws.follow = true;
                ws.preview_landing(&id, cx)
            }),
        ))
    }

    /// A chat's header: its name, its state in words, how full its
    /// context is, then what can be done with it, Push first; over a
    /// heavy rule.
    pub(super) fn run_header(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = |title: &str, color| {
            div()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::PAGE_TITLE)
                .text_color(color)
                .child(title.to_owned())
        };
        let Some(run) = self.shown_run() else {
            return header(t).child(name("New run", t.bright));
        };
        // A chat that ended is read-only: its title, and a stamp saying
        // so.
        if run.ending.is_some() {
            return header(t).child(name(&run.title, t.muted)).child(
                ui::stamp(ui::ending::READ_ONLY, crate::theme::Tone::Quiet, t),
            );
        }
        let live = run.status.is_live();
        let (color, state): (gpui::Hsla, gpui::SharedString) = if live {
            (t.green, format!("working, turn {}", run.turn).into())
        } else {
            ui::status_look(&run.status, t)
        };
        // With the run's pane open the header has half the width: the
        // context's size goes.
        let paned = self.pane_title.is_some() && self.pane_is_open(&run.id);
        let context = run
            .context
            .window
            .filter(|window| *window > 0 && !paned)
            .map(|window| {
                format!(
                    "{} of {}",
                    crate::view::tokens(run.context.used),
                    crate::view::tokens(window)
                )
            });
        // A main chat pushes to the forge instead (ADR 0023).
        let done = self.catalog.pull_requests
            && run.status == RunStatus::Finished(StopReason::Stop)
            && !self.is_main(&run.id);
        let id = run.id.clone();
        let compare = run
            .children
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
            .map(|(main, fork)| Route::Compare { main, fork });
        let actions = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(sp(3.5))
            .children(ui::push::header(self, run, t, cx))
            .children(compare.map(|route| {
                div()
                    .id("compare")
                    .child(ui::link_action("compare forks", t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
            }))
            .when(done, |actions| {
                actions.child(
                    div()
                        .id("pull-request")
                        .child(ui::link_action("pull request", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.open_pull_request(&id, cx)
                        })),
                )
            })
            .children(self.land_button(run, t, cx))
            .when(self.can_fork(run), |actions| {
                actions.child(
                    div()
                        .id("fork")
                        .child(ui::link_action("fork", t))
                        .on_click(cx.listener(|ws, _, window, cx| {
                            ws.start_fork(window, cx)
                        })),
                )
            })
            // What a plugin shows in the run's pane, in the inspector's
            // place; the pane's own bar closes it.
            .children(self.pane_title.clone().filter(|_| !paned).map(|title| {
                div()
                    .id("pane")
                    .child(ui::link_action(title.to_lowercase(), t))
                    .on_click(cx.listener(|ws, _, _, cx| ws.toggle_pane(cx)))
            }))
            // The run's details beside the transcript: its plugins,
            // limits and events.
            .when(self.width >= NARROW_MAX, |actions| {
                actions.child(
                    div()
                        .id("details")
                        .child(ui::link_action(
                            if self.details_open && !paned {
                                "hide details"
                            } else {
                                "details"
                            },
                            t,
                        ))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.toggle_details(cx)),
                        ),
                )
            })
            .when(live, |actions| {
                actions.child(
                    div()
                        .id("cancel")
                        .child(ui::link_action("cancel", t).border_color(t.red))
                        .on_click(cx.listener(|ws, _, _, cx| ws.cancel(cx))),
                )
            });
        header(t)
            .child(name(&run.title, t.bright))
            // Small print sits on the title's baseline, not its middle.
            .child(
                div()
                    .flex_shrink_0()
                    .pt(sp(1.))
                    .text_color(color)
                    .child(state),
            )
            .children(context.map(|context| {
                ui::mono(context, Type::CAPTION, t.dim)
                    .flex_shrink_0()
                    .pt(sp(1.))
            }))
            .child(div().flex_1())
            .child(actions)
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
            .min_h(rems(0.))
            .border_l_1()
            .border_color(t.hairline)
            // As tall as the chat's header, so its heavy rule goes on
            // across the panel.
            .child(
                div()
                    .h(rems(4.75))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .mx(sp(4.))
                    .pb(sp(3.))
                    .border_b_2()
                    .border_color(t.rule)
                    .children(run.map(|run| inspector::header(self, run, t))),
            )
            .child(
                div()
                    .id("inspector")
                    .flex_1()
                    .min_h(rems(0.))
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
            .min_w(rems(0.))
            .min_h(rems(0.))
            .flex()
            .flex_col()
            .when(!compact, |screen| screen.child(self.run_header(t, cx)))
            .map(|screen| {
                // What plugins put above the transcript.
                let banners = self
                    .shown_run()
                    .map(|run| {
                        self.drawn(
                            heiko_ui_plugin::points::RUN_BANNER,
                            &heiko_ui_plugin::points::AtRun { run: run.info() },
                            cx,
                        )
                    })
                    .unwrap_or_default();
                // In the transcript's column on a desktop; on a phone,
                // in from the window's sides.
                screen.when(!banners.is_empty(), |screen| {
                    screen.child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .when(!compact, |column| {
                                column.max_w(rems(50.5)).mx_auto()
                            })
                            .px(sp(if compact { PHONE_GUTTER } else { 6. }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .pt(sp(4.))
                                    .pb(sp(3.))
                                    .border_b_1()
                                    .border_color(t.hairline)
                                    .children(banners),
                            ),
                    )
                })
            })
            .child(self.transcript(compact, t, cx))
            // A main chat's crew, docked above its composer.
            .children(
                self.shown_run()
                    .and_then(|run| ui::crew::tray(self, run, compact, t, cx))
                    .map(|tray| {
                        div()
                            .w_full()
                            .when(!compact, |column| {
                                column.max_w(rems(50.5)).mx_auto()
                            })
                            .px(sp(if compact { PHONE_GUTTER } else { 6. }))
                            .pb(sp(2.5))
                            .child(tray)
                    }),
            )
            // What plugins put between the transcript and the composer.
            .children(
                self.shown_run()
                    .map(|run| {
                        self.drawn(
                            heiko_ui_plugin::points::COMPOSER_BAND,
                            &heiko_ui_plugin::points::AtRun { run: run.info() },
                            cx,
                        )
                    })
                    .filter(|bands| !bands.is_empty())
                    .map(|bands| {
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(sp(1.5))
                            .when(!compact, |column| {
                                column.max_w(rems(50.5)).mx_auto()
                            })
                            .px(sp(if compact { PHONE_GUTTER } else { 6. }))
                            .pb(sp(2.5))
                            .children(bands)
                    }),
            )
            .child({
                // A plugin that needs the person's input more than a
                // message draws in the composer's place, taking the keys
                // as it comes back there; once none does, the composer
                // takes them back.
                self.tree_widgets.borrow_mut().composer_anew =
                    !self.composer_replaced.get();
                let instead = self.shown_run().and_then(|run| {
                    let at = heiko_ui_plugin::points::AtRun { run: run.info() };
                    self.drawn(heiko_ui_plugin::points::COMPOSER, &at, cx)
                        .into_iter()
                        .next()
                });
                self.tree_widgets.borrow_mut().composer_anew = false;
                if self.composer_replaced.replace(instead.is_some())
                    && instead.is_none()
                {
                    self.composer_back.set(true);
                }
                // A chat that ended takes no more messages.
                let ended = self
                    .shown_run()
                    .and_then(|run| ui::ending::note(run, compact, t, cx));
                match (instead, ended) {
                    (_, Some(note)) => note.into_any_element(),
                    // On a desktop, in the transcript's column, as the
                    // composer it stands in for.
                    (Some(instead), None) if !compact => div()
                        .w_full()
                        .max_w(rems(50.5))
                        .mx_auto()
                        .px(sp(6.))
                        .pb(sp(5.))
                        .child(instead)
                        .into_any_element(),
                    // On a phone, in from the window's sides as the
                    // rest of the screen is.
                    (Some(instead), None) => div()
                        .w_full()
                        .flex_shrink_0()
                        .px(sp(PHONE_GUTTER))
                        .pb(sp(4.5))
                        .child(instead)
                        .into_any_element(),
                    (None, None) => {
                        self.composer(compact, t, cx).into_any_element()
                    }
                }
            })
    }

    /// Home, on a desktop: a new run's composer over what plugins list
    /// of its repository, beside what happened since the person last
    /// looked. A narrow window stacks them.
    pub(super) fn home(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let start = div()
            .flex()
            .flex_col()
            .gap(sp(13.))
            .child(self.new_run(t, cx))
            .children(self.home_lists(t, cx));
        let page = div()
            .flex()
            .pl(sp(INSET.0))
            .pr(sp(INSET.1))
            .pt(sp(13.5))
            .pb(sp(11.));
        let page = if self.width / self.zoom >= NARROW_MAX {
            page.items_start()
                .gap(sp(14.5))
                .child(start.w(rems(26.875)).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .child(self.since_list(t, cx)),
                )
        } else {
            page.flex_col()
                .gap(sp(13.))
                .child(start)
                .child(self.since_list(t, cx))
        };
        div()
            .id("home")
            .flex_1()
            .min_h(rems(0.))
            .overflow_y_scroll()
            .child(page)
            .into_any_element()
    }

    /// A new run's composer, on Home: what to do, large, between two
    /// rules; then where it starts and on what, as words that open what
    /// changes them, and Start.
    fn new_run(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let repo = self.selected_repo().map(str::to_owned);
        let branch = repo.as_deref().map(|repo| {
            self.repo_named(repo)
                .trunk
                .clone()
                .unwrap_or_else(|| "main".to_owned())
        });
        let choice = self.choice_for(&PickerTarget::Next);
        let quiet = |text: &'static str| {
            div().flex_shrink_0().text_color(t.dim).child(text)
        };
        // A word the person can change: underlined, in its color.
        let word = |id: &'static str, text: String, color: gpui::Hsla| {
            div()
                .id(id)
                .flex_shrink_0()
                .border_b_1()
                .border_color(t.slate)
                .text_color(color)
                .cursor_pointer()
                .hover(|style| style.text_color(t.bright))
                .child(text)
        };
        let pick =
            |ws: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                if ws.picker == Some(PickerTarget::Next) {
                    ws.close_picker(cx)
                } else {
                    ws.open_picker(PickerTarget::Next, window, cx)
                }
            };
        let words = div()
            .flex_1()
            .min_w(rems(0.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(2.))
            // The repository: search lists the others.
            .when_some(repo, |row, repo| {
                row.child(quiet("in")).child(
                    word("home-repo", repo, t.text).on_click(cx.listener(
                        |ws, _, window, cx| ws.open_search(window, cx),
                    )),
                )
            })
            // A run starts from its repository's trunk.
            .when_some(branch, |row, branch| {
                row.child(quiet("from")).child(
                    div()
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(t.slate)
                        .text_color(t.roles.branch)
                        .child(branch),
                )
            })
            .child(quiet("with"))
            .child(word("home-model", choice.model.clone(), t.blue).on_click(
                cx.listener(move |ws, _, window, cx| pick(ws, window, cx)),
            ))
            .child(
                word("home-effort", choice.effort.label().to_owned(), t.text)
                    .on_click(cx.listener(move |ws, _, window, cx| {
                        pick(ws, window, cx)
                    })),
            );
        // The words wrap among themselves; Start keeps the row's end.
        let words = div()
            .flex()
            .items_center()
            .gap(sp(4.))
            .pt(sp(3.5))
            .border_t_1()
            .border_color(t.hairline)
            .typeset(Type::LEAD)
            .child(words)
            .child(
                div()
                    .id("send")
                    .flex_shrink_0()
                    .child(
                        ui::primary_block("Start", ui::BlockSize::Large, t)
                            .child(ui::mono(
                                "\u{21b5}",
                                Type::CAPTION,
                                t.roles.on_primary,
                            )),
                    )
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.submit_from_button(cx)),
                    ),
            );
        div()
            .flex()
            .flex_col()
            // Files dropped on it are attached: images as images.
            .on_drop(cx.listener(|ws, paths: &gpui::ExternalPaths, _, cx| {
                for path in paths.paths() {
                    ws.attach_path(path, cx);
                }
            }))
            .child(ui::section_header("New run", Some("ctrl n".into()), t))
            .child(
                div()
                    .min_h(rems(9.375))
                    .pt(sp(4.5))
                    .pb(sp(3.))
                    .typeset(Type::HEADLINE.sized(23.))
                    .leading(1.35)
                    .text_color(t.bright)
                    .child(self.composer.clone()),
            )
            .when(!self.attachments.is_empty(), |column| {
                column.child(
                    div().flex().flex_wrap().gap(sp(3.5)).pb(sp(3.)).children(
                        self.attachments.iter().enumerate().map(|(n, file)| {
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(1.5))
                                .child(ui::mono(
                                    file.name.clone(),
                                    Type::CAPTION,
                                    t.text_soft,
                                ))
                                .child(
                                    div()
                                        .id(("detach", n))
                                        .cursor_pointer()
                                        .child(ui::icon(
                                            Icon::Close,
                                            IconSize::SMALL,
                                            t.dim,
                                        ))
                                        .on_click(cx.listener(
                                            move |ws, _, _, cx| {
                                                ws.remove_attachment(n, cx)
                                            },
                                        )),
                                )
                        }),
                    ),
                )
            })
            .when_some(self.composer_note(), |column, note| {
                column.child(
                    div()
                        .id("composer-note")
                        .pb(sp(3.))
                        .text_color(t.green)
                        .child(note.to_owned()),
                )
            })
            .child(words)
            // The commands a slash opens, under what is written.
            .children(
                self.slash_popover(true, t, cx)
                    .map(|popover| div().pt(sp(4.)).child(popover)),
            )
    }

    /// What plugins list on Home of the selected repository
    /// ([`heiko_ui_plugin::points::HOME`]): each list with rows a titled
    /// section of ruled rows, a row its id, its title and its small
    /// print. None while no plugin lists anything.
    fn home_lists(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<gpui::Div> {
        let Some(repo) = self.selected_repo().map(str::to_owned) else {
            return Vec::new();
        };
        let at = heiko_ui_plugin::points::AtRepo { repo: repo.clone() };
        self.contributions_by(heiko_ui_plugin::points::HOME, &at)
            .into_iter()
            .filter(|(_, list)| !list.rows.is_empty())
            .enumerate()
            .map(|(nth, (plugin, list))| {
                let rows = list.rows.into_iter().enumerate().map(|(n, row)| {
                    let (repo, action) = (repo.clone(), row.action);
                    let selector = format!("home-row:{}", row.id);
                    div()
                        .id(gpui::SharedString::from(format!(
                            "home-row-{nth}-{n}"
                        )))
                        .debug_selector(move || selector.clone())
                        .flex()
                        .items_baseline()
                        .gap(sp(3.))
                        .py(sp(2.75))
                        .border_b_1()
                        .border_color(t.hairline)
                        .typeset(Type::BODY)
                        .text_color(t.text)
                        .cursor_pointer()
                        .hover(|style| style.text_color(t.bright))
                        .child(
                            ui::mono(row.id, Type::CAPTION, t.dim)
                                .min_w(rems(3.75))
                                .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(rems(0.))
                                .truncate()
                                .child(row.title),
                        )
                        .child(
                            ui::mono(row.detail, Type::CAPTION, t.dim)
                                .flex_shrink_0(),
                        )
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.home_act(plugin, &repo, &action, window, cx)
                        }))
                });
                let section = format!("home-list:{plugin}");
                div()
                    .flex()
                    .flex_col()
                    .child(
                        ui::section_header(
                            list.title,
                            list.meta.map(Into::into),
                            t,
                        )
                        .debug_selector(move || section.clone()),
                    )
                    .children(rows)
            })
            .collect()
    }

    /// What clicking a row of one of Home's lists does: its words go in
    /// the new run's composer, which takes the keys, or `plugin` carries
    /// out its action in `repo`.
    fn home_act(
        &mut self,
        plugin: &str,
        repo: &str,
        action: &heiko_ui_plugin::points::HomeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use heiko_ui_plugin::points::HomeAction;
        match action {
            HomeAction::Write(text) => {
                self.set_composer(text, cx);
                self.composer.read(cx).focus_handle(cx).focus(window, cx);
            }
            HomeAction::Act { action, args } => {
                let origin = crate::plugins::Origin {
                    run: None,
                    params: [("repo".to_owned(), repo.to_owned())].into(),
                };
                self.plugin_action(plugin, action, args, &origin, cx);
            }
        }
    }

    /// What happened across runs and agents, on Home: a ruled list,
    /// each thing with when, its mark, whose it is, one line and the one
    /// thing to do about it.
    fn since_list(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let list = self.since();
        let waiting = list.iter().filter(|happened| happened.hot).count();
        let meta = (waiting > 0).then(|| {
            gpui::SharedString::from(format!("{waiting} waiting on you"))
        });
        // As far back as the oldest thing listed.
        let oldest = list
            .iter()
            .rev()
            .find(|happened| !happened.day.is_empty())
            .map(|happened| (happened.day.as_str(), happened.when.as_str()));
        let title = match oldest {
            None | Some(("Today", _)) => "Today".to_owned(),
            // Yesterday's is said with its part of the day; older, by
            // its date.
            Some(("Yesterday", when)) => match part_of_day(when) {
                Some(part) => format!("Since yesterday {part}"),
                None => "Since yesterday".to_owned(),
            },
            Some(("Earlier", _)) => "Earlier".to_owned(),
            Some((day, _)) => format!("Since {day}"),
        };
        div()
            .flex()
            .flex_col()
            .child(ui::section_header(title, meta, t))
            .when(list.is_empty(), |column| {
                column.child(ui::empty("Nothing yet.", t))
            })
            .children(
                list.into_iter()
                    .enumerate()
                    .map(|(n, happened)| happened_row(n, happened, t, cx)),
            )
    }

    /// Any screen but a run's.
    pub(super) fn screen(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.route {
            Route::Home | Route::NewRun if !compact => self.home(t, cx),
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
            Route::Setup(_) | Route::Pair(_) => self.focused(compact, t, cx),
            // A run's pull request, beside the sidebar on a desktop.
            Route::PullRequest(run) => {
                screens::pull_request::render(self, run, compact, t, cx)
            }
            Route::Models => screens::models::render(self, compact, t, cx),
            Route::Repo(repo) => {
                screens::repo::render(self, repo, compact, t, cx)
            }
            Route::Plugin {
                plugin,
                page,
                params,
            } => self.plugin_page(plugin, page, params, cx).unwrap_or_else(
                || {
                    ui::screen(
                        "plugin-page",
                        compact,
                        ui::empty("This page's plugin is not here.", t),
                    )
                    .into_any_element()
                },
            ),
        }
    }

    /// A run's pane: its bar, with what the plugin calls it and a button
    /// that closes it, over what the plugin draws. On a phone it covers
    /// the run.
    pub(super) fn pane(
        &self,
        pane: super::DrawnPane,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // As a screen's header: on a desktop as tall as the chat's, so
        // the two heavy rules meet; on a phone as its other headers.
        let bar = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(sp(3.))
            .border_b_2()
            .border_color(t.rule)
            .when(compact, |bar| {
                bar.min_h(rems(3.5)).mx(sp(PHONE_GUTTER)).pt(sp(2.))
            })
            .when(!compact, |bar| {
                bar.min_h(rems(4.75))
                    .ml(sp(PANE_INSET))
                    .mr(sp(INSET.1))
                    .pt(sp(6.))
                    .pb(sp(2.5))
            })
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .truncate()
                    .typeset(if compact {
                        Type::PAGE_TITLE.sized(19.)
                    } else {
                        Type::SECTION
                    })
                    .text_color(t.bright)
                    .child(pane.title),
            )
            .child(
                div()
                    .id("pane-close")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .when(compact, |close| close.min_h(control::LARGE))
                    .child(ui::text_link("close", Type::SMALL, t))
                    .on_click(cx.listener(|ws, _, _, cx| ws.toggle_pane(cx))),
            );
        div()
            .flex()
            .flex_col()
            .min_w(rems(0.))
            .min_h(rems(0.))
            .bg(t.bg)
            .when(!compact, |pane| pane.border_l_1().border_color(t.hairline))
            .child(bar)
            .child(
                div()
                    .flex_1()
                    .min_h(rems(0.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    // On a phone, in from the window's sides as the
                    // screen it covers is; on a desktop, under its bar.
                    .when(compact, |body| body.px(sp(PHONE_GUTTER)))
                    .when(!compact, |body| {
                        body.pl(sp(PANE_INSET)).pr(sp(INSET.1))
                    })
                    .child(pane.body),
            )
    }

    pub(super) fn desktop(
        &self,
        wide: bool,
        pane: Option<super::DrawnPane>,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Setup and pairing take the whole window; a pull request keeps
        // the sidebar.
        if self.route.is_focused(false) {
            return self.focused(false, t, cx);
        }
        // An open pane takes the inspector's place.
        let details = self.details_open
            && pane.is_none()
            && matches!(self.route, Route::Run(_));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h(rems(0.))
                    .flex()
                    .child(
                        chrome::sidebar(self, t, cx)
                            .w(sidebar_width(wide))
                            .flex_shrink_0(),
                    )
                    // The screen: a flat plane, with the one hairline
                    // the sidebar draws between them.
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .flex()
                            .flex_col()
                            .bg(t.bg)
                            .child(self.screen(false, t, cx)),
                    )
                    .when_some(pane, |row, pane| {
                        // Beside the transcript, with a little less of the
                        // width than it.
                        row.child(
                            self.pane(pane, false, t, cx)
                                .w(self.screen_width() * 0.45)
                                .min_w(rems(24.))
                                .flex_shrink_0(),
                        )
                    })
                    .when(wide && details, |row| {
                        row.child(
                            div()
                                .w(rems(20.5))
                                .flex_shrink_0()
                                .flex()
                                .flex_col()
                                .child(self.inspector(t, cx)),
                        )
                    }),
            )
            .children(chrome::status_bar(self, t, cx))
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
            _ => self.screen(compact, t, cx),
        }
    }

    pub(super) fn phone(
        &self,
        pane: Option<super::DrawnPane>,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // On a phone a pull request takes it too.
        if self.route.is_focused(true) {
            return self.focused(true, t, cx);
        }
        let screen = div().size_full().relative().flex().flex_col();
        match (&self.route, self.current()) {
            (Route::Run(_), Some(run)) => screen
                .child(chrome::phone_run_bar(
                    self,
                    run,
                    self.is_main(&run.id),
                    self.pane_title.is_some(),
                    t,
                    cx,
                ))
                .child(self.run_screen(true, t, cx))
                .when(self.sheet_open, |screen| {
                    screen.child(self.sheet(run, t, cx))
                })
                .when_some(pane, |screen, pane| {
                    screen.child(
                        self.pane(pane, true, t, cx)
                            .absolute()
                            .inset_0()
                            .occlude(),
                    )
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
                        .min_h(rems(0.))
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
        // A main chat with changes the forge lacks pushes from here.
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
                    .px(sp(4.5))
                    .pb(sp(6.))
                    .bg(t.panel)
                    .border_t_2()
                    .border_color(t.rule)
                    .pt(sp(4.))
                    .child(inspector::header(self, run, t))
                    .child(
                        div()
                            .id("sheet-body")
                            .flex_1()
                            .min_h(rems(0.))
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
                                            "fork here",
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
                                            format!(
                                                "Push to {}",
                                                self.catalog.forge_name()
                                            ),
                                            None,
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
                                            "pull request",
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
                                            "cancel run",
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

/// A screen's header on a desktop: one row over a heavy rule, standing
/// in from the sidebar as the screen under it does.
fn header(t: &Theme) -> gpui::Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(sp(3.5))
        .min_h(rems(4.75))
        .ml(sp(INSET.0))
        .mr(sp(INSET.1))
        .pt(sp(6.))
        .pb(sp(2.5))
        .border_b_2()
        .border_color(t.rule)
        .typeset(Type::BODY)
}

/// The part of the day a time of day (`19:02`) is in, as a title says
/// it. None for anything else, such as `now`.
fn part_of_day(when: &str) -> Option<&'static str> {
    let hour: u8 = when.split_once(':')?.0.parse().ok()?;
    Some(match hour {
        0..=11 => "morning",
        12..=17 => "afternoon",
        _ => "evening",
    })
}

/// One thing that happened, in Home's list: when, its mark, whose it
/// is, what happened, and its one action; the loud one when it waits on
/// the person.
fn happened_row(
    n: usize,
    happened: Happened,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<gpui::Div> {
    let sign = match happened.sign {
        Sign::State(state) => ui::mark(state, t).into_any_element(),
        Sign::Note => div()
            .size(rems(0.875))
            .flex()
            .items_center()
            .justify_center()
            .child(ui::dot(t.roles.agent, 6.))
            .into_any_element(),
        Sign::Push => {
            ui::icon(Icon::Push, IconSize::BASE, t.blue).into_any_element()
        }
    };
    let action = if happened.hot {
        ui::hot_action(happened.action, t)
    } else {
        ui::link_action(happened.action, t)
    };
    let act = happened.act;
    div()
        .id(("since", n))
        .flex()
        .items_start()
        .gap(sp(3.))
        .py(sp(2.75))
        .border_b_1()
        .border_color(t.hairline)
        .typeset(Type::BODY)
        .leading(1.5)
        .child(
            ui::mono(happened.when, Type::CAPTION, t.dim)
                .w(rems(2.625))
                .flex_shrink_0()
                .pt(sp(0.5)),
        )
        .child(div().flex_shrink_0().pt(sp(0.75)).child(sign))
        .child(
            div()
                .w(rems(8.75))
                .flex_shrink_0()
                .truncate()
                .font_weight(weight::STRONG)
                .text_color(t.bright)
                .child(happened.name),
        )
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .text_color(t.text_soft)
                .child(happened.line),
        )
        .child(div().id("act").flex_shrink_0().child(action).on_click(
            cx.listener(move |ws, _, _, cx| ws.act_on(act.clone(), cx)),
        ))
}
