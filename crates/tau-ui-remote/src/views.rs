//! Plugins' views built for a phone (ADR 0046): the computer builds the
//! view a [`ViewKey`] names from its own state, as it would draw it in
//! the phone's layout, and the phone draws what comes back.
//!
//! - On the phone, a [`ViewCache`] holds what came back; a view it lacks
//!   is asked for once the window has drawn, and draws as nothing until
//!   it comes.
//! - On the computer, [`Workspace::ask_views`] builds and keeps what
//!   phones asked for, and [`Workspace::refresh_views`] builds them again
//!   after a change, giving those whose answer changed.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use gpui::Context;
use statig::{
    Outcome::{self, Handled, Super, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};
use tau_ui_plugin::{
    Point,
    PointCx,
    points::{
        self,
        AtAnchor,
        AtApp,
        AtCard,
        AtMessage,
        AtRepo,
        AtRewrite,
        AtRun,
    },
    wire::{AtKey, Built, Contributed, Layout, View, ViewKey, Wired},
};

use crate::{
    plugins::registry,
    update::HostUpdate,
    view::{Item, RunView},
    workspace::{Workspace, WorkspaceEvent},
};

/// How long a phone keeps a view it no longer draws: after it, the phone
/// drops it, and the computer stops building it for this phone.
pub const KEEP: std::time::Duration = std::time::Duration::from_secs(60);

/// What a phone has of plugins' views: each one it drew, asked for or
/// not, and what the computer sent of it.
#[derive(Default)]
pub struct ViewCache {
    views: HashMap<ViewKey, StateMachine<Slot>>,
}

impl std::fmt::Debug for ViewCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.views.iter().map(|(key, slot)| (key, slot.state())))
            .finish()
    }
}

/// One view the phone drew: when it last did, and where it is with the
/// computer.
struct Slot {
    drawn: std::time::Instant,
}

/// What happens to a view the phone drew.
enum Input {
    /// The phone drew it then.
    Drawn(std::time::Instant),
    /// It is asked for.
    Asked,
    /// The computer sent it, built so.
    Answered(Built),
    /// The computer the phone reaches may not know it asked.
    Reconnected,
}

#[state_machine(initial = "State::missing(None)", state(derive(Debug, Clone)))]
impl Slot {
    /// Drawn and not asked for: shows what it `had`, if anything, until
    /// it is asked for and comes.
    #[state(superstate = "drawn")]
    fn missing(had: &mut Option<Built>, event: &Input) -> Outcome<State> {
        match event {
            Input::Asked => Transition(State::asked(had.take())),
            _ => Super,
        }
    }

    /// Asked for, with no answer yet.
    #[state(superstate = "drawn")]
    fn asked(had: &mut Option<Built>, event: &Input) -> Outcome<State> {
        match event {
            Input::Answered(built) => Transition(State::built(built.clone())),
            Input::Reconnected => Transition(State::missing(had.take())),
            _ => Super,
        }
    }

    /// As the computer last sent it: it builds it for this phone.
    #[state(superstate = "drawn")]
    fn built(built: &mut Built, event: &Input) -> Outcome<State> {
        match event {
            Input::Answered(again) => {
                *built = again.clone();
                Handled
            }
            Input::Reconnected => {
                Transition(State::missing(Some(built.clone())))
            }
            _ => Super,
        }
    }

    #[superstate]
    fn drawn(&mut self, event: &Input) -> Outcome<State> {
        match event {
            Input::Drawn(at) => {
                self.drawn = *at;
                Handled
            }
            // Not asked for: another phone's answer, which the computer
            // does not build again for this one.
            Input::Answered(_) | Input::Asked | Input::Reconnected => Handled,
        }
    }
}

impl State {
    /// What the phone draws of it.
    fn shown(&self) -> Option<&Built> {
        match self {
            Self::Built { built } => Some(built),
            Self::Missing { had } | Self::Asked { had } => had.as_ref(),
        }
    }
}

impl ViewCache {
    /// The view `key` names, as the computer sent it, drawn at `now`; a
    /// view the phone lacks is asked for once the window has drawn.
    pub fn get(
        &mut self,
        key: &ViewKey,
        now: std::time::Instant,
    ) -> Option<&Built> {
        let slot = self
            .views
            .entry(key.clone())
            .or_insert_with(|| Slot { drawn: now }.state_machine());
        slot.handle(&Input::Drawn(now));
        slot.state().shown()
    }

    /// A frame drew at `now`: the views to ask for, and those to drop.
    /// The views not drawn for [`KEEP`] go first, so none is asked for
    /// as it goes.
    fn drawn_frame(
        &mut self,
        now: std::time::Instant,
    ) -> (Vec<ViewKey>, Vec<ViewKey>) {
        let stale = self.take_stale(now);
        (self.take_missing(), stale)
    }

    /// Drops the views not drawn for [`KEEP`] by `now`, and gives those
    /// the computer may build for this phone: it need not any more.
    fn take_stale(&mut self, now: std::time::Instant) -> Vec<ViewKey> {
        let mut stale = Vec::new();
        self.views.retain(|key, slot| {
            let kept = now.saturating_duration_since(slot.drawn) < KEEP;
            let known = !matches!(slot.state(), State::Missing { had: None });
            if !kept && known {
                stale.push(key.clone());
            }
            kept
        });
        stale.sort();
        stale
    }

    /// The views to ask for now: those drawn without one since the last
    /// ask.
    fn take_missing(&mut self) -> Vec<ViewKey> {
        let mut missing: Vec<ViewKey> = self
            .views
            .iter_mut()
            .filter(|(_, slot)| matches!(slot.state(), State::Missing { .. }))
            .map(|(key, slot)| {
                slot.handle(&Input::Asked);
                key.clone()
            })
            .collect();
        missing.sort();
        missing
    }

    /// Keeps what the computer sent of the views this phone asked for or
    /// has; the others were for another phone.
    fn take(&mut self, answers: Vec<(ViewKey, Built)>) {
        for (key, built) in answers {
            if let Some(slot) = self.views.get_mut(&key) {
                slot.handle(&Input::Answered(built));
            }
        }
    }

    /// Asks again for everything the phone has, as the computer it
    /// reaches may not know what it asked: it shows what it had until
    /// the answers come.
    fn ask_again(&mut self) {
        for slot in self.views.values_mut() {
            slot.handle(&Input::Reconnected);
        }
    }
}

/// A view phones asked for, on the computer: as last built, and the
/// phones that draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    built: Built,
    by: BTreeSet<String>,
}

impl Workspace {
    /// Draws plugins' views from what the computer builds (ADR 0046):
    /// a phone's window.
    pub fn draw_remote_views(&mut self) {
        self.remote_views = Some(std::cell::RefCell::new(ViewCache::default()));
    }

    /// Whether this window draws plugins' views from the computer.
    pub fn draws_remote_views(&self) -> bool {
        self.remote_views.is_some()
    }

    /// This window's layout, as a view built for it reads it.
    pub fn view_layout(&self) -> Layout {
        Layout::of(self.compact(), f32::from(self.screen_width()))
    }

    /// The view `view` in this window's layout, as the computer sent it;
    /// `None` on a window that builds its own, or before it came.
    pub(crate) fn remote_view(&self, view: View) -> Option<Option<Built>> {
        let cache = self.remote_views.as_ref()?;
        let key = ViewKey {
            view,
            layout: self.view_layout(),
        };
        Some(
            cache
                .borrow_mut()
                .get(&key, std::time::Instant::now())
                .cloned(),
        )
    }

    /// Asks the computer for the views this window drew without, and
    /// tells it of those it stopped drawing.
    pub fn ask_for_views(&mut self, cx: &mut Context<Self>) {
        let Some(cache) = &self.remote_views else {
            return;
        };
        let (missing, stale) =
            cache.borrow_mut().drawn_frame(std::time::Instant::now());
        if !missing.is_empty() {
            cx.emit(WorkspaceEvent::AskViews(missing));
        }
        if !stale.is_empty() {
            cx.emit(WorkspaceEvent::DropViews(stale));
        }
    }

    /// Keeps the views the computer sent, and draws them.
    pub(crate) fn take_views(
        &mut self,
        answers: Vec<(ViewKey, Built)>,
        cx: &mut Context<Self>,
    ) {
        if let Some(cache) = &self.remote_views {
            cache.borrow_mut().take(answers);
            cx.notify();
        }
    }

    /// After a snapshot, the computer may not know what this window
    /// asked: it asks again.
    pub(crate) fn ask_views_again(&mut self) {
        if let Some(cache) = &self.remote_views {
            cache.borrow_mut().ask_again();
        }
    }

    /// Builds the views the phone `device` asked for, keeps them to build
    /// again after a change, and gives them to send.
    pub fn ask_views(
        &mut self,
        device: &str,
        keys: Vec<ViewKey>,
    ) -> HostUpdate {
        let answers: Vec<(ViewKey, Built)> = keys
            .into_iter()
            .map(|key| {
                let built = self.build_view(&key);
                (key, built)
            })
            .collect();
        for (key, built) in &answers {
            let asked =
                self.phone_views
                    .entry(key.clone())
                    .or_insert_with(|| Asked {
                        built: built.clone(),
                        by: BTreeSet::new(),
                    });
            asked.built = built.clone();
            asked.by.insert(device.to_owned());
        }
        HostUpdate::Views { answers }
    }

    /// The phone `device` no longer draws `keys`: they are built again
    /// only for the phones that still do.
    pub fn drop_views(&mut self, device: &str, keys: &[ViewKey]) {
        for key in keys {
            if let Some(asked) = self.phone_views.get_mut(key) {
                asked.by.remove(device);
                if asked.by.is_empty() {
                    self.phone_views.remove(key);
                }
            }
        }
    }

    /// The phone `device` left: no view is built for it any more (ADR
    /// 0046). It asks again for what it draws when it comes back.
    pub fn forget_phone(&mut self, device: &str) {
        self.phone_views.retain(|_, asked| {
            asked.by.remove(device);
            !asked.by.is_empty()
        });
    }

    /// Builds again every view phones asked for, and gives those whose
    /// answer changed, if any did.
    pub fn refresh_views(&mut self) -> Option<HostUpdate> {
        let keys: Vec<ViewKey> = self.phone_views.keys().cloned().collect();
        let mut answers = Vec::new();
        for key in keys {
            let built = self.build_view(&key);
            if let Some(asked) = self.phone_views.get_mut(&key)
                && asked.built != built
            {
                asked.built = built.clone();
                answers.push((key, built));
            }
        }
        (!answers.is_empty()).then_some(HostUpdate::Views { answers })
    }

    /// The views phones asked for, for tests: each with the phones that
    /// still draw it.
    pub fn asked_views(&self) -> BTreeMap<ViewKey, BTreeSet<String>> {
        self.phone_views
            .iter()
            .map(|(key, asked)| (key.clone(), asked.by.clone()))
            .collect()
    }

    /// Forgets what phones asked for: none is connected.
    pub fn forget_views(&mut self) {
        self.phone_views.clear();
    }

    /// The view `key` names, built from what this window has: the same
    /// for the same state, since views are pure (ADR 0045).
    pub fn build_view(&self, key: &ViewKey) -> Built {
        let layout = Some(key.layout);
        match &key.view {
            View::Point { point, at } => self.build_point(point, at, layout),
            View::Page {
                plugin,
                page,
                params,
            } => {
                let Some(erased) = registry().get(plugin) else {
                    return Built::Nothing;
                };
                let run = params.get("run").and_then(|id| {
                    self.runs.iter().find(|run| &*run.id.0 == id.as_str())
                });
                self.with_plugin_in(
                    erased.as_ref(),
                    run,
                    params,
                    layout,
                    |plugin, env| plugin.draw_page(page, env),
                )
                .flatten()
                .map_or(Built::Nothing, |(tree, framed)| {
                    Built::Page { tree, framed }
                })
            }
            View::Settings {
                plugin,
                scope,
                entry,
            } => {
                let Some(erased) = registry().get(plugin) else {
                    return Built::Nothing;
                };
                let mut params = BTreeMap::new();
                if let Some(scope) = scope {
                    params
                        .insert(tau_ui_plugin::SCOPE.to_owned(), scope.clone());
                }
                if let Some(entry) = entry {
                    params
                        .insert(tau_ui_plugin::ENTRY.to_owned(), entry.clone());
                }
                self.with_plugin_in(
                    erased.as_ref(),
                    None,
                    &params,
                    layout,
                    |plugin, env| plugin.draw_settings(env),
                )
                .flatten()
                .map_or(Built::Nothing, |tree| Built::Tree { tree })
            }
            View::Popover {
                plugin,
                command,
                args,
                run,
            } => {
                let Some(erased) = registry().get(plugin) else {
                    return Built::Nothing;
                };
                let run = run.as_ref().and_then(|id| self.run(id));
                self.with_plugin_in(
                    erased.as_ref(),
                    run,
                    &BTreeMap::new(),
                    layout,
                    |plugin, env| plugin.command_popover(command, args, env),
                )
                .flatten()
                .map_or(Built::Nothing, |tree| Built::Tree { tree })
            }
            View::Title {
                plugin,
                page,
                params,
            } => {
                let Some(erased) = registry().get(plugin) else {
                    return Built::Nothing;
                };
                let run = params.get("run").and_then(|id| {
                    self.runs.iter().find(|run| &*run.id.0 == id.as_str())
                });
                self.with_plugin_in(
                    erased.as_ref(),
                    run,
                    params,
                    layout,
                    |plugin, env| plugin.page_title(page, env),
                )
                .flatten()
                .map_or(Built::Nothing, |title| Built::Title { title })
            }
            View::Commands { repo } => Built::Commands {
                commands: registry()
                    .plugins()
                    .flat_map(|plugin| {
                        let name = plugin.name().to_owned();
                        self.plugin_commands_in(
                            plugin.as_ref(),
                            repo.as_deref(),
                        )
                        .into_iter()
                        .map(move |command| (name.clone(), command))
                    })
                    .collect(),
            },
        }
    }

    /// What every plugin contributes at the point named `point`, at the
    /// place `at` names, as JSON of the point's own type.
    fn build_point(
        &self,
        point: &str,
        at: &AtKey,
        layout: Option<Layout>,
    ) -> Built {
        macro_rules! at {
            ($point:expr, $context:expr) => {
                match $context {
                    Some(context) => {
                        self.outs($point, |_| Some(&context), layout)
                    }
                    None => Built::Nothing,
                }
            };
        }
        let run = || match at {
            AtKey::Run { run } => self.run(run),
            _ => None,
        };
        let at_run = || run().map(|run| AtRun { run: run.info() });
        let at_app = || matches!(at, AtKey::App).then_some(AtApp);
        let at_repo = || match at {
            AtKey::Repo { repo } => Some(AtRepo { repo: repo.clone() }),
            _ => None,
        };
        match point {
            p if p == points::TRANSCRIPT.name => {
                at!(points::TRANSCRIPT, self.at_anchor(at))
            }
            p if p == points::REWRITE.name => {
                at!(points::REWRITE, self.at_rewrite(at))
            }
            p if p == points::USER_MESSAGE.name => {
                at!(points::USER_MESSAGE, self.at_message(at))
            }
            p if p == points::CONTEXT.name => at!(points::CONTEXT, at_run()),
            p if p == points::RUN_BANNER.name => {
                at!(points::RUN_BANNER, at_run())
            }
            p if p == points::INSPECTOR.name => {
                at!(points::INSPECTOR, at_run())
            }
            p if p == points::STATUS.name => at!(points::STATUS, at_run()),
            p if p == points::PLAN.name => at!(points::PLAN, at_run()),
            p if p == points::CONTEXT_TRIGGER.name => {
                at!(points::CONTEXT_TRIGGER, at_run())
            }
            p if p == points::RUN_ROW.name => at!(points::RUN_ROW, at_run()),
            p if p == points::ASKS.name => at!(points::ASKS, at_run()),
            p if p == points::PLAN_STEPS.name => {
                at!(points::PLAN_STEPS, at_run())
            }
            p if p == points::COMPOSER.name => at!(points::COMPOSER, at_run()),
            p if p == points::COMPOSER_BAND.name => {
                at!(points::COMPOSER_BAND, at_run())
            }
            p if p == points::PANE.name => at!(points::PANE, at_run()),
            p if p == points::SIDEBAR.name => at!(points::SIDEBAR, at_app()),
            p if p == points::EVERYWHERE.name => {
                at!(points::EVERYWHERE, at_app())
            }
            p if p == points::MODELS.name => at!(points::MODELS, at_app()),
            p if p == points::SEARCH.name => at!(points::SEARCH, at_app()),
            p if p == points::PICKER_AUTO.name => {
                at!(points::PICKER_AUTO, at_app())
            }
            p if p == points::ACTIVITY.name => at!(points::ACTIVITY, at_app()),
            p if p == points::SIDEBAR_REPO.name => {
                at!(points::SIDEBAR_REPO, at_repo())
            }
            p if p == points::REPO_MENU.name => {
                at!(points::REPO_MENU, at_repo())
            }
            p if p == points::REPO_ACTIONS.name => {
                at!(points::REPO_ACTIONS, at_repo())
            }
            p if p == points::REPO_KIND.name => {
                at!(points::REPO_KIND, at_app())
            }
            p if p == points::KIND_ROW_END.name => {
                at!(points::KIND_ROW_END, at_repo())
            }
            p if p == points::KIND_ROW_LINE.name => {
                at!(points::KIND_ROW_LINE, at_repo())
            }
            p if p == points::WAITING.name => at!(points::WAITING, at_app()),
            // A card's context is each plugin's own: its anchors on it.
            p if p == points::CARD.name => {
                self.card_outs(points::CARD, at, layout)
            }
            p if p == points::CARD_BADGE.name => {
                self.card_outs(points::CARD_BADGE, at, layout)
            }
            p if p == points::CARD_BODY.name => {
                self.card_outs(points::CARD_BODY, at, layout)
            }
            p if p == points::CHANGES.name => {
                let at = match at {
                    AtKey::Changes { run, changes } => {
                        self.run(run).map(|run| points::AtChanges {
                            run: run.info(),
                            changes: changes.clone(),
                        })
                    }
                    _ => None,
                };
                at!(points::CHANGES, at)
            }
            p if p == points::CARD_STAT.name => {
                self.card_outs(points::CARD_STAT, at, layout)
            }
            // Setup stays on the computer.
            _ => Built::Nothing,
        }
    }

    /// Each plugin's output at `point`, with the context `at` gives it,
    /// as JSON.
    fn outs<'c, Cx: PointCx, Out: Wired>(
        &self,
        point: Point<Cx, Out>,
        at: impl Fn(&str) -> Option<&'c Cx>,
        layout: Option<Layout>,
    ) -> Built {
        let outs = self
            .contributions_in(point, at, layout)
            .into_iter()
            .filter_map(|(plugin, out)| {
                Some(Contributed {
                    plugin: plugin.to_owned(),
                    out: out.to_wire()?,
                })
            })
            .collect();
        Built::Outs { outs }
    }

    /// Each plugin's output at a card's `point`, each given its anchors
    /// on the card.
    fn card_outs<Out: Wired>(
        &self,
        point: Point<AtCard, Out>,
        at: &AtKey,
        layout: Option<Layout>,
    ) -> Built {
        let AtKey::Card { run, call } = at else {
            return Built::Nothing;
        };
        let Some(run) = self.run(run) else {
            return Built::Nothing;
        };
        let Some(card) = run.items.iter().find_map(|item| match item {
            Item::Tool(card) if &card.call_id == call => Some(card),
            _ => None,
        }) else {
            return Built::Nothing;
        };
        let contexts: Vec<(&'static str, AtCard)> = registry()
            .plugins()
            .map(|plugin| (plugin.name(), at_card(run, card, plugin.name())))
            .collect();
        self.outs(
            point,
            |plugin| {
                contexts
                    .iter()
                    .find(|(name, _)| *name == plugin)
                    .map(|(_, at)| at)
            },
            layout,
        )
    }

    /// The anchor `at` names, if its run has it there.
    fn at_anchor(&self, at: &AtKey) -> Option<AtAnchor> {
        let AtKey::Anchor { run, key, index } = at else {
            return None;
        };
        let run = self.run(run)?;
        matches!(run.items.get(*index), Some(Item::Anchor { key: there, .. }) if there == key)
            .then(|| AtAnchor {
                run: run.info(),
                key: key.clone(),
                index: *index,
            })
    }

    /// The rewrite `at` names, if its run has it there.
    fn at_rewrite(&self, at: &AtKey) -> Option<AtRewrite> {
        let AtKey::Rewrite { run, key, index } = at else {
            return None;
        };
        let run = self.run(run)?;
        match run.items.get(*index) {
            Some(Item::Rewrite {
                key: Some(there),
                tokens,
                ..
            }) if there == key => Some(AtRewrite {
                run: run.info(),
                key: key.clone(),
                tokens: *tokens,
                index: *index,
            }),
            _ => None,
        }
    }

    /// The message `at` names: the person's or tau's, at its place.
    fn at_message(&self, at: &AtKey) -> Option<AtMessage> {
        let AtKey::Message { run, index } = at else {
            return None;
        };
        let run = self.run(run)?;
        let text = match run.items.get(*index)? {
            Item::User(text) | Item::Tau(text) => text.clone(),
            _ => return None,
        };
        Some(AtMessage {
            run: run.info(),
            text,
            index: *index,
        })
    }
}

/// A card's context for `plugin`: the call, with the plugin's own
/// anchors on it, as the transcript gives it.
pub fn at_card(
    run: &RunView,
    card: &crate::view::ToolCard,
    plugin: &str,
) -> AtCard {
    AtCard {
        run: run.info(),
        call_id: card.call_id.clone(),
        tool: card.tool.clone(),
        keys: card
            .anchors
            .iter()
            .filter(|(by, _)| by == plugin)
            .map(|(_, key)| key.clone())
            .collect(),
        data: card.data.clone(),
        summary: card.summary.clone(),
        cut: card.cut.as_deref().cloned(),
    }
}

#[cfg(test)]
mod tests {
    use tau_ui_plugin::wire::{AtKey, Built, Layout, View, ViewKey};

    use super::{KEEP, ViewCache};

    fn key(point: &str) -> ViewKey {
        ViewKey {
            view: View::Point {
                point: point.to_owned(),
                at: AtKey::App,
            },
            layout: Layout::default(),
        }
    }

    /// A view drawn without an answer is asked for once; a view the
    /// phone has is kept while it draws it, and dropped once it has not
    /// for [`KEEP`].
    #[test]
    fn a_view_is_asked_once_and_dropped_once_unseen() {
        let mut cache = ViewCache::default();
        let start = std::time::Instant::now();
        let (shown, gone) = (key("shown"), key("gone"));
        assert!(cache.get(&shown, start).is_none());
        assert!(cache.get(&gone, start).is_none());
        assert_eq!(cache.take_missing().len(), 2);
        assert!(cache.get(&shown, start).is_none());
        assert!(cache.take_missing().is_empty(), "asked, not again");
        cache.take(vec![
            (shown.clone(), Built::Nothing),
            (gone.clone(), Built::Nothing),
        ]);
        assert!(cache.take_stale(start).is_empty());
        // Only `shown` is drawn again; `gone`'s last draw is older.
        let soon = start + std::time::Duration::from_millis(5);
        assert!(cache.get(&shown, soon).is_some());
        let later = start + KEEP + std::time::Duration::from_millis(1);
        assert_eq!(cache.take_stale(later), std::slice::from_ref(&gone));
        assert!(cache.get(&gone, later).is_none(), "dropped, so asked again");
        assert_eq!(cache.take_missing(), [gone]);
    }

    /// Answers go to every phone. A phone keeps none for a view it never
    /// drew, nor for one it drew and has not asked for yet: the computer
    /// does not build those for it, so it would never hear their
    /// changes. It asks for the second itself.
    #[test]
    fn a_phone_keeps_no_answer_it_did_not_ask_for() {
        let now = std::time::Instant::now();
        let (drawn, unseen) = (key("drawn"), key("unseen"));
        let mut cache = ViewCache::default();
        assert!(cache.get(&drawn, now).is_none());
        cache.take(vec![
            (drawn.clone(), Built::Nothing),
            (unseen.clone(), Built::Nothing),
        ]);
        assert!(cache.get(&drawn, now).is_none(), "not asked for yet");
        assert!(!cache.views.contains_key(&unseen), "never drawn");
        assert_eq!(cache.drawn_frame(now), (vec![drawn], vec![]));
    }

    /// A view not drawn for [`KEEP`] when the phone comes back is dropped,
    /// not asked for again, and the answer to an ask still on its way is
    /// not kept: drawn again, it is asked for afresh, so the computer
    /// builds it for this phone.
    #[test]
    fn a_view_stale_when_the_phone_comes_back_is_asked_afresh() {
        let start = std::time::Instant::now();
        let view = key("view");
        let mut cache = ViewCache::default();
        cache.get(&view, start);
        assert_eq!(cache.drawn_frame(start), (vec![view.clone()], vec![]));
        cache.take(vec![(view.clone(), Built::Nothing)]);
        let later = start + KEEP + std::time::Duration::from_secs(1);
        cache.ask_again();
        assert_eq!(cache.drawn_frame(later), (vec![], vec![view.clone()]));
        cache.take(vec![(view.clone(), Built::Nothing)]);
        assert!(cache.get(&view, later).is_none(), "dropped, not kept");
        assert_eq!(cache.drawn_frame(later), (vec![view], vec![]));
    }
}

#[cfg(test)]
mod model {
    use std::{
        collections::{BTreeMap, BTreeSet},
        time::{Duration, Instant},
    };

    use hegel::{TestCase, generators as gs};
    use tau_ui_plugin::wire::{AtKey, Built, Layout, View, ViewKey};

    use super::{KEEP, ViewCache};

    const KEYS: [&str; 3] = ["a", "b", "c"];
    const PHONES: usize = 2;

    fn key(point: &str) -> ViewKey {
        ViewKey {
            view: View::Point {
                point: point.to_owned(),
                at: AtKey::App,
            },
            layout: Layout::default(),
        }
    }

    /// What the computer builds for `key` at `version` of its state.
    fn built(key: &ViewKey, version: u32) -> Built {
        Built::Title {
            title: format!("{key:?} v{version}"),
        }
    }

    /// Phones drawing plugins' views from a computer that builds them
    /// (`Workspace::ask_views`, `drop_views`, `forget_phone`,
    /// `refresh_views`), over a link that carries each message at once,
    /// its answers going to every phone as the mirror sends them. Time
    /// passes, phones draw views and finish frames, reconnect, and the
    /// computer's state changes.
    struct Phones {
        start: Instant,
        now: Duration,
        phones: Vec<ViewCache>,
        /// The phones the computer builds each view for.
        asked: BTreeMap<ViewKey, BTreeSet<usize>>,
        /// The computer's state, as each view would show it.
        version: u32,
    }

    impl Phones {
        fn now(&self) -> Instant {
            self.start + self.now
        }

        /// The computer's answers, to every phone.
        fn answer(&mut self, answers: Vec<(ViewKey, Built)>) {
            for phone in &mut self.phones {
                phone.take(answers.clone());
            }
        }
    }

    #[hegel::state_machine]
    impl Phones {
        /// A phone draws a view.
        #[rule(weight = 4)]
        fn draw(&mut self, tc: TestCase) {
            let phone = tc.draw(gs::integers().max_value(PHONES - 1));
            let key = key(tc.draw(gs::sampled_from(KEYS.to_vec())));
            let now = self.now();
            self.phones[phone].get(&key, now);
        }

        /// A phone's frame ends: it asks for what it lacks and drops what
        /// it stopped drawing, and the computer answers.
        #[rule(weight = 4)]
        fn frame(&mut self, tc: TestCase) {
            let phone = tc.draw(gs::integers().max_value(PHONES - 1));
            let now = self.now();
            let (asks, drops) = self.phones[phone].drawn_frame(now);
            let answers: Vec<(ViewKey, Built)> = asks
                .into_iter()
                .map(|key| {
                    self.asked.entry(key.clone()).or_default().insert(phone);
                    let built = built(&key, self.version);
                    (key, built)
                })
                .collect();
            for key in drops {
                if let Some(by) = self.asked.get_mut(&key) {
                    by.remove(&phone);
                    if by.is_empty() {
                        self.asked.remove(&key);
                    }
                }
            }
            self.answer(answers);
        }

        #[rule(weight = 2)]
        fn time_passes(&mut self, tc: TestCase) {
            let seconds = tc.draw(gs::integers::<u64>().max_value(90));
            self.now += Duration::from_secs(seconds);
        }

        /// A phone's link drops and comes back: the computer forgets it,
        /// and it asks again for what it has.
        #[rule]
        fn reconnect(&mut self, tc: TestCase) {
            let phone = tc.draw(gs::integers().max_value(PHONES - 1));
            self.asked.retain(|_, by| {
                by.remove(&phone);
                !by.is_empty()
            });
            self.phones[phone].ask_again();
        }

        /// The computer's state changes: it builds again what phones
        /// asked for, and sends what changed.
        #[rule(weight = 2)]
        fn change(&mut self, _tc: TestCase) {
            self.version += 1;
            let answers: Vec<(ViewKey, Built)> = self
                .asked
                .keys()
                .map(|key| (key.clone(), built(key, self.version)))
                .collect();
            self.answer(answers);
        }

        /// A view a phone drew within [`KEEP`] and has shows the
        /// computer's state as it is now: the computer builds it for that
        /// phone, so no change passes it by.
        #[invariant(always_run)]
        fn a_view_drawn_lately_is_up_to_date(&self, _tc: TestCase) {
            let now = self.now();
            for (index, phone) in self.phones.iter().enumerate() {
                for (key, slot) in &phone.views {
                    let super::State::Built { built: have } = slot.state()
                    else {
                        continue;
                    };
                    if now.saturating_duration_since(slot.drawn) >= KEEP {
                        continue;
                    }
                    assert!(
                        self.asked
                            .get(key)
                            .is_some_and(|by| by.contains(&index)),
                        "phone {index} draws {key:?}, which is not built for it"
                    );
                    assert_eq!(
                        have,
                        &built(key, self.version),
                        "phone {index}"
                    );
                }
            }
        }
    }

    #[hegel::test(test_cases = 1000)]
    fn views_drawn_lately_keep_up_with_the_computer(tc: TestCase) {
        let machine = Phones {
            start: Instant::now(),
            now: Duration::ZERO,
            phones: (0..PHONES).map(|_| ViewCache::default()).collect(),
            asked: BTreeMap::new(),
            version: 0,
        };
        hegel::stateful::machine(machine).steps(40).run(tc);
    }
}
