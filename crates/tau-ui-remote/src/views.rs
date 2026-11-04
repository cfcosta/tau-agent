//! Plugins' views built for a phone (ADR 0046): the computer builds the
//! view a [`ViewKey`] names from its own state, as it would draw it in
//! the phone's layout, and the phone draws what comes back.

use std::collections::BTreeMap;

use serde::Serialize;
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
    wire::{AtKey, Built, Contributed, Layout, View, ViewKey},
};

use crate::{
    plugins::registry,
    view::{Item, RunView},
    workspace::Workspace,
};

impl Workspace {
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
            // Setup stays on the computer.
            _ => Built::Nothing,
        }
    }

    /// Each plugin's output at `point`, with the context `at` gives it,
    /// as JSON.
    fn outs<'c, Cx: PointCx, Out: Serialize + 'static>(
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
                    out: serde_json::to_value(out).ok()?,
                })
            })
            .collect();
        Built::Outs { outs }
    }

    /// Each plugin's output at a card's `point`, each given its anchors
    /// on the card.
    fn card_outs<Out: Serialize + 'static>(
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
