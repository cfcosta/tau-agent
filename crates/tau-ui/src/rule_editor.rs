//! The Constitution screen's workings: the rule being written or edited,
//! trying it on past calls with Jev, what the repository's runs say
//! about each rule, and what waits for a person.

use std::collections::HashMap;

use gpui::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_constitution::{Trial, rules::Target};

use crate::{
    view::{Item, ToolState},
    workspace::{Workspace, WorkspaceEvent},
};

/// The places the editor offers, with what each reads.
pub const PLACES: [(&str, &str); 4] = [
    ("bash.command", "the shell command"),
    ("edit.newText", "each replacement"),
    ("write.content", "the whole file"),
    ("final answer", "sent back when broken"),
];

/// How many past calls, and answers, a rule is tried on.
const TRIAL_CALLS: usize = 6;
const TRIAL_ANSWERS: usize = 3;

/// Thresholds by name. The editor offers these first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    Lenient,
    Balanced,
    Strict,
}

impl Preset {
    pub const ALL: [Self; 3] = [Self::Lenient, Self::Balanced, Self::Strict];

    /// Review, then block.
    pub fn marks(self) -> (f64, f64) {
        match self {
            Self::Lenient => (0.5, 0.9),
            Self::Balanced => (0.3, 0.8),
            Self::Strict => (0.2, 0.6),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lenient => "Lenient",
            Self::Balanced => "Balanced",
            Self::Strict => "Strict",
        }
    }
}

/// Which threshold a nudge moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mark {
    Review,
    Block,
}

/// What trying the rule gave.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum Trying {
    #[default]
    Not,
    Asking,
    Done {
        trials: Vec<Trial>,
        cost: f64,
    },
    Failed(String),
}

/// A rule being written (or rewritten), in the editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleDraft {
    pub repo: String,
    /// The rule being edited, or `None` for a new one.
    pub editing: Option<String>,
    /// Where it applies: `tool.field` or `final answer`.
    pub places: Vec<String>,
    pub review: f64,
    pub block: f64,
    pub trying: Trying,
    /// Whether Add was pressed with something missing: the editor then
    /// says what.
    pub tried_to_save: bool,
}

impl RuleDraft {
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|preset| {
            let (review, block) = preset.marks();
            (review - self.review).abs() < 1e-6
                && (block - self.block).abs() < 1e-6
        })
    }
}

/// Which list the Constitution screen shows.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
pub enum RulesTab {
    #[default]
    Rules,
    Review,
}

/// The most times the Constitution screen lets one run's answer be sent
/// back.
pub const MAX_HOLDS: u32 = 10;

/// What a repository's runs say about its constitution.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RulesStats {
    pub runs: usize,
    /// Calls and final answers checked.
    pub checked: u32,
    /// Checks Jev could not answer.
    pub failed: u32,
    pub blocked: usize,
    pub flagged: usize,
    /// Flagged calls and answers nobody has looked at.
    pub waiting: usize,
    /// Runs whose answer was sent back at least once.
    pub held_runs: usize,
    pub cost: f64,
    /// Per rule: blocked, flagged, held.
    pub per_rule: HashMap<String, (usize, usize, usize)>,
}

/// A flagged call or answer that waits for a person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub run: RunId,
    pub run_title: String,
    /// The call's id, or `answer-N` for a final answer.
    pub key: String,
    /// The tool, or `None` for a final answer.
    pub tool: Option<String>,
    /// The command or arguments, or the answer.
    pub shown: String,
    pub rule: String,
    pub score: f64,
}

/// Something the rules dealt with on their own, or a person marked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handled {
    pub what: HandledKind,
    pub shown: String,
    pub rule: String,
    pub run_title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandledKind {
    Blocked,
    Held,
    LookedFine,
}

impl Workspace {
    /// Opens the editor on a new rule, or on rule `id`.
    pub fn open_rule_editor(
        &mut self,
        repo: &str,
        id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let rule = id.and_then(|id| {
            self.repo_named(repo)
                .constitution
                .rules
                .iter()
                .find(|rule| rule.id == id)
                .cloned()
        });
        let (review, block) = Preset::Balanced.marks();
        let text = rule
            .as_ref()
            .map(|rule| rule.text.clone())
            .unwrap_or_default();
        self.rule_text
            .update(cx, |input, cx| input.set_text(text, cx));
        self.rule_on.update(cx, |input, cx| input.clear(cx));
        self.rule_menu = None;
        self.rule_draft = Some(RuleDraft {
            repo: repo.to_owned(),
            editing: rule.as_ref().map(|rule| rule.id.clone()),
            places: rule
                .as_ref()
                .map(|rule| rule.applies_to.clone())
                .unwrap_or_default(),
            review: rule
                .as_ref()
                .map_or(review, |rule| round(rule.review.into())),
            block: rule.as_ref().map_or(block, |rule| round(rule.block.into())),
            trying: Trying::Not,
            tried_to_save: false,
        });
        cx.notify();
    }

    pub fn close_rule_editor(&mut self, cx: &mut Context<Self>) {
        self.rule_draft = None;
        cx.notify();
    }

    pub fn rule_draft(&self) -> Option<&RuleDraft> {
        self.rule_draft.as_ref()
    }

    /// Picks or unpicks a place the rule applies.
    pub fn toggle_place(&mut self, place: &str, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.rule_draft else {
            return;
        };
        match draft.places.iter().position(|known| known == place) {
            Some(at) => {
                draft.places.remove(at);
            }
            None => draft.places.push(place.to_owned()),
        }
        draft.trying = Trying::Not;
        cx.notify();
    }

    /// Adds the field typed under "Another tool's field", if it names
    /// one (`tool.field`). Returns whether it did.
    pub fn add_other_place(&mut self, cx: &mut Context<Self>) -> bool {
        let typed = self.rule_on.read(cx).text().trim().to_owned();
        let Ok(target) = Target::parse(&typed) else {
            return false;
        };
        let place = target.label();
        let Some(draft) = &mut self.rule_draft else {
            return false;
        };
        if !draft.places.contains(&place) {
            draft.places.push(place);
        }
        draft.trying = Trying::Not;
        self.rule_on.update(cx, |input, cx| input.clear(cx));
        cx.notify();
        true
    }

    pub fn set_preset(&mut self, preset: Preset, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.rule_draft else {
            return;
        };
        (draft.review, draft.block) = preset.marks();
        cx.notify();
    }

    /// Moves a threshold by `delta`, keeping review at most block.
    pub fn nudge(&mut self, mark: Mark, delta: f64, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.rule_draft else {
            return;
        };
        match mark {
            Mark::Review => {
                draft.review =
                    round((draft.review + delta).clamp(0.0, draft.block));
            }
            Mark::Block => {
                draft.block =
                    round((draft.block + delta).clamp(draft.review, 1.0));
            }
        }
        cx.notify();
    }

    /// What keeps the draft from being saved, if anything.
    pub fn draft_problem(&self, cx: &gpui::App) -> Option<&'static str> {
        let draft = self.rule_draft.as_ref()?;
        if self.rule_text.read(cx).text().trim().is_empty() {
            Some(
                "Write the rule: one sentence, the way you would tell a person.",
            )
        } else if draft.places.is_empty() {
            Some("Pick at least one place, or Jev has nothing to check.")
        } else {
            None
        }
    }

    /// Adds the rule, or saves the one being edited. With something
    /// missing, the editor says what instead.
    pub fn save_rule(&mut self, cx: &mut Context<Self>) {
        if self.draft_problem(cx).is_some() {
            if let Some(draft) = &mut self.rule_draft {
                draft.tried_to_save = true;
            }
            cx.notify();
            return;
        }
        let Some(draft) = self.rule_draft.take() else {
            return;
        };
        let text = self.rule_text.read(cx).text().trim().to_owned();
        self.rule_text.update(cx, |input, cx| input.clear(cx));
        cx.emit(match draft.editing {
            Some(id) => WorkspaceEvent::UpdateRule {
                repo: draft.repo,
                id,
                text,
                on: draft.places,
                review: draft.review,
                block: draft.block,
            },
            None => WorkspaceEvent::AddRule {
                repo: draft.repo,
                text,
                on: draft.places,
                review: draft.review,
                block: draft.block,
            },
        });
        cx.notify();
    }

    /// Tries the draft on the repository's latest calls and answers the
    /// rule would read. The host asks Jev and answers with
    /// [`Self::set_rule_trial`].
    pub fn try_rule(&mut self, cx: &mut Context<Self>) {
        if self.draft_problem(cx).is_some() {
            if let Some(draft) = &mut self.rule_draft {
                draft.tried_to_save = true;
            }
            cx.notify();
            return;
        }
        let Some(draft) = &self.rule_draft else {
            return;
        };
        let text = self.rule_text.read(cx).text().trim().to_owned();
        let (calls, answers) = self.trial_samples(&draft.repo, &draft.places);
        let event = WorkspaceEvent::TryRule {
            repo: draft.repo.clone(),
            text,
            on: draft.places.clone(),
            review: draft.review,
            block: draft.block,
            calls,
            answers,
        };
        if let Some(draft) = &mut self.rule_draft {
            draft.trying = Trying::Asking;
        }
        cx.emit(event);
        cx.notify();
    }

    pub fn set_rule_trial(
        &mut self,
        result: Result<(Vec<Trial>, f64), String>,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = &mut self.rule_draft else {
            return;
        };
        draft.trying = match result {
            Ok((trials, cost)) => Trying::Done { trials, cost },
            Err(error) => Trying::Failed(error),
        };
        cx.notify();
    }

    /// The latest calls in `repo`'s runs that `places` read, and its
    /// latest final answers when `places` has them.
    fn trial_samples(
        &self,
        repo: &str,
        places: &[String],
    ) -> (Vec<(String, Value)>, Vec<String>) {
        let targets: Vec<Target> = places
            .iter()
            .filter_map(|place| Target::parse(place).ok())
            .collect();
        let tools: Vec<&str> = targets
            .iter()
            .filter_map(|target| match target {
                Target::Field { tool, .. } => Some(tool.as_str()),
                Target::FinalAnswer => None,
            })
            .collect();
        let runs = self.runs.iter().filter(|run| self.repo_of(run) == repo);
        let mut calls = Vec::new();
        let mut answers = Vec::new();
        for run in runs {
            for item in run.items.iter().rev() {
                if let Item::Tool(card) = item
                    && tools.contains(&card.tool.as_str())
                    && calls.len() < TRIAL_CALLS
                    && !calls.iter().any(|(_, args)| args == &card.args)
                {
                    calls.push((card.tool.clone(), card.args.clone()));
                }
            }
            if targets.contains(&Target::FinalAnswer)
                && answers.len() < TRIAL_ANSWERS
                && !run.status.is_live()
                && let Some(text) = run.last_text()
            {
                answers.push(text.to_owned());
            }
        }
        (calls, answers)
    }

    /// Sets what tau-constitution does with what Jev cannot answer in
    /// `repo`: refuse it, or let it through.
    pub fn set_blocks_unchecked(
        &mut self,
        repo: &str,
        blocks: bool,
        cx: &mut Context<Self>,
    ) {
        let max_holds = self.repo_named(repo).constitution.max_holds;
        self.save_constitution_settings(repo, blocks, max_holds, cx);
    }

    /// Changes how many times one run's answer may be sent back in
    /// `repo`, by `delta`, from 0 to [`MAX_HOLDS`].
    pub fn nudge_max_holds(
        &mut self,
        repo: &str,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        let constitution = &self.repo_named(repo).constitution;
        let max_holds = constitution
            .max_holds
            .saturating_add_signed(delta)
            .min(MAX_HOLDS);
        let blocks = constitution.blocks_unchecked;
        self.save_constitution_settings(repo, blocks, max_holds, cx);
    }

    fn save_constitution_settings(
        &mut self,
        repo: &str,
        blocks_unchecked: bool,
        max_holds: u32,
        cx: &mut Context<Self>,
    ) {
        if let Some(repo) =
            self.catalog.repos.iter_mut().find(|r| r.name == repo)
        {
            repo.constitution.blocks_unchecked = blocks_unchecked;
            repo.constitution.max_holds = max_holds;
        }
        cx.emit(WorkspaceEvent::ConstitutionSettings {
            repo: repo.to_owned(),
            blocks_unchecked,
            max_holds,
        });
        cx.notify();
    }

    /// Asks to remove `repo`'s unreadable rules; the banner asks to
    /// confirm. `None` takes the question back.
    pub fn ask_reset_rules(
        &mut self,
        repo: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.resetting_rules = repo.map(str::to_owned);
        cx.notify();
    }

    /// Removes `repo`'s unreadable rules, as confirmed.
    pub fn reset_rules(&mut self, repo: &str, cx: &mut Context<Self>) {
        self.resetting_rules = None;
        cx.emit(WorkspaceEvent::ResetRules {
            repo: repo.to_owned(),
        });
        cx.notify();
    }

    pub fn rules_tab(&self) -> RulesTab {
        self.rules_tab
    }

    pub fn set_rules_tab(&mut self, tab: RulesTab, cx: &mut Context<Self>) {
        self.rules_tab = tab;
        cx.notify();
    }

    /// Opens or closes the ⋯ menu of rule `id`.
    pub fn toggle_rule_menu(&mut self, id: &str, cx: &mut Context<Self>) {
        self.rule_menu = match self.rule_menu.as_deref() {
            Some(open) if open == id => None,
            _ => Some(id.to_owned()),
        };
        cx.notify();
    }

    /// What `repo`'s runs say about its constitution.
    pub fn rules_stats(&self, repo: &str) -> RulesStats {
        let mut stats = RulesStats::default();
        for run in self.runs.iter().filter(|run| self.repo_of(run) == repo) {
            let checks = &run.constitution;
            stats.runs += 1;
            stats.checked += checks.calls + checks.answers;
            stats.failed += checks.failed;
            stats.blocked += checks.blocked.len();
            stats.flagged += checks.flagged.len();
            stats.cost += checks.cost;
            if !checks.held.is_empty() {
                stats.held_runs += 1;
            }
            for (list, slot) in [
                (&checks.blocked, 0),
                (&checks.flagged, 1),
                (&checks.held, 2),
            ] {
                for rule in list {
                    let counts =
                        stats.per_rule.entry(rule.clone()).or_default();
                    match slot {
                        0 => counts.0 += 1,
                        1 => counts.1 += 1,
                        _ => counts.2 += 1,
                    }
                }
            }
        }
        stats.waiting = self.review_items(repo).len();
        stats
    }

    /// Flagged calls and answers in `repo`'s runs nobody has looked at,
    /// newest run first.
    pub fn review_items(&self, repo: &str) -> Vec<ReviewItem> {
        let mut items = Vec::new();
        for run in self.runs.iter().filter(|run| self.repo_of(run) == repo) {
            for card in run.reviews() {
                if let ToolState::Flagged { rule, score, .. } = &card.state
                    && !self
                        .dismissed
                        .contains(&(run.id.clone(), card.call_id.clone()))
                {
                    items.push(ReviewItem {
                        run: run.id.clone(),
                        run_title: run.title.clone(),
                        key: card.call_id.clone(),
                        tool: Some(card.tool.clone()),
                        shown: card.summary.clone(),
                        rule: rule.clone(),
                        score: score.parse().unwrap_or_default(),
                    });
                }
            }
            for (n, answer) in
                run.constitution.flagged_answers.iter().enumerate()
            {
                let key = format!("answer-{n}");
                if !self.dismissed.contains(&(run.id.clone(), key.clone())) {
                    items.push(ReviewItem {
                        run: run.id.clone(),
                        run_title: run.title.clone(),
                        key,
                        tool: None,
                        shown: answer.answer.clone(),
                        rule: answer.rule.clone(),
                        score: answer.score,
                    });
                }
            }
        }
        items
    }

    /// What the rules dealt with in `repo`'s runs, and what a person
    /// marked as fine.
    pub fn handled(&self, repo: &str) -> Vec<Handled> {
        let mut handled = Vec::new();
        for run in self.runs.iter().filter(|run| self.repo_of(run) == repo) {
            for card in run.reviews() {
                let (what, rule) = match &card.state {
                    ToolState::Blocked { rule, .. } => {
                        (HandledKind::Blocked, rule)
                    }
                    ToolState::Flagged { rule, .. }
                        if self.dismissed.contains(&(
                            run.id.clone(),
                            card.call_id.clone(),
                        )) =>
                    {
                        (HandledKind::LookedFine, rule)
                    }
                    _ => continue,
                };
                handled.push(Handled {
                    what,
                    shown: format!("{} {}", card.tool, card.summary),
                    rule: rule.clone(),
                    run_title: run.title.clone(),
                });
            }
            for rule in &run.constitution.held {
                handled.push(Handled {
                    what: HandledKind::Held,
                    shown: "final answer".into(),
                    rule: rule.clone(),
                    run_title: run.title.clone(),
                });
            }
            // Flagged answers a person marked fine, keyed as in
            // `review_items`.
            for (n, answer) in
                run.constitution.flagged_answers.iter().enumerate()
            {
                if self
                    .dismissed
                    .contains(&(run.id.clone(), format!("answer-{n}")))
                {
                    handled.push(Handled {
                        what: HandledKind::LookedFine,
                        shown: format!("final answer: {}", answer.answer),
                        rule: answer.rule.clone(),
                        run_title: run.title.clone(),
                    });
                }
            }
        }
        handled
    }
}

/// A threshold to two places, so steps of 0.05 do not drift.
fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
