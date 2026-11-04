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

/// What a phone has of plugins' views: each one the computer sent, and
/// those it lacks, to ask for once the window has drawn.
#[derive(Debug, Default)]
pub struct ViewCache {
    built: HashMap<ViewKey, Built>,
    /// When each view was last drawn.
    drawn: HashMap<ViewKey, std::time::Instant>,
    /// Drawn without an answer, and not asked for yet.
    missing: BTreeSet<ViewKey>,
    /// Asked for, with no answer yet.
    asked: BTreeSet<ViewKey>,
}

impl ViewCache {
    /// The view `key` names, if the computer sent it; else it is asked
    /// for once the window has drawn.
    pub fn get(&mut self, key: &ViewKey) -> Option<&Built> {
        self.drawn.insert(key.clone(), std::time::Instant::now());
        if !self.built.contains_key(key) && !self.asked.contains(key) {
            self.missing.insert(key.clone());
        }
        self.built.get(key)
    }

    /// Drops the views not drawn for [`KEEP`] by `now`, and gives them:
    /// the computer need not build them for this phone any more.
    fn take_stale(&mut self, now: std::time::Instant) -> Vec<ViewKey> {
        let stale: Vec<ViewKey> = self
            .built
            .keys()
            .filter(|key| {
                self.drawn
                    .get(*key)
                    .is_none_or(|at| now.saturating_duration_since(*at) >= KEEP)
            })
            .cloned()
            .collect();
        for key in &stale {
            self.built.remove(key);
            self.drawn.remove(key);
        }
        stale
    }

    /// The views to ask for now: those drawn without one since the last
    /// ask.
    fn take_missing(&mut self) -> Vec<ViewKey> {
        let missing = std::mem::take(&mut self.missing);
        self.asked.extend(missing.iter().cloned());
        missing.into_iter().collect()
    }

    /// Keeps what the computer sent.
    fn take(&mut self, answers: Vec<(ViewKey, Built)>) {
        let now = std::time::Instant::now();
        for (key, built) in answers {
            self.asked.remove(&key);
            // Kept from when it came, as if drawn then.
            self.drawn.entry(key.clone()).or_insert(now);
            self.built.insert(key, built);
        }
    }

    /// Asks again for everything the phone has, as the computer it
    /// reaches may not know what it asked: it shows what it had until
    /// the answers come.
    fn ask_again(&mut self) {
        self.asked.clear();
        self.missing.extend(self.built.keys().cloned());
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
        Some(cache.borrow_mut().get(&key).cloned())
    }

    /// Asks the computer for the views this window drew without, and
    /// tells it of those it stopped drawing.
    pub fn ask_for_views(&mut self, cx: &mut Context<Self>) {
        let Some(cache) = &self.remote_views else {
            return;
        };
        let (missing, stale) = {
            let mut cache = cache.borrow_mut();
            (
                cache.take_missing(),
                cache.take_stale(std::time::Instant::now()),
            )
        };
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
        let (shown, gone) = (key("shown"), key("gone"));
        assert!(cache.get(&shown).is_none());
        assert!(cache.get(&gone).is_none());
        assert_eq!(cache.take_missing().len(), 2);
        assert!(cache.get(&shown).is_none());
        assert!(cache.take_missing().is_empty(), "asked, not again");
        cache.take(vec![
            (shown.clone(), Built::Nothing),
            (gone.clone(), Built::Nothing),
        ]);
        let start = std::time::Instant::now();
        assert!(cache.take_stale(start).is_empty());
        // Only `shown` is drawn again; `gone`'s last draw is older.
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(cache.get(&shown).is_some());
        let later = start + KEEP + std::time::Duration::from_millis(1);
        assert_eq!(cache.take_stale(later), std::slice::from_ref(&gone));
        assert!(cache.get(&gone).is_none(), "dropped, so asked again");
        assert_eq!(cache.take_missing(), [gone]);
    }
}
