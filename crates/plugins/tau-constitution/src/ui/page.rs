//! A repository's rules: what each reads, how strict it is and what it
//! did, the calls and answers that wait for a person, and the editor
//! that writes and tries a rule.

use std::collections::{BTreeSet, HashMap};

use gpui::{
    AnyElement,
    App,
    ClickEvent,
    Context,
    Div,
    Entity,
    HighlightStyle,
    Hsla,
    SharedString,
    StyledText,
    Window,
    div,
    prelude::*,
    px,
};
use serde_json::Value;
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, ButtonKind, Material as _, heading, icon, mono},
    format::usd,
    input::TextInput,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
};
use tau_ui_plugin::{Handle, ViewCx};

use super::{
    Act,
    ConstitutionUi,
    Flag,
    RuleInfo,
    Rules,
    TrialResult,
    stats::RulesStats,
};
use crate::{Trial, VerdictKind, rules::Target};

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

/// The most times the page lets one run's answer be sent back.
pub const MAX_HOLDS: u32 = 10;

/// The widest the page's column grows: rules read as lines of prose.
const COLUMN: f32 = 860.;

/// Thresholds by name. The editor offers these first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Review,
    Block,
}

/// What trying the rule gave.
#[derive(Debug, Clone, PartialEq, Default)]
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
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
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

impl Draft {
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|preset| {
            let (review, block) = preset.marks();
            (review - self.review).abs() < 1e-6
                && (block - self.block).abs() < 1e-6
        })
    }
}

/// Which list the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Rules,
    Review,
}

/// The page's state in one window.
pub struct Ui {
    handle: Handle,
    /// The rule being written, and a field it applies to.
    pub(super) rule_text: Entity<TextInput>,
    pub(super) rule_on: Entity<TextInput>,
    tab: Tab,
    /// The rule whose ⋯ menu is open.
    menu: Option<String>,
    /// The repository whose unreadable rules the user asked to remove,
    /// until confirmed.
    resetting: Option<String>,
    draft: Option<Draft>,
    /// What was looked at in this window, before the host says so.
    dismissed: BTreeSet<(String, String)>,
}

impl Ui {
    pub(super) fn new(
        handle: Handle,
        rule_text: Entity<TextInput>,
        rule_on: Entity<TextInput>,
    ) -> Self {
        Self {
            handle,
            rule_text,
            rule_on,
            tab: Tab::default(),
            menu: None,
            resetting: None,
            draft: None,
            dismissed: BTreeSet::new(),
        }
    }

    /// Draws the interface again.
    fn changed(&self, cx: &mut Context<Self>) {
        cx.notify();
        self.handle.refresh(cx);
    }

    /// Opens the editor on a new rule, or on `rule`.
    pub fn open_editor(
        &mut self,
        repo: &str,
        rule: Option<RuleInfo>,
        cx: &mut Context<Self>,
    ) {
        let (review, block) = Preset::Balanced.marks();
        let text = rule
            .as_ref()
            .map(|rule| rule.text.clone())
            .unwrap_or_default();
        self.rule_text
            .update(cx, |input, cx| input.set_text(text, cx));
        self.rule_on.update(cx, |input, cx| input.clear(cx));
        self.menu = None;
        self.draft = Some(Draft {
            repo: repo.to_owned(),
            editing: rule.as_ref().map(|rule| rule.id.clone()),
            places: rule
                .as_ref()
                .map(|rule| rule.applies_to.clone())
                .unwrap_or_default(),
            review: rule.as_ref().map_or(review, |rule| round(rule.review)),
            block: rule.as_ref().map_or(block, |rule| round(rule.block)),
            trying: Trying::Not,
            tried_to_save: false,
        });
        self.changed(cx);
    }

    /// Fills the rule being written, as typing would.
    pub fn set_rule_text(&self, text: &str, cx: &mut Context<Self>) {
        self.rule_text
            .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
    }

    /// Fills the field typed under the places, as typing would.
    pub fn set_rule_on(&self, text: &str, cx: &mut Context<Self>) {
        self.rule_on
            .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn close_editor(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        self.changed(cx);
    }

    pub fn draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
    }

    /// Picks or unpicks a place the rule applies.
    pub fn toggle_place(&mut self, place: &str, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        match draft.places.iter().position(|known| known == place) {
            Some(at) => {
                draft.places.remove(at);
            }
            None => draft.places.push(place.to_owned()),
        }
        draft.trying = Trying::Not;
        self.changed(cx);
    }

    /// Adds the field typed under the places, if it names one
    /// (`tool.field`). Returns whether it did.
    pub fn add_other_place(&mut self, cx: &mut Context<Self>) -> bool {
        let typed = self.rule_on.read(cx).text().trim().to_owned();
        let Ok(target) = Target::parse(&typed) else {
            return false;
        };
        let place = target.label();
        let Some(draft) = &mut self.draft else {
            return false;
        };
        if !draft.places.contains(&place) {
            draft.places.push(place);
        }
        draft.trying = Trying::Not;
        self.rule_on.update(cx, |input, cx| input.clear(cx));
        self.changed(cx);
        true
    }

    pub fn set_preset(&mut self, preset: Preset, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        (draft.review, draft.block) = preset.marks();
        self.changed(cx);
    }

    /// Moves a threshold by `delta`, keeping review at most block.
    pub fn nudge(&mut self, mark: Mark, delta: f64, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.draft else {
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
        self.changed(cx);
    }

    /// What keeps the draft from being saved, if anything.
    pub fn problem(&self, cx: &App) -> Option<&'static str> {
        let draft = self.draft.as_ref()?;
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
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.problem(cx).is_some() {
            if let Some(draft) = &mut self.draft {
                draft.tried_to_save = true;
            }
            self.changed(cx);
            return;
        }
        let Some(draft) = self.draft.take() else {
            return;
        };
        let text = self.rule_text.read(cx).text().trim().to_owned();
        self.rule_text.update(cx, |input, cx| input.clear(cx));
        self.handle.act(
            match draft.editing {
                Some(id) => Act::Update {
                    repo: draft.repo,
                    id,
                    text,
                    on: draft.places,
                    review: draft.review,
                    block: draft.block,
                },
                None => Act::Add {
                    repo: draft.repo,
                    text,
                    on: draft.places,
                    review: draft.review,
                    block: draft.block,
                },
            },
            cx,
        );
        self.changed(cx);
    }

    /// Tries the draft on `calls` and `answers` the rule would read. The
    /// host asks Jev and answers with [`Self::tried`].
    pub fn try_rule(
        &mut self,
        calls: Vec<(String, Value)>,
        answers: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.problem(cx).is_some() {
            if let Some(draft) = &mut self.draft {
                draft.tried_to_save = true;
            }
            self.changed(cx);
            return;
        }
        let text = self.rule_text.read(cx).text().trim().to_owned();
        let Some(draft) = &mut self.draft else {
            return;
        };
        draft.trying = Trying::Asking;
        let act = Act::Try {
            text,
            on: draft.places.clone(),
            review: draft.review,
            block: draft.block,
            calls,
            answers,
        };
        self.handle.act(act, cx);
        self.changed(cx);
    }

    /// What trying the rule gave.
    pub fn tried(&mut self, result: TrialResult) {
        if let Some(draft) = &mut self.draft {
            draft.trying = match result {
                Ok((trials, cost)) => Trying::Done { trials, cost },
                Err(error) => Trying::Failed(error),
            };
        }
    }

    /// Saves `repo`'s settings: what Jev cannot answer, and how many
    /// times an answer may go back.
    pub fn settings(
        &mut self,
        repo: &str,
        blocks_unchecked: bool,
        max_holds: u32,
        cx: &mut Context<Self>,
    ) {
        self.handle.act(
            Act::Settings {
                repo: repo.to_owned(),
                blocks_unchecked,
                max_holds,
            },
            cx,
        );
        self.changed(cx);
    }

    /// Asks to remove `repo`'s unreadable rules; the banner asks to
    /// confirm. `None` takes the question back.
    pub fn ask_reset(&mut self, repo: Option<&str>, cx: &mut Context<Self>) {
        self.resetting = repo.map(str::to_owned);
        self.changed(cx);
    }

    /// Removes `repo`'s unreadable rules, as confirmed.
    pub fn reset(&mut self, repo: &str, cx: &mut Context<Self>) {
        self.resetting = None;
        self.handle.act(
            Act::Reset {
                repo: repo.to_owned(),
            },
            cx,
        );
        self.changed(cx);
    }

    pub fn remove(&mut self, repo: &str, id: &str, cx: &mut Context<Self>) {
        self.menu = None;
        self.handle.act(
            Act::Remove {
                repo: repo.to_owned(),
                id: id.to_owned(),
            },
            cx,
        );
        self.changed(cx);
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.changed(cx);
    }

    /// Opens or closes the ⋯ menu of rule `id`.
    pub fn toggle_menu(&mut self, id: &str, cx: &mut Context<Self>) {
        self.menu = match self.menu.as_deref() {
            Some(open) if open == id => None,
            _ => Some(id.to_owned()),
        };
        self.changed(cx);
    }

    /// Takes a flagged call or answer off the review queue, for good.
    pub fn reviewed(&mut self, run: &str, key: &str, cx: &mut Context<Self>) {
        self.dismissed.insert((run.to_owned(), key.to_owned()));
        self.handle.act(
            Act::Reviewed {
                run: run.to_owned(),
                key: key.to_owned(),
            },
            cx,
        );
        self.changed(cx);
    }

    /// Whether `(run, key)` was looked at.
    fn looked_at(
        &self,
        reviewed: &[(String, String)],
        run: &str,
        key: &str,
    ) -> bool {
        let entry = (run.to_owned(), key.to_owned());
        self.dismissed.contains(&entry) || reviewed.contains(&entry)
    }
}

/// How many answers may go back after a step of `delta` from `holds`:
/// from none to [`MAX_HOLDS`].
pub fn next_holds(holds: u32, delta: i32) -> u32 {
    holds.saturating_add_signed(delta).min(MAX_HOLDS)
}

/// A threshold to two places, so steps of 0.05 do not drift.
fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// A click that changes the page's state.
fn on_ui(
    ui: &Entity<Ui>,
    f: impl Fn(&mut Ui, &mut Context<Ui>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let ui = ui.clone();
    move |_, _, cx| ui.update(cx, |ui, cx| f(ui, cx))
}

/// A flagged call or answer that waits for a person.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewItem {
    pub run: tau_agent::tool::RunId,
    pub run_title: String,
    pub flag: Flag,
}

/// Something the rules dealt with on their own, or a person marked.
#[derive(Debug, Clone, PartialEq)]
pub struct Handled {
    pub what: HandledKind,
    pub shown: String,
    pub rule: String,
    pub run_title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandledKind {
    Blocked,
    Held,
    LookedFine,
}

/// Flagged calls and answers in `repo`'s runs nobody has looked at.
pub fn review_items(
    view: &ViewCx<'_, ConstitutionUi>,
    repo: &str,
) -> Vec<ReviewItem> {
    let ui = view.read_ui();
    let reviewed = &view.data.reviewed;
    view.runs()
        .into_iter()
        .filter(|(run, _)| run.repo == repo)
        .flat_map(|(run, state)| {
            state
                .flags()
                .into_iter()
                .filter(|flag| !ui.looked_at(reviewed, &run.id.0, &flag.key))
                .map(|flag| ReviewItem {
                    run: run.id.clone(),
                    run_title: run.title.clone(),
                    flag,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What the rules dealt with in `repo`'s runs, and what a person marked
/// as fine.
pub fn handled(view: &ViewCx<'_, ConstitutionUi>, repo: &str) -> Vec<Handled> {
    let ui = view.read_ui();
    let reviewed = &view.data.reviewed;
    let mut handled = Vec::new();
    for (run, state) in
        view.runs().into_iter().filter(|(run, _)| run.repo == repo)
    {
        for call in state.calls.values() {
            if let Some(verdict) = &call.verdict
                && verdict.kind == VerdictKind::Blocked
            {
                handled.push(Handled {
                    what: HandledKind::Blocked,
                    shown: format!("{} {}", call.tool, call.shown),
                    rule: verdict.rule.clone(),
                    run_title: run.title.clone(),
                });
            }
        }
        for rule in &state.stats.held {
            handled.push(Handled {
                what: HandledKind::Held,
                shown: "final answer".into(),
                rule: rule.clone(),
                run_title: run.title.clone(),
            });
        }
        for flag in state.flags() {
            if ui.looked_at(reviewed, &run.id.0, &flag.key) {
                handled.push(Handled {
                    what: HandledKind::LookedFine,
                    shown: match &flag.tool {
                        Some(tool) => format!("{tool} {}", flag.shown),
                        None => format!("final answer: {}", flag.shown),
                    },
                    rule: flag.rule,
                    run_title: run.title.clone(),
                });
            }
        }
    }
    handled
}

/// What `repo`'s runs say about its constitution.
pub fn rules_stats(
    view: &ViewCx<'_, ConstitutionUi>,
    repo: &str,
) -> RulesStats {
    let runs: Vec<_> = view
        .runs()
        .into_iter()
        .filter(|(run, _)| run.repo == repo)
        .collect();
    let loaded: Vec<(&str, &super::Stats)> = runs
        .iter()
        .map(|(run, state)| (&*run.id.0, &state.stats))
        .collect();
    let history = view
        .repo(repo)
        .map(|rules| rules.history.as_slice())
        .unwrap_or_default();
    let mut stats = RulesStats::of(&loaded, history);
    stats.waiting = review_items(view, repo).len();
    stats
}

/// The latest calls in `repo`'s runs that `places` read, and its latest
/// final answers when `places` has them.
pub fn trial_samples(
    view: &ViewCx<'_, ConstitutionUi>,
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
    let mut calls: Vec<(String, Value)> = Vec::new();
    let mut answers = Vec::new();
    for (run, _) in view.runs().into_iter().filter(|(run, _)| run.repo == repo)
    {
        for card in view.cards(&run.id).into_iter().rev() {
            if tools.contains(&card.tool.as_str())
                && calls.len() < TRIAL_CALLS
                && !calls.iter().any(|(_, args)| args == &card.args)
            {
                calls.push((card.tool, card.args));
            }
        }
        if targets.contains(&Target::FinalAnswer)
            && answers.len() < TRIAL_ANSWERS
            && let Some(text) = run.answer
        {
            answers.push(text);
        }
    }
    (calls, answers)
}

/// The page: its repository is the `repo` parameter; `rule`, when
/// given, is the rule to show. The repository's own page draws its name
/// above, so this starts with what the rules do.
pub fn render(view: &mut ViewCx<'_, ConstitutionUi>) -> AnyElement {
    let t = view.theme().clone();
    let compact = view.compact;
    let Some(repo) = view.param("repo").map(str::to_owned) else {
        return ui::empty("Pick a repository to see its rules.", &t)
            .into_any_element();
    };
    let focus = view.param("rule").map(str::to_owned);
    let rules = view.repo(&repo).cloned().unwrap_or_default();
    let stats = rules_stats(view, &repo);
    let jev = view.jev;
    let ui = view.ui.clone();
    let tab = view.read_ui().tab;
    let empty = rules.rules.is_empty() && rules.error.is_none();
    let column = div()
        .w_full()
        .max_w(px(COLUMN))
        .flex()
        .flex_col()
        .gap(sp(4.))
        .when(!empty, |column| column.child(intro(&ui, &repo, &stats, &t)))
        .when_some(broken(view, &rules, &repo, jev, &t), |column, banner| {
            column.child(banner)
        })
        .when(!jev && rules.error.is_none(), |column| {
            column.child(no_key(view.handle.clone(), compact, &t))
        })
        .when(empty, |column| column.child(nothing_yet(&ui, &repo, &t)))
        .when(!empty && rules.error.is_none(), |column| {
            let list = match tab {
                Tab::Rules => rules_list(
                    view,
                    &repo,
                    &rules,
                    &stats,
                    focus.as_deref(),
                    &t,
                )
                .into_any_element(),
                Tab::Review => {
                    review(view, &repo, &rules, &t).into_any_element()
                }
            };
            column
                .child(tabs(&ui, tab, &rules, &stats, &t))
                .child(list)
                .when(tab == Tab::Rules, |column| {
                    column.child(settings(&ui, &repo, &rules, &t))
                })
        });
    let editor = editor(view, &t);
    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .child(ui::screen(
            "constitution",
            compact,
            div().flex().justify_center().child(column),
        ))
        .when_some(editor, |screen, editor| screen.child(editor))
        .into_any_element()
}

/// One line on what the rules do and what they did over `repo`'s runs,
/// with each count in its color, and the way to add a rule.
fn intro(ui: &Entity<Ui>, repo: &str, stats: &RulesStats, t: &Theme) -> Div {
    let held: usize = stats.per_rule.values().map(|(_, _, held)| held).sum();
    let mut parts: Vec<(String, Option<Hsla>)> = vec![
        ("tau-constitution".into(), Some(t.roles.rules)),
        (
            " checks every tool call and final answer against these rules \
             before they run. "
                .into(),
            None,
        ),
    ];
    if stats.runs == 0 {
        parts.push(("Nothing checked yet.".into(), None));
    } else {
        parts.push((
            format!(
                "Across {} {}: ",
                stats.runs,
                if stats.runs == 1 { "run" } else { "runs" }
            ),
            None,
        ));
        parts.push((format!("{} checked", stats.checked), Some(t.text_soft)));
        for (count, what, color) in [
            (stats.blocked, "blocked", t.red),
            (stats.flagged, "flagged", t.roles.waiting),
            (held, "held", t.roles.live),
            (stats.failed as usize, "not checked", t.roles.waiting),
        ] {
            if count > 0 {
                parts.push((", ".into(), None));
                parts.push((format!("{count} {what}"), Some(color)));
            }
        }
        parts.push((".".into(), None));
        if stats.cost > 0.0 {
            parts.push((" Jev cost ".into(), None));
            parts.push((
                if stats.cost < 0.01 {
                    format!("${:.5}", stats.cost)
                } else {
                    usd(stats.cost)
                },
                Some(t.roles.cost),
            ));
            parts.push((".".into(), None));
        }
    }
    let mut line = String::new();
    let mut highlights = Vec::new();
    for (part, color) in parts {
        let start = line.len();
        line.push_str(&part);
        if let Some(color) = color {
            highlights.push((
                start..line.len(),
                HighlightStyle {
                    color: Some(color),
                    ..HighlightStyle::default()
                },
            ));
        }
    }
    let new_repo = repo.to_owned();
    div()
        .flex()
        .items_center()
        .gap(sp(3.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .typeset(Type::BODY)
                .text_color(t.muted)
                .child(StyledText::new(line).with_highlights(highlights)),
        )
        .child(
            div()
                .id("new-rule")
                .child(ui::button("Add a rule", ButtonKind::Secondary, t))
                .on_click(on_ui(ui, move |ui, cx| {
                    ui.open_editor(&new_repo, None, cx)
                })),
        )
}

/// Rules that cannot be read from the store: why, what it means, and
/// the way out: removing them, once confirmed.
fn broken(
    view: &ViewCx<'_, ConstitutionUi>,
    rules: &Rules,
    repo: &str,
    jev: bool,
    t: &Theme,
) -> Option<Div> {
    let error = rules.error.clone()?;
    // Without a key no run checks rules, so none fails on them yet.
    let title = if jev {
        format!(
            "The rules for {repo} can't be read, so runs in {repo} fail at start"
        )
    } else {
        format!(
            "The rules for {repo} can't be read: once a TypeSafe key is \
             added, runs in {repo} fail at start until they can"
        )
    };
    let confirming = view.read_ui().resetting.as_deref() == Some(repo);
    let ui = view.ui.clone();
    let (ask, remove) = (repo.to_owned(), repo.to_owned());
    let actions = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .when(!confirming, |row| {
            row.child(
                div()
                    .id("reset-rules")
                    .child(ui::button(
                        "Remove these rules",
                        ButtonKind::Danger,
                        t,
                    ))
                    .on_click(on_ui(&ui, move |ui, cx| {
                        ui.ask_reset(Some(&ask), cx)
                    })),
            )
        })
        .when(confirming, |row| {
            row.child(ui::text(
                "This deletes them for good, so you can write them again.",
                Type::SMALL,
                t.text_soft,
            ))
            .child(
                div()
                    .id("reset-rules-confirm")
                    .child(ui::button("Delete", ButtonKind::Danger, t))
                    .on_click(on_ui(&ui, move |ui, cx| ui.reset(&remove, cx))),
            )
            .child(
                div()
                    .id("reset-rules-keep")
                    .child(ui::button("Keep", ButtonKind::Secondary, t))
                    .on_click(on_ui(&ui, |ui, cx| ui.ask_reset(None, cx))),
            )
        });
    Some(
        div()
            .flex()
            .gap(sp(3.5))
            .p(sp(4.))
            .rounded(radius::BOX)
            .bg(t.red_soft)
            .border_1()
            .border_color(t.red_border)
            .child(icon(Icon::Blocked, IconSize::LARGE, t.red))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(2.))
                    .child(div().font_weight(weight::STRONG).child(title))
                    .child(ui::text(error, Type::SMALL, t.text_soft))
                    .child(actions),
            ),
    )
}

/// How the checks behave beyond each rule: what happens when Jev cannot
/// answer, and how many times an answer may go back. Rows are split by
/// lines, as the rules are, and a note under them says what Jev reads.
fn settings(ui: &Entity<Ui>, repo: &str, rules: &Rules, t: &Theme) -> Div {
    let row = || {
        div()
            .flex()
            .items_center()
            .gap(sp(4.))
            .px(sp(1.))
            .py(sp(3.5))
            .border_b_1()
            .border_color(t.border)
    };
    let what = |name: &str, caption: String| {
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .child(div().font_weight(weight::EMPHASIS).child(name.to_owned()))
            .child(ui::text(caption, Type::SMALL, t.muted))
    };
    let (blocks, holds) = (rules.blocks_unchecked, rules.max_holds);
    let segment = |label: &'static str, on: bool, blocks: bool| {
        let repo = repo.to_owned();
        div()
            .id(SharedString::from(format!("on-error-{label}")))
            .h(px(28.))
            .px(sp(3.))
            .flex()
            .items_center()
            .rounded(radius::CONTROL)
            .typeset(Type::CAPTION)
            .cursor_pointer()
            .text_color(if on { t.text } else { t.muted })
            .when(on, |segment| segment.key(t))
            .child(label)
            .on_click(on_ui(ui, move |ui, cx| {
                ui.settings(&repo, blocks, holds, cx)
            }))
    };
    let step = |label: &'static str, delta: i32, id: &'static str| {
        let repo = repo.to_owned();
        let max_holds = next_holds(holds, delta);
        div()
            .id(id)
            .child(ui::button(label, ButtonKind::Secondary, t))
            .on_click(on_ui(ui, move |ui, cx| {
                ui.settings(&repo, blocks, max_holds, cx)
            }))
    };
    div()
        .flex()
        .flex_col()
        .mt(sp(4.))
        .child(div().pb(sp(1.)).child(heading("Settings", t)))
        .child(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(t.border)
                .child(
                    row()
                        .child(what(
                            "When Jev can't answer",
                            if blocks {
                                "Calls it could not check are refused, and \
                                 answers sent back while holds are left."
                                    .into()
                            } else {
                                "Calls it could not check run, and answers \
                                 stand; each is noted."
                                    .into()
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(sp(0.5))
                                .p(sp(0.5))
                                .rounded(radius::BOX)
                                .well(t)
                                .child(segment("Let through", !blocks, false))
                                .child(segment("Refuse", blocks, true)),
                        ),
                )
                .child(
                    row()
                        .child(what(
                            "Answers sent back per run",
                            match holds {
                                0 => "A final answer that breaks a rule \
                                      stands, flagged."
                                    .into(),
                                n => format!(
                                    "Up to {n}; past that, an answer that \
                                     breaks a rule stands, flagged."
                                ),
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(2.))
                                .child(step("−", -1, "max-holds-less"))
                                .child(mono(
                                    holds.to_string(),
                                    Type::BODY,
                                    t.text,
                                ))
                                .child(step("+", 1, "max-holds-more")),
                        ),
                ),
        )
        .child(div().pt(sp(3.)).child(ui::text(
            "Jev sees only the fields a rule names, as the model wrote them. \
             Never tool output or file contents.",
            Type::CAPTION,
            t.dim,
        )))
}

/// Without a TypeSafe key nothing is checked: say so, and where to fix it.
fn no_key(handle: Handle, compact: bool, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(3.5))
        .p(sp(3.5))
        .rounded(radius::BOX)
        .bg(t.accent_soft)
        .border_1()
        .border_color(t.accent_border)
        .child(icon(Icon::Key, IconSize::LARGE, t.accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.75))
                .child(div().font_weight(weight::EMPHASIS).child("Rules aren't checked yet"))
                .when(!compact, |text| {
                    text.child(ui::text(
                        "tau-constitution asks Jev, TypeSafe's scoring model. Add \
                         a TypeSafe key and every run checks these rules.",
                        Type::SMALL,
                        t.muted,
                    ))
                }),
        )
        .child(
            div()
                .id("add-jev-key")
                .child(ui::button("Add TypeSafe key", ButtonKind::Primary, t))
                .on_click(move |_, _, cx| handle.ask_jev_key(cx)),
        )
}

/// No rules: what a rule is, and how to write one.
fn nothing_yet(ui: &Entity<Ui>, repo: &str, t: &Theme) -> Div {
    let new_repo = repo.to_owned();
    div().flex().justify_center().py(sp(12.)).child(
        div()
            .w(px(520.))
            .max_w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(sp(3.5))
            .child(
                div()
                    .size(px(48.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::BOX)
                    .key(t)
                    .child(icon(Icon::Blocked, IconSize::LARGE, t.muted)),
            )
            .child(
                div()
                    .typeset(Type::LEAD)
                    .font_weight(weight::STRONG)
                    .child(format!("No rules for {repo}")),
            )
            .child(div().text_center().child(ui::text(
                "A rule is one sentence, checked where you say: a shell \
                 command, an edit, a written file, or the final answer. Past \
                 its block mark the call is refused and the model gets the \
                 rule back.",
                Type::BODY,
                t.muted,
            )))
            .child(
                div().flex().gap(sp(2.)).mt(sp(1.5)).child(
                    div()
                        .id("first-rule")
                        .child(ui::button("New rule", ButtonKind::Primary, t))
                        .on_click(on_ui(ui, move |ui, cx| {
                            ui.open_editor(&new_repo, None, cx)
                        })),
                ),
            ),
    )
}

/// The two lists: the rules, and what waits for review. A count of
/// what waits stands out until it is looked at.
fn tabs(
    ui: &Entity<Ui>,
    current: Tab,
    rules: &Rules,
    stats: &RulesStats,
    t: &Theme,
) -> Div {
    let tab = |which: Tab, name: &'static str, count: usize, badge: bool| {
        let on = current == which;
        div()
            .id(name)
            .flex()
            .items_center()
            .gap(sp(1.5))
            .typeset(Type::SMALL)
            .text_color(if on { t.text } else { t.muted })
            .when(on, |tab| tab.font_weight(weight::EMPHASIS))
            .hover(|style| style.text_color(t.text))
            .cursor_pointer()
            .child(name)
            .map(|tab| {
                if badge && count > 0 {
                    tab.child(ui::count_pill(count, t))
                } else {
                    tab.child(mono(count.to_string(), Type::CAPTION, t.dim))
                }
            })
            .on_click(on_ui(ui, move |ui, cx| ui.set_tab(which, cx)))
    };
    div()
        .flex()
        .items_center()
        .gap(sp(5.))
        .child(tab(Tab::Rules, "Rules", rules.rules.len(), false))
        .child(tab(Tab::Review, "Review", stats.waiting, true))
}

/// A rule's verdict over its runs, in a few words, and its color:
/// blocked red, held live, flagged waiting, nothing caught green.
fn verdict(rule: &RuleInfo, stats: &RulesStats, t: &Theme) -> (String, Hsla) {
    let (blocked, flagged, held) =
        stats.per_rule.get(&rule.id).copied().unwrap_or_default();
    let parts: Vec<String> =
        [(blocked, "blocked"), (held, "held"), (flagged, "flagged")]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect();
    let color = if blocked > 0 {
        t.red
    } else if held > 0 {
        t.roles.live
    } else if flagged > 0 {
        t.roles.waiting
    } else {
        t.green
    };
    if parts.is_empty() {
        ("clear".into(), color)
    } else {
        (parts.join(" · "), color)
    }
}

/// What each rule last caught in `repo`'s runs: the call it blocked or
/// flagged, or the final answer it held.
fn last_caught(
    view: &ViewCx<'_, ConstitutionUi>,
    repo: &str,
) -> HashMap<String, String> {
    let mut last = HashMap::new();
    for (run, state) in
        view.runs().into_iter().filter(|(run, _)| run.repo == repo)
    {
        for call in state.calls.values() {
            if let Some(verdict) = &call.verdict {
                last.insert(verdict.rule.clone(), call.shown.clone());
            }
        }
        for rule in &state.stats.held {
            last.insert(rule.clone(), format!("final answer, {}", run.title));
        }
    }
    last
}

/// The rules, one line each: the id, the rule with where it reads and
/// how strict it is, what it caught, and its ⋯ menu. A click edits it.
fn rules_list(
    view: &ViewCx<'_, ConstitutionUi>,
    repo: &str,
    rules: &Rules,
    stats: &RulesStats,
    focus: Option<&str>,
    t: &Theme,
) -> Div {
    let compact = view.compact;
    let menu = view.read_ui().menu.clone();
    let last = last_caught(view, repo);
    let ui = view.ui.clone();
    let rows = rules.rules.iter().map(|rule| {
        let (said, said_color) = verdict(rule, stats, t);
        let focused = focus == Some(rule.id.as_str());
        let menu_open = menu.as_deref() == Some(rule.id.as_str());
        let (edit_repo, edit_rule) = (repo.to_owned(), rule.clone());
        let menu_id = rule.id.clone();
        let mut caption = format!(
            "Checks {} · flag {:.2} · block {:.2}",
            rule.applies_to.join(", "),
            rule.review,
            rule.block
        );
        if let Some(shown) = last.get(&rule.id) {
            caption.push_str(&format!(" · last: {shown}"));
        }
        let toggle = ui.clone();
        let menu_button = div()
            .id(SharedString::from(format!("rule-menu-{}", rule.id)))
            .size(px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::CONTROL)
            .when(menu_open, |button| button.bg(t.border_strong))
            .hover(|style| style.bg(t.border_strong))
            .child(mono("⋯", Type::BODY, t.muted))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                toggle.update(cx, |ui, cx| ui.toggle_menu(&menu_id, cx))
            });
        div()
            .id(SharedString::from(format!("rule-{}", rule.id)))
            .relative()
            .flex()
            .items_start()
            .gap(sp(3.5))
            .px(sp(1.))
            .py(sp(3.5))
            .border_b_1()
            .border_color(t.border)
            .cursor_pointer()
            .when(focused || menu_open, |row| row.pressed(t))
            .hover(|style| style.bg(t.selected))
            .on_click(on_ui(&ui, move |ui, cx| {
                ui.open_editor(&edit_repo, Some(edit_rule.clone()), cx)
            }))
            .child(
                mono(rule.id.clone(), Type::CODE, t.roles.rules)
                    .w(px(40.))
                    .flex_shrink_0()
                    .pt(sp(0.25)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.5))
                    .child(
                        div()
                            .font_weight(weight::EMPHASIS)
                            .child(rule.text.clone()),
                    )
                    .child(ui::text(caption, Type::SMALL, t.muted))
                    .when(compact, |text| {
                        text.child(ui::text(
                            said.clone(),
                            Type::SMALL,
                            said_color,
                        ))
                    }),
            )
            .when(!compact, |row| {
                row.child(
                    div()
                        .w(px(150.))
                        .flex_shrink_0()
                        .flex()
                        .justify_end()
                        .child(ui::text(said, Type::SMALL, said_color)),
                )
            })
            .child(menu_button)
            .when(menu_open, |row| row.child(rule_menu(&ui, repo, rule, t)))
    });
    div().flex().flex_col().children(rows)
}

/// A rule's ⋯ menu: edit it, or remove it.
fn rule_menu(
    ui: &Entity<Ui>,
    repo: &str,
    rule: &RuleInfo,
    t: &Theme,
) -> impl IntoElement {
    let (edit_repo, edit_rule) = (repo.to_owned(), rule.clone());
    let (remove_repo, remove_id) = (repo.to_owned(), rule.id.clone());
    let entry = |name: &'static str, color: Hsla| {
        div()
            .id(name)
            .px(sp(3.))
            .py(sp(2.))
            .rounded(radius::CONTROL)
            .text_color(color)
            .cursor_pointer()
            .hover(|style| style.bg(t.selected))
            .child(name)
    };
    let (edit, remove) = (ui.clone(), ui.clone());
    div()
        .absolute()
        .right(sp(4.))
        .top(px(44.))
        .w(px(160.))
        .p(sp(1.))
        .flex()
        .flex_col()
        .raised(t)
        .border_1()
        .border_color(t.border_strong)
        .rounded(radius::BOX)
        .child(entry("Edit", t.text).on_click(move |_, _, cx| {
            cx.stop_propagation();
            edit.update(cx, |ui, cx| {
                ui.open_editor(&edit_repo, Some(edit_rule.clone()), cx)
            })
        }))
        .child(entry("Remove", t.red).on_click(move |_, _, cx| {
            cx.stop_propagation();
            remove.update(cx, |ui, cx| ui.remove(&remove_repo, &remove_id, cx))
        }))
}

/// What waits for a person, and what the rules handled on their own.
fn review(
    view: &ViewCx<'_, ConstitutionUi>,
    repo: &str,
    rules: &Rules,
    t: &Theme,
) -> Div {
    let compact = view.compact;
    let items = review_items(view, repo);
    let cards: Vec<AnyElement> = items
        .iter()
        .enumerate()
        .map(|(n, item)| {
            review_card(view, n, item, rules.rule(&item.flag.rule), repo, t)
                .into_any_element()
        })
        .collect();
    let waiting = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(sp(3.5))
        .child(heading(&format!("Waiting for you · {}", items.len()), t))
        .when(items.is_empty(), |list| {
            list.child(ui::empty(
                "Nothing waits for you. Flagged calls land here.",
                t,
            ))
        })
        .children(cards);
    let handled = handled(view, repo);
    let side = div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .when(!compact, |side| side.w(px(360.)).flex_shrink_0())
        .child(heading("Handled by the rules", t))
        .child(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(t.border)
                .when(handled.is_empty(), |list| {
                    list.child(div().py(sp(3.)).child(ui::text(
                        "Nothing yet.",
                        Type::CAPTION,
                        t.dim,
                    )))
                })
                .children(handled.into_iter().map(|done| {
                    let (glyph, color, what) = match done.what {
                        HandledKind::Blocked => {
                            (Icon::Blocked, t.red, "blocked")
                        }
                        HandledKind::Held => (Icon::Chat, t.roles.live, "held"),
                        HandledKind::LookedFine => {
                            (Icon::Check, t.green, "looked fine")
                        }
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.5))
                        .px(sp(3.5))
                        .py(sp(2.5))
                        .border_b_1()
                        .border_color(t.border)
                        .child(icon(glyph, IconSize::SMALL, color))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(sp(0.5))
                                .child(
                                    mono(
                                        done.shown,
                                        Type::CAPTION,
                                        t.text_soft,
                                    )
                                    .truncate(),
                                )
                                .child(ui::text(
                                    done.run_title,
                                    Type::MICRO,
                                    t.dim,
                                )),
                        )
                        .child(mono(
                            format!("{} · {what}", done.rule),
                            Type::MICRO,
                            t.dim,
                        ))
                })),
        )
        .child(ui::text(
            "Blocked calls need nothing from you: the model got the rule and \
             changed the call. They show that a rule does its job.",
            Type::CAPTION,
            t.dim,
        ));
    div()
        .flex()
        .when(compact, |layout| layout.flex_col())
        .gap(sp(6.))
        .items_start()
        .child(waiting)
        .child(side)
}

fn review_card(
    view: &ViewCx<'_, ConstitutionUi>,
    n: usize,
    item: &ReviewItem,
    rule: Option<&RuleInfo>,
    repo: &str,
    t: &Theme,
) -> impl IntoElement {
    let compact = view.compact;
    let ui = view.ui.clone();
    let handle = view.handle.clone();
    let open = item.run.clone();
    let (fine_run, fine_key) = (item.run.0.to_string(), item.flag.key.clone());
    let (adjust_repo, adjust_rule) = (repo.to_owned(), rule.cloned());
    let answer = item.flag.tool.is_none();
    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .p(sp(4.))
        .rounded(radius::BOX)
        .border_1()
        .border_color(t.accent_border)
        .raised(t)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(icon(if answer { Icon::Chat } else { Icon::Runs }, IconSize::BASE, t.muted))
                .child(mono(
                    item.flag.tool.clone().unwrap_or_else(|| "final answer".into()),
                    Type::CAPTION,
                    t.blue,
                ))
                .child(ui::text(format!("in {}", item.run_title), Type::CAPTION, t.muted))
                .child(div().flex_1())
                .child(mono(format!("p {:.2}", item.flag.score), Type::CAPTION, t.roles.waiting)),
        )
        .child(
            div()
                .px(sp(3.5))
                .py(sp(2.5))
                .rounded(radius::CONTROL)
                .well(t)
                .map(|body| {
                    if answer {
                        body.child(ui::text(item.flag.shown.clone(), Type::BODY, t.text_soft))
                    } else {
                        body.child(mono(item.flag.shown.clone(), Type::SMALL, t.text_soft))
                    }
                }),
        )
        .when_some(rule, |card, rule| {
            card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.5))
                    .px(sp(3.))
                    .py(sp(2.5))
                    .rounded(radius::CONTROL)
                    .border_1()
                    .border_color(t.border)
                    .child(mono(rule.id.clone(), Type::CAPTION, t.muted))
                    .child(div().flex_1().min_w(px(0.)).text_color(t.text_soft).child(rule.text.clone()))
                    .when(!compact, |row| {
                        row.child(ui::strictness(
                            rule.review,
                            rule.block,
                            Some(item.flag.score),
                            Some(160.),
                            false,
                            t,
                        ))
                    }),
            )
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .when(!compact, |row| {
                    row.child(div().flex_1().child(ui::text(
                        if answer {
                            "The answer stood, but its score is between this rule's flag and block marks."
                        } else {
                            "It ran: its score is between this rule's flag and block marks."
                        },
                        Type::CAPTION,
                        t.dim,
                    )))
                })
                .child(
                    div()
                        .id(("review-adjust", n))
                        .child(ui::button("Adjust rule", ButtonKind::Secondary, t))
                        .on_click(on_ui(&ui, move |ui, cx| {
                            ui.open_editor(&adjust_repo, adjust_rule.clone(), cx)
                        })),
                )
                .child(
                    div()
                        .id(("review-open", n))
                        .child(ui::button("Open in run", ButtonKind::Secondary, t))
                        .on_click(move |_, _, cx| handle.open_run(&open, cx)),
                )
                .child(
                    div()
                        .id(("review-dismiss", n))
                        .child(ui::button("Looks fine", ButtonKind::Primary, t))
                        .on_click(on_ui(&ui, move |ui, cx| ui.reviewed(&fine_run, &fine_key, cx))),
                ),
        )
}

/// The rule editor: a drawer on the right, the whole screen on a phone.
fn editor(view: &ViewCx<'_, ConstitutionUi>, t: &Theme) -> Option<AnyElement> {
    let compact = view.compact;
    let ui_state = view.read_ui();
    let draft = ui_state.draft()?.clone();
    let problem = ui_state.problem(view.cx);
    let (rule_text, rule_on) =
        (ui_state.rule_text.clone(), ui_state.rule_on.clone());
    let ui = view.ui.clone();
    let shown_problem = problem.filter(|_| draft.tried_to_save);
    let title = match &draft.editing {
        Some(id) => format!("Edit {id}"),
        None => "New rule".to_owned(),
    };
    let save_label = if draft.editing.is_some() {
        "Save"
    } else {
        "Add rule"
    };

    // Where it applies: the usual places, then any other field picked.
    let mut offered: Vec<(String, &str)> = PLACES
        .iter()
        .map(|(place, what)| ((*place).to_owned(), *what))
        .collect();
    for place in &draft.places {
        if !offered.iter().any(|(known, _)| known == place) {
            offered.push((place.clone(), "another tool's field"));
        }
    }
    let places = offered.into_iter().map(|(place, what)| {
        let on = draft.places.contains(&place);
        let toggle = place.clone();
        let label = match place.split_once('.') {
            Some((tool, field)) => div()
                .flex()
                .gap(sp(1.5))
                .child(mono(tool.to_owned(), Type::SMALL, t.blue))
                .child(mono("·", Type::SMALL, t.dim))
                .child(mono(field.to_owned(), Type::SMALL, t.text_soft)),
            None => div().child("Final answer"),
        };
        div()
            .id(SharedString::from(format!("place-{place}")))
            .flex()
            .items_center()
            .gap(sp(2.5))
            .min_h(px(if compact { 44. } else { 34. }))
            .px(sp(3.))
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .when(on, |row| row.bg(t.accent_soft))
            .hover(|style| style.bg(t.selected))
            .child(ui::checkbox(on, false, t))
            .child(div().flex_1().child(label))
            .child(ui::text(what, Type::CAPTION, t.dim))
            .on_click(on_ui(&ui, move |ui, cx| ui.toggle_place(&toggle, cx)))
    });
    let applies = div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(label_row("Applies to", "what Jev reads for this rule", t))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .p(sp(1.5))
                .rounded(radius::BOX)
                .border_1()
                .border_color(
                    if shown_problem.is_some() && draft.places.is_empty() {
                        t.red_border
                    } else {
                        t.border
                    },
                )
                .well(t)
                .children(places)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .px(sp(3.))
                        .pt(sp(1.5))
                        .pb(sp(1.))
                        .child(
                            div().flex_1().child(ui::field(&rule_on, true, t)),
                        )
                        .child(
                            div()
                                .id("add-place")
                                .child(ui::button(
                                    "Add field",
                                    ButtonKind::Secondary,
                                    t,
                                ))
                                .on_click(on_ui(&ui, |ui, cx| {
                                    ui.add_other_place(cx);
                                })),
                        ),
                ),
        );

    let presets = Preset::ALL.into_iter().map(|preset| {
        let on = draft.preset() == Some(preset);
        div()
            .id(preset.label())
            .flex_1()
            .h(px(if compact { 40. } else { 28. }))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .typeset(Type::SMALL)
            .text_color(if on { t.text } else { t.muted })
            .when(on, |segment| segment.key(t).font_weight(weight::EMPHASIS))
            .child(preset.label())
            .on_click(on_ui(&ui, move |ui, cx| ui.set_preset(preset, cx)))
    });
    let stepper = |name: &'static str, mark: Mark, value: f64, color: Hsla| {
        let step = |id: &'static str, glyph: &'static str, delta: f64| {
            div()
                .id(id)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::CONTROL)
                .border_1()
                .border_color(t.border_strong)
                .cursor_pointer()
                .hover(|style| style.bg(t.selected))
                .child(mono(glyph, Type::SMALL, t.text_soft))
                .on_click(on_ui(&ui, move |ui, cx| ui.nudge(mark, delta, cx)))
        };
        let (down, up) = match mark {
            Mark::Review => ("review-down", "review-up"),
            Mark::Block => ("block-down", "block-up"),
        };
        div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .child(ui::text(name, Type::CAPTION, color))
            .child(step(down, "−", -0.05))
            .child(mono(format!("{value:.2}"), Type::SMALL, t.text).w(px(36.)))
            .child(step(up, "+", 0.05))
    };
    let zone = |head: String, color: Hsla, body: &'static str| {
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(sp(0.75))
            .child(ui::text(head, Type::CAPTION, color))
            .child(ui::text(body, Type::CAPTION, t.dim))
    };
    let strictness = div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(label_row("Strictness", "how sure Jev must be that the rule is broken", t))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(3.5))
                .p(sp(4.))
                .rounded(radius::BOX)
                .border_1()
                .border_color(t.border)
                .bg(t.bg)
                .child(
                    div()
                        .flex()
                        .gap(sp(1.5))
                        .p(sp(0.75))
                        .rounded(radius::BOX)
                        .well(t)
                        .children(presets),
                )
                .child(div().px(sp(2.)).py(sp(2.)).child(ui::strictness(
                    draft.review,
                    draft.block,
                    None,
                    None,
                    true,
                    t,
                )))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(sp(5.))
                        .child(stepper("Flag at", Mark::Review, draft.review, t.accent))
                        .child(stepper("Block at", Mark::Block, draft.block, t.red)),
                )
                .when(!compact, |panel| {
                    panel.child(
                        div()
                            .flex()
                            .gap(sp(3.))
                            .child(zone(format!("Below {:.2}", draft.review), t.text_soft, "The call runs."))
                            .child(zone(
                                format!("{:.2} to {:.2}", draft.review, draft.block),
                                t.accent,
                                "It runs, and waits in Review for you.",
                            ))
                            .child(zone(
                                format!("{:.2} and up", draft.block),
                                t.red,
                                "Refused. The model gets the rule and tries again.",
                            )),
                    )
                }),
        );

    let trial = try_section(view, &draft, t);
    let body = div()
        .id("rule-editor-body")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .p(sp(5.))
        .flex()
        .flex_col()
        .gap(sp(5.5))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(label_row(
                    "Rule",
                    "one sentence, the way you would tell a person",
                    t,
                ))
                .child(ui::field(&rule_text, false, t)),
        )
        .child(applies)
        .when_some(shown_problem, |body, problem| {
            body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(icon(Icon::Warning, IconSize::SMALL, t.red))
                    .child(ui::text(problem, Type::SMALL, t.red)),
            )
        })
        .child(strictness)
        .child(trial);

    let save = div()
        .id("save-rule")
        .child(ui::button(save_label, ButtonKind::Primary, t))
        .when(problem.is_some(), |button| button.opacity(0.5))
        .on_click(on_ui(&ui, |ui, cx| ui.save(cx)));
    let cancel = div()
        .id("cancel-rule")
        .child(ui::button("Cancel", ButtonKind::Secondary, t))
        .on_click(on_ui(&ui, |ui, cx| ui.close_editor(cx)));
    let panel = div()
        .flex()
        .flex_col()
        .bg(t.panel)
        .child(
            div()
                .flex_shrink_0()
                .h(px(56.))
                .flex()
                .items_center()
                .gap(sp(2.5))
                .px(sp(5.))
                .border_b_1()
                .border_color(t.border)
                .child(div().flex_1().typeset(Type::LEAD).font_weight(weight::STRONG).child(title))
                .when(compact, |bar| bar.child(phone_save(&ui, save_label, problem, t))),
        )
        .child(body)
        .when(!compact, |panel| {
            panel.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(sp(2.5))
                    .px(sp(5.))
                    .py(sp(3.5))
                    .border_t_1()
                    .border_color(t.border)
                    .child(div().flex_1().child(ui::text(
                        match (shown_problem, &draft.editing) {
                            (Some(problem), _) => problem.to_owned(),
                            (None, Some(id)) => format!(
                                "Saves over {id}; runs check with it from their next tool call"
                            ),
                            (None, None) => {
                                "Adds it; runs check with it from their next tool call".to_owned()
                            }
                        },
                        Type::CAPTION,
                        if shown_problem.is_some() { t.red } else { t.dim },
                    )))
                    .child(cancel)
                    .child(save),
            )
        });
    Some(if compact {
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .child(panel.flex_1())
            .into_any_element()
    } else {
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .child(
                div()
                    .id("rule-scrim")
                    .flex_1()
                    .bg(t.scrim)
                    .on_click(on_ui(&ui, |ui, cx| ui.close_editor(cx))),
            )
            .child(
                panel
                    .w(px(560.))
                    .h_full()
                    .border_l_1()
                    .border_color(t.border_strong)
                    .shadow_lg(),
            )
            .into_any_element()
    })
}

/// On a phone, the editor's bar carries Cancel and Add.
fn phone_save(
    ui: &Entity<Ui>,
    label: &'static str,
    problem: Option<&str>,
    t: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .gap(sp(2.))
        .child(
            div()
                .id("phone-cancel-rule")
                .child(ui::button("Cancel", ButtonKind::Secondary, t))
                .on_click(on_ui(ui, |ui, cx| ui.close_editor(cx))),
        )
        .child(
            div()
                .id("phone-save-rule")
                .child(ui::button(label, ButtonKind::Primary, t))
                .when(problem.is_some(), |button| button.opacity(0.5))
                .on_click(on_ui(ui, |ui, cx| ui.save(cx))),
        )
}

/// A field's name, and what it is for.
fn label_row(name: &'static str, hint: &'static str, t: &Theme) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(sp(2.5))
        .child(div().font_weight(weight::EMPHASIS).child(name))
        .child(ui::text(hint, Type::CAPTION, t.dim))
}

/// Trying the rule on the repository's latest calls, with Jev.
fn try_section(
    view: &ViewCx<'_, ConstitutionUi>,
    draft: &Draft,
    t: &Theme,
) -> Div {
    let jev = view.jev;
    let (calls, answers) = trial_samples(view, &draft.repo, &draft.places);
    let button = div()
        .id("try-rule")
        .child(ui::button(
            match draft.trying {
                Trying::Not => "Try on recent calls",
                _ => "Try again",
            },
            ButtonKind::Secondary,
            t,
        ))
        .when(!jev, |button| button.opacity(0.5))
        .on_click(on_ui(&view.ui, move |ui, cx| {
            ui.try_rule(calls.clone(), answers.clone(), cx)
        }));
    let body: AnyElement = match &draft.trying {
        Trying::Not => ui::text(
            if jev {
                "Jev scores the latest calls and answers this rule would read, \
                 as a check would. Nothing runs again."
            } else {
                "Trying a rule asks Jev: add a TypeSafe key first."
            },
            Type::CAPTION,
            t.dim,
        )
        .into_any_element(),
        Trying::Asking => div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .child(icon(Icon::Spinner, IconSize::SMALL, t.accent))
            .child(ui::text("Asking Jev…", Type::CAPTION, t.muted))
            .into_any_element(),
        Trying::Failed(error) => {
            ui::text(error.clone(), Type::CAPTION, t.red).into_any_element()
        }
        Trying::Done { trials, .. } if trials.is_empty() => ui::text(
            "No calls in this repository's runs that the rule reads yet.",
            Type::CAPTION,
            t.dim,
        )
        .into_any_element(),
        Trying::Done { trials, cost } => {
            let rows = trials.iter().map(|trial| {
                let (color, verdict) = if trial.score >= draft.block {
                    (t.red, "would block")
                } else if trial.score >= draft.review {
                    (t.accent, "would flag")
                } else {
                    (t.muted, "would run")
                };
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .px(sp(3.))
                    .py(sp(2.25))
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .gap(sp(2.))
                            .child(mono(
                                trial
                                    .tool
                                    .clone()
                                    .unwrap_or_else(|| "answer".into()),
                                Type::CAPTION,
                                t.blue,
                            ))
                            .child(
                                mono(
                                    trial.shown.clone(),
                                    Type::CAPTION,
                                    t.text_soft,
                                )
                                .truncate(),
                            ),
                    )
                    .child(ui::strictness(
                        draft.review,
                        draft.block,
                        Some(trial.score),
                        Some(90.),
                        false,
                        t,
                    ))
                    .child(
                        mono(
                            format!("{:.2} {verdict}", trial.score),
                            Type::CAPTION,
                            color,
                        )
                        .w(px(120.))
                        .flex_shrink_0(),
                    )
            });
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .rounded(radius::BOX)
                        .border_1()
                        .border_color(t.border)
                        .raised(t)
                        .children(rows),
                )
                .child(ui::text(
                    format!(
                        "{} {} · {}. Only what a check shows went to Jev; nothing ran again.",
                        trials.len(),
                        if trials.len() == 1 { "call" } else { "calls" },
                        usd(*cost)
                    ),
                    Type::CAPTION,
                    t.dim,
                ))
                .into_any_element()
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(div().flex_1().child(label_row(
                    "Try it",
                    "before it runs for real",
                    t,
                )))
                .child(button),
        )
        .child(body)
}
