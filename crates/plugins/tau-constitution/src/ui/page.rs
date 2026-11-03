//! A repository's rules: what each reads, how strict it is and what it
//! did, the calls and answers that wait for a person, and the editor
//! that writes and tries a rule. A view tree (ADR 0045), in the frame
//! the interface gives a page: the rule being written, the tab shown and
//! a reset waiting to be confirmed are the interface's UI state.

use std::collections::HashMap;

use gpui::{AnyElement, App, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_ui_kit::format::usd;
use tau_ui_plugin::{
    Handle,
    Request,
    ViewCx,
    tree::{self, KeyValue, Kind, Node, Tone},
};

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
    ("exec_command.cmd", "the shell command"),
    ("edit.newText", "each replacement"),
    ("write.content", "the whole file"),
    ("final answer", "sent back when broken"),
];

/// How many past calls, and answers, a rule is tried on.
const TRIAL_CALLS: usize = 6;
const TRIAL_ANSWERS: usize = 3;

/// The most times the page lets one run's answer be sent back.
pub const MAX_HOLDS: u32 = 10;

/// The UI state the page keeps: the rule being written ([`Draft`]),
/// what trying it gave ([`Trying`]), the repository whose unreadable
/// rules wait to be removed, and the flagged calls looked at in this
/// window, before the host says so.
pub const EDITOR: &str = "editor";
pub const TRYING: &str = "trying";
pub const RESETTING: &str = "resetting";
pub const DISMISSED: &str = "dismissed";

/// The inputs' ids: the rule's text, and a field it applies to. Their
/// drafts are the UI state under [`tree::draft_key`].
pub const RULE_TEXT: &str = "rule-text";
pub const RULE_ON: &str = "rule-on";

/// The tabs' id: the rules, then what waits for review. The tab chosen
/// is the UI state under [`tree::tab_key`].
pub const TABS: &str = "rules-tabs";

/// Every button's and switch's action: carry out the [`Do`] its
/// arguments hold.
const DO: &str = "do";
/// Enter in the rule's field saves it; Enter in the field's field adds
/// the field.
const SAVE_TYPED: &str = "save-typed";
const ADD_TYPED: &str = "add-typed";

/// Thresholds by name. The editor offers these first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
#[serde(rename_all = "snake_case")]
pub enum Mark {
    Review,
    Block,
}

/// What trying the rule gave.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl From<TrialResult> for Trying {
    fn from(result: TrialResult) -> Self {
        match result {
            Ok((trials, cost)) => Self::Done { trials, cost },
            Err(error) => Self::Failed(error),
        }
    }
}

/// A rule being written (or rewritten), in the editor. Its text is the
/// [`RULE_TEXT`] input's draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub repo: String,
    /// The rule being edited, or `None` for a new one.
    pub editing: Option<String>,
    /// Where it applies: `tool.field` or `final answer`.
    pub places: Vec<String>,
    pub review: f64,
    pub block: f64,
    /// Whether Add was pressed with something missing: the editor then
    /// says what.
    #[serde(default)]
    pub tried_to_save: bool,
    /// The rule as Enter took it from its field, which Enter empties,
    /// kept while it could not be saved.
    #[serde(default)]
    pub kept: Option<String>,
    /// A field typed under the places that names no `tool.field`.
    #[serde(default)]
    pub bad_place: Option<String>,
}

impl Draft {
    /// A new rule for `repo`, or `rule` as it is.
    pub fn new(repo: &str, rule: Option<&RuleInfo>) -> Self {
        let (review, block) = Preset::Balanced.marks();
        Self {
            repo: repo.to_owned(),
            editing: rule.map(|rule| rule.id.clone()),
            places: rule
                .map(|rule| rule.applies_to.clone())
                .unwrap_or_default(),
            review: rule.map_or(review, |rule| round(rule.review)),
            block: rule.map_or(block, |rule| round(rule.block)),
            tried_to_save: false,
            kept: None,
            bad_place: None,
        }
    }

    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|preset| {
            let (review, block) = preset.marks();
            (review - self.review).abs() < 1e-6
                && (block - self.block).abs() < 1e-6
        })
    }

    /// Picks or unpicks a place the rule applies.
    pub fn toggle_place(&mut self, place: &str) {
        match self.places.iter().position(|known| known == place) {
            Some(at) => {
                self.places.remove(at);
            }
            None => self.places.push(place.to_owned()),
        }
    }

    /// Adds `typed`, if it names a field (`tool.field`). Returns whether
    /// it did.
    pub fn add_place(&mut self, typed: &str) -> bool {
        let Ok(target) = Target::parse(typed.trim()) else {
            self.bad_place = Some(typed.trim().to_owned());
            return false;
        };
        self.bad_place = None;
        let place = target.label();
        if !self.places.contains(&place) {
            self.places.push(place);
        }
        true
    }

    pub fn set_preset(&mut self, preset: Preset) {
        (self.review, self.block) = preset.marks();
    }

    /// Moves a threshold by `delta`, keeping review at most block.
    pub fn nudge(&mut self, mark: Mark, delta: f64) {
        match mark {
            Mark::Review => {
                self.review =
                    round((self.review + delta).clamp(0.0, self.block));
            }
            Mark::Block => {
                self.block =
                    round((self.block + delta).clamp(self.review, 1.0));
            }
        }
    }

    /// What keeps the draft, written as `text`, from being saved.
    pub fn problem(&self, text: &str) -> Option<&'static str> {
        if text.trim().is_empty() {
            Some(
                "Write the rule: one sentence, the way you would tell a person.",
            )
        } else if self.places.is_empty() {
            Some("Pick at least one place, or Jev has nothing to check.")
        } else {
            None
        }
    }

    /// What adds the rule, or saves the one being edited, as `text`.
    pub fn save(&self, text: &str) -> Act {
        let (text, on) = (text.trim().to_owned(), self.places.clone());
        let (review, block) = (self.review, self.block);
        match &self.editing {
            Some(id) => Act::Update {
                repo: self.repo.clone(),
                id: id.clone(),
                text,
                on,
                review,
                block,
            },
            None => Act::Add {
                repo: self.repo.clone(),
                text,
                on,
                review,
                block,
            },
        }
    }
}

/// What the page's UI state holds as it is drawn, for its clicks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub draft: Option<Draft>,
    /// The drafts of [`RULE_TEXT`] and [`RULE_ON`].
    pub text: String,
    pub on: String,
    pub dismissed: Vec<(String, String)>,
}

impl Snapshot {
    pub fn of(view: &ViewCx<'_, ConstitutionUi>) -> Self {
        let draft = |id: &str| {
            view.ui_value(&tree::draft_key(id))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Self {
            draft: view
                .ui_value(EDITOR)
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
            text: draft(RULE_TEXT),
            on: draft(RULE_ON),
            dismissed: dismissed(view),
        }
    }

    /// The rule as written: its field, or what Enter took from it.
    pub fn rule_text(&self) -> String {
        match (self.text.trim(), &self.draft) {
            (
                "",
                Some(Draft {
                    kept: Some(kept), ..
                }),
            ) => kept.clone(),
            (text, _) => text.to_owned(),
        }
    }

    /// What keeps the draft from being saved, if anything.
    pub fn problem(&self) -> Option<&'static str> {
        self.draft.as_ref()?.problem(&self.rule_text())
    }
}

/// What a click on the page asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum Do {
    /// Opens the editor on a new rule, or on `rule`.
    Open {
        repo: String,
        rule: Option<RuleInfo>,
    },
    Close,
    /// Picks or unpicks a place.
    Place {
        place: String,
    },
    /// Adds the field typed under the places: `typed`, as Enter took
    /// it, or the field's draft.
    AddPlace {
        typed: Option<String>,
    },
    Preset {
        preset: Preset,
    },
    Nudge {
        mark: Mark,
        delta: f64,
    },
    /// Adds or saves the rule: `typed`, as Enter took it, or the
    /// field's draft. With something missing, the editor says what.
    Save {
        typed: Option<String>,
    },
    /// Tries the draft on `calls` and `answers` the rule would read; the
    /// host asks Jev and answers with a [`TrialResult`].
    Try {
        calls: Vec<(String, Value)>,
        answers: Vec<String>,
    },
    /// Asks to remove `repo`'s unreadable rules, to be confirmed;
    /// `None` takes the question back.
    AskReset {
        repo: Option<String>,
    },
    /// Removes `repo`'s unreadable rules, as confirmed.
    Reset {
        repo: String,
    },
    /// Takes a flagged call or answer off the review queue, for good.
    Reviewed {
        run: String,
        key: String,
    },
    OpenRun {
        run: String,
    },
    JevKey,
    /// Asks the host half for `act` as it is.
    Host {
        act: Act,
    },
}

fn set(key: impl Into<String>, value: Option<Value>) -> Request {
    Request::Ui {
        key: key.into(),
        value,
    }
}

fn host(act: Act) -> Request {
    Request::Act(serde_json::to_value(act).unwrap_or_default())
}

fn value(of: &impl Serialize) -> Option<Value> {
    serde_json::to_value(of).ok()
}

/// What `doing` asks of the interface, from the page as `now` holds it.
pub fn apply(now: &Snapshot, doing: Do) -> Vec<Request> {
    // The editor changed: its new draft, and nothing tried with it yet.
    let edited =
        |draft: &Draft| vec![set(EDITOR, value(draft)), set(TRYING, None)];
    match doing {
        Do::Open { repo, rule } => vec![
            set(EDITOR, value(&Draft::new(&repo, rule.as_ref()))),
            set(TRYING, None),
            set(
                tree::draft_key(RULE_TEXT),
                rule.map(|rule| Value::String(rule.text)),
            ),
            set(tree::draft_key(RULE_ON), None),
        ],
        Do::Close => vec![
            set(EDITOR, None),
            set(TRYING, None),
            set(tree::draft_key(RULE_TEXT), None),
            set(tree::draft_key(RULE_ON), None),
        ],
        Do::Reset { repo } => {
            vec![set(RESETTING, None), host(Act::Reset { repo })]
        }
        Do::AskReset { repo } => {
            vec![set(RESETTING, repo.map(Value::String))]
        }
        Do::Reviewed { run, key } => {
            let mut dismissed = now.dismissed.clone();
            dismissed.push((run.clone(), key.clone()));
            vec![
                set(DISMISSED, value(&dismissed)),
                host(Act::Reviewed { run, key }),
            ]
        }
        Do::OpenRun { run } => vec![Request::OpenRun(RunId(run.into()))],
        Do::JevKey => vec![Request::AskJevKey],
        Do::Host { act } => vec![host(act)],
        editing => {
            let Some(mut draft) = now.draft.clone() else {
                return Vec::new();
            };
            match editing {
                Do::Place { place } => {
                    draft.toggle_place(&place);
                    edited(&draft)
                }
                Do::AddPlace { typed } => {
                    let entered = typed.is_some();
                    let added =
                        draft.add_place(typed.as_deref().unwrap_or(&now.on));
                    let mut asked = edited(&draft);
                    if added && !entered {
                        asked.push(set(tree::draft_key(RULE_ON), None));
                    }
                    asked
                }
                Do::Preset { preset } => {
                    draft.set_preset(preset);
                    vec![set(EDITOR, value(&draft))]
                }
                Do::Nudge { mark, delta } => {
                    draft.nudge(mark, delta);
                    vec![set(EDITOR, value(&draft))]
                }
                Do::Save { typed } => {
                    let text = typed.clone().unwrap_or_else(|| now.rule_text());
                    if draft.problem(&text).is_some() {
                        draft.tried_to_save = true;
                        // Enter emptied the field: keep what it held.
                        if typed.is_some() {
                            draft.kept = Some(text);
                        }
                        return vec![set(EDITOR, value(&draft))];
                    }
                    vec![
                        set(EDITOR, None),
                        set(TRYING, None),
                        set(tree::draft_key(RULE_TEXT), None),
                        set(tree::draft_key(RULE_ON), None),
                        host(draft.save(&text)),
                    ]
                }
                Do::Try { calls, answers } => {
                    let text = now.rule_text();
                    if draft.problem(&text).is_some() {
                        draft.tried_to_save = true;
                        return vec![set(EDITOR, value(&draft))];
                    }
                    vec![
                        set(TRYING, value(&Trying::Asking)),
                        host(Act::Try {
                            text: text.trim().to_owned(),
                            on: draft.places,
                            review: draft.review,
                            block: draft.block,
                            calls,
                            answers,
                        }),
                    ]
                }
                _ => Vec::new(),
            }
        }
    }
}

/// The page's state in one window: only what its host half answers,
/// which goes to the UI state as what trying a rule gave.
pub struct Ui {
    handle: Handle,
}

impl Ui {
    pub(super) fn new(handle: Handle) -> Self {
        Self { handle }
    }

    /// What trying the rule gave.
    pub fn tried(&mut self, result: TrialResult, cx: &mut Context<Self>) {
        self.handle.set_ui(TRYING, value(&Trying::from(result)), cx);
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

/// A flagged call or answer that waits for a person.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewItem {
    pub run: RunId,
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

/// The flagged calls looked at in this window, before the host says so.
fn dismissed(view: &ViewCx<'_, ConstitutionUi>) -> Vec<(String, String)> {
    view.ui_value(DISMISSED)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// Whether `(run, key)` was looked at, here or as the host says.
fn looked_at(
    view: &ViewCx<'_, ConstitutionUi>,
    dismissed: &[(String, String)],
    run: &str,
    key: &str,
) -> bool {
    let entry = (run.to_owned(), key.to_owned());
    dismissed.contains(&entry) || view.data.reviewed.contains(&entry)
}

/// Flagged calls and answers in `repo`'s runs nobody has looked at.
pub fn review_items(
    view: &ViewCx<'_, ConstitutionUi>,
    repo: &str,
) -> Vec<ReviewItem> {
    let dismissed = dismissed(view);
    view.runs()
        .into_iter()
        .filter(|(run, _)| run.repo == repo)
        .flat_map(|(run, state)| {
            state
                .flags()
                .into_iter()
                .filter(|flag| {
                    !looked_at(view, &dismissed, &run.id.0, &flag.key)
                })
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
    let dismissed = dismissed(view);
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
            if looked_at(view, &dismissed, &run.id.0, &flag.key) {
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

/// What a tree's buttons, switches and inputs ask: the [`Do`] in their
/// arguments, from the page as `now` holds it.
fn on_do(
    handle: Handle,
    now: Snapshot,
) -> impl Fn(&str, &Value, &mut App) + 'static {
    move |action, args, cx| {
        let typed = || args["text"].as_str().map(str::to_owned);
        let doing = match action {
            DO => serde_json::from_value::<Do>(args.clone()).ok(),
            SAVE_TYPED => Some(Do::Save { typed: typed() }),
            ADD_TYPED => Some(Do::AddPlace { typed: typed() }),
            _ => None,
        };
        for request in doing.map(|doing| apply(&now, doing)).unwrap_or_default()
        {
            handle.request(request, cx);
        }
    }
}

/// A button that does `doing`.
fn button(label: &str, kind: Kind, doing: &Do) -> Node {
    Node::Button {
        label: label.into(),
        action: DO.into(),
        args: value(doing).unwrap_or_default(),
        kind,
        icon: None,
    }
}

fn icon(name: &str, tone: Tone) -> Node {
    Node::Icon {
        name: name.into(),
        tone,
    }
}

fn mono(text: impl Into<String>) -> Node {
    Node::Mono { text: text.into() }
}

fn heading(text: impl Into<String>) -> Node {
    Node::Heading { text: text.into() }
}

/// The page: its repository is the `repo` parameter; `rule`, when
/// given, is the rule to show. The repository's own page draws its name
/// above, so this starts with what the rules do.
pub fn render(view: &mut ViewCx<'_, ConstitutionUi>) -> AnyElement {
    let now = Snapshot::of(view);
    let tree = tree(view, &now);
    let handle = view.handle.clone();
    view.tree("constitution", &tree, on_do(handle, now))
}

/// The page's tree: the editor while a rule is written, else what the
/// rules do, what is wrong with them, and the two lists.
pub fn tree(view: &ViewCx<'_, ConstitutionUi>, now: &Snapshot) -> Node {
    let Some(repo) = view.param("repo").map(str::to_owned) else {
        return Node::Empty {
            text: "Pick a repository to see its rules.".into(),
        };
    };
    if let Some(draft) = &now.draft {
        let trying = view
            .ui_value(TRYING)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        return editor(view, now, draft, &trying);
    }
    let focus = view.param("rule").map(str::to_owned);
    let rules = view.repo(&repo).cloned().unwrap_or_default();
    let stats = rules_stats(view, &repo);
    let jev = view.jev;
    let empty = rules.rules.is_empty() && rules.error.is_none();
    let resetting = view.ui_value(RESETTING).and_then(Value::as_str);
    let mut page = Vec::new();
    if !empty {
        page.push(intro(&repo, &stats));
    }
    if let Some(banner) =
        broken(&rules, &repo, jev, resetting == Some(repo.as_str()))
    {
        page.push(banner);
    }
    if !jev && rules.error.is_none() {
        page.push(no_key(view.compact));
    }
    if empty {
        page.push(nothing_yet(&repo));
    } else if rules.error.is_none() {
        if let Some(rule) = focus.as_deref().and_then(|id| rules.rule(id)) {
            page.push(focused(&repo, rule, &stats, &last_caught(view, &repo)));
        }
        page.push(Node::Tabs {
            id: TABS.into(),
            tabs: vec![
                tree::Tab {
                    label: format!("Rules · {}", rules.rules.len()),
                    children: vec![
                        rules_list(view, &repo, &rules, &stats),
                        settings(&repo, &rules),
                    ],
                },
                tree::Tab {
                    label: format!("Review · {}", stats.waiting),
                    children: vec![review(view, &repo, &rules)],
                },
            ],
        });
    }
    Node::stack(page)
}

/// What the rules do and what they did over `repo`'s runs, each count
/// in its tone, and the way to add a rule.
fn intro(repo: &str, stats: &RulesStats) -> Node {
    let held: usize = stats.per_rule.values().map(|(_, _, held)| held).sum();
    let mut counts = Vec::new();
    if stats.runs == 0 {
        counts.push(Node::text("Nothing checked yet."));
    } else {
        counts.push(Node::text(format!(
            "Across {} {}:",
            stats.runs,
            if stats.runs == 1 { "run" } else { "runs" }
        )));
        counts.push(Node::Pill {
            text: format!("{} checked", stats.checked),
            tone: Tone::Neutral,
        });
        for (count, what, tone) in [
            (stats.blocked, "blocked", Tone::Bad),
            (stats.flagged, "flagged", Tone::Warn),
            (held, "held", Tone::Info),
            (stats.failed as usize, "not checked", Tone::Warn),
        ] {
            if count > 0 {
                counts.push(Node::Pill {
                    text: format!("{count} {what}"),
                    tone,
                });
            }
        }
        if stats.cost > 0.0 {
            counts.push(Node::text("Jev cost"));
            counts.push(mono(if stats.cost < 0.01 {
                format!("${:.5}", stats.cost)
            } else {
                usd(stats.cost)
            }));
        }
    }
    Node::stack([
        Node::row([
            Node::Rich {
                text: "**tau-constitution** checks every tool call and final \
                       answer against these rules before they run."
                    .into(),
            },
            Node::Spacer,
            button(
                "Add a rule",
                Kind::Secondary,
                &Do::Open {
                    repo: repo.to_owned(),
                    rule: None,
                },
            ),
        ]),
        Node::row(counts),
    ])
}

/// Rules that cannot be read from the store: why, what it means, and
/// the way out: removing them, once confirmed.
fn broken(
    rules: &Rules,
    repo: &str,
    jev: bool,
    confirming: bool,
) -> Option<Node> {
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
    let actions = if confirming {
        Node::row([
            Node::text(
                "This deletes them for good, so you can write them again.",
            ),
            button(
                "Delete",
                Kind::Danger,
                &Do::Reset {
                    repo: repo.to_owned(),
                },
            ),
            button("Keep", Kind::Secondary, &Do::AskReset { repo: None }),
        ])
    } else {
        Node::row([button(
            "Remove these rules",
            Kind::Danger,
            &Do::AskReset {
                repo: Some(repo.to_owned()),
            },
        )])
    };
    Some(Node::Card {
        children: vec![Node::stack([
            Node::row([icon("blocked", Tone::Bad), heading(title)]),
            Node::text(error),
            actions,
        ])],
    })
}

/// Without a TypeSafe key nothing is checked: say so, and where to fix it.
fn no_key(compact: bool) -> Node {
    let mut text = vec![heading("Rules aren't checked yet")];
    if !compact {
        text.push(Node::text(
            "tau-constitution asks Jev, TypeSafe's scoring model. Add a \
             TypeSafe key and every run checks these rules.",
        ));
    }
    Node::Card {
        children: vec![Node::row([
            icon("key", Tone::Info),
            Node::stack(text),
            Node::Spacer,
            button("Add TypeSafe key", Kind::Primary, &Do::JevKey),
        ])],
    }
}

/// No rules: what a rule is, and how to write one.
fn nothing_yet(repo: &str) -> Node {
    Node::Card {
        children: vec![Node::stack([
            icon("blocked", Tone::Neutral),
            heading(format!("No rules for {repo}")),
            Node::text(
                "A rule is one sentence, checked where you say: a shell \
                 command, an edit, a written file, or the final answer. Past \
                 its block mark the call is refused and the model gets the \
                 rule back.",
            ),
            Node::row([button(
                "New rule",
                Kind::Primary,
                &Do::Open {
                    repo: repo.to_owned(),
                    rule: None,
                },
            )]),
        ])],
    }
}

/// A rule's verdict over its runs, in a few words, and its tone:
/// blocked bad, held info, flagged warn, nothing caught good.
fn verdict(rule: &RuleInfo, stats: &RulesStats) -> (String, Tone) {
    let (blocked, flagged, held) =
        stats.per_rule.get(&rule.id).copied().unwrap_or_default();
    let parts: Vec<String> =
        [(blocked, "blocked"), (held, "held"), (flagged, "flagged")]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect();
    let tone = if blocked > 0 {
        Tone::Bad
    } else if held > 0 {
        Tone::Info
    } else if flagged > 0 {
        Tone::Warn
    } else {
        Tone::Good
    };
    if parts.is_empty() {
        ("clear".into(), tone)
    } else {
        (parts.join(" · "), tone)
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

/// Where a rule reads and how strict it is, and what it last caught.
fn caption(rule: &RuleInfo, last: &HashMap<String, String>) -> String {
    let mut caption = format!(
        "Checks {} · flag {:.2} · block {:.2}",
        rule.applies_to.join(", "),
        rule.review,
        rule.block
    );
    if let Some(shown) = last.get(&rule.id) {
        caption.push_str(&format!(" · last: {shown}"));
    }
    caption
}

/// Editing a rule, and removing it.
fn rule_actions(repo: &str, rule: &RuleInfo) -> Node {
    Node::row([
        button(
            "Edit",
            Kind::Secondary,
            &Do::Open {
                repo: repo.to_owned(),
                rule: Some(rule.clone()),
            },
        ),
        button(
            "Remove",
            Kind::Danger,
            &Do::Host {
                act: Act::Remove {
                    repo: repo.to_owned(),
                    id: rule.id.clone(),
                },
            },
        ),
    ])
}

/// The rule the page was opened on, in a card above the list.
fn focused(
    repo: &str,
    rule: &RuleInfo,
    stats: &RulesStats,
    last: &HashMap<String, String>,
) -> Node {
    let (said, tone) = verdict(rule, stats);
    Node::Card {
        children: vec![Node::stack([
            Node::row([
                mono(rule.id.clone()),
                heading(rule.text.clone()),
                Node::Spacer,
                Node::Badge { text: said, tone },
            ]),
            Node::text(caption(rule, last)),
            Node::Progress {
                share: rule.block,
                label: Some(format!(
                    "flags from {:.2}, blocks from {:.2}",
                    rule.review, rule.block
                )),
            },
            rule_actions(repo, rule),
        ])],
    }
}

/// The rules, one row each: the id, the rule with where it reads and
/// how strict it is, what it caught, and the ways to edit or remove it.
fn rules_list(
    view: &ViewCx<'_, ConstitutionUi>,
    repo: &str,
    rules: &Rules,
    stats: &RulesStats,
) -> Node {
    let last = last_caught(view, repo);
    let rows = rules
        .rules
        .iter()
        .map(|rule| {
            let (said, tone) = verdict(rule, stats);
            vec![
                mono(rule.id.clone()),
                Node::stack([
                    Node::text(rule.text.clone()),
                    Node::text(caption(rule, &last)),
                ]),
                Node::Badge { text: said, tone },
                rule_actions(repo, rule),
            ]
        })
        .collect();
    Node::Table {
        head: vec![
            "Rule".into(),
            String::new(),
            "Caught".into(),
            String::new(),
        ],
        rows,
    }
}

/// How the checks behave beyond each rule: what happens when Jev cannot
/// answer, and how many times an answer may go back, with a note on
/// what Jev reads.
fn settings(repo: &str, rules: &Rules) -> Node {
    let (blocks, holds) = (rules.blocks_unchecked, rules.max_holds);
    let set = |blocks_unchecked: bool, max_holds: u32| Do::Host {
        act: Act::Settings {
            repo: repo.to_owned(),
            blocks_unchecked,
            max_holds,
        },
    };
    Node::stack([
        heading("Settings"),
        Node::Switch {
            label: "Refuse what Jev can't answer".into(),
            detail: Some(if blocks {
                "Calls it could not check are refused, and answers sent back \
                 while holds are left."
                    .into()
            } else {
                "Calls it could not check run, and answers stand; each is \
                 noted."
                    .into()
            }),
            on: blocks,
            action: DO.into(),
            args: value(&set(!blocks, holds)).unwrap_or_default(),
        },
        Node::row([
            Node::stack([
                Node::text("Answers sent back per run"),
                Node::text(match holds {
                    0 => "A final answer that breaks a rule stands, flagged."
                        .to_owned(),
                    n => format!(
                        "Up to {n}; past that, an answer that breaks a rule \
                         stands, flagged."
                    ),
                }),
            ]),
            Node::Spacer,
            button("−", Kind::Secondary, &set(blocks, next_holds(holds, -1))),
            mono(holds.to_string()),
            button("+", Kind::Secondary, &set(blocks, next_holds(holds, 1))),
        ]),
        Node::text(
            "Jev sees only the fields a rule names, as the model wrote them. \
             Never tool output or file contents.",
        ),
    ])
}

/// What waits for a person, and what the rules handled on their own.
fn review(
    view: &ViewCx<'_, ConstitutionUi>,
    repo: &str,
    rules: &Rules,
) -> Node {
    let items = review_items(view, repo);
    let mut page = vec![heading(format!("Waiting for you · {}", items.len()))];
    if items.is_empty() {
        page.push(Node::Empty {
            text: "Nothing waits for you. Flagged calls land here.".into(),
        });
    }
    page.extend(items.iter().map(|item| {
        review_card(view.compact, item, rules.rule(&item.flag.rule), repo)
    }));
    page.push(heading("Handled by the rules"));
    let handled = handled(view, repo);
    if handled.is_empty() {
        page.push(Node::text("Nothing yet."));
    } else {
        page.push(Node::Table {
            head: Vec::new(),
            rows: handled
                .into_iter()
                .map(|done| {
                    let (glyph, tone, what) = match done.what {
                        HandledKind::Blocked => {
                            ("blocked", Tone::Bad, "blocked")
                        }
                        HandledKind::Held => ("chat", Tone::Info, "held"),
                        HandledKind::LookedFine => {
                            ("check", Tone::Good, "looked fine")
                        }
                    };
                    vec![
                        icon(glyph, tone),
                        Node::stack([
                            mono(done.shown),
                            Node::text(done.run_title),
                        ]),
                        mono(format!("{} · {what}", done.rule)),
                    ]
                })
                .collect(),
        });
    }
    page.push(Node::text(
        "Blocked calls need nothing from you: the model got the rule and \
         changed the call. They show that a rule does its job.",
    ));
    Node::stack(page)
}

/// A flagged call or answer: what it was, the rule and how its score
/// sits against the rule's marks, and what to do with it.
fn review_card(
    compact: bool,
    item: &ReviewItem,
    rule: Option<&RuleInfo>,
    repo: &str,
) -> Node {
    let answer = item.flag.tool.is_none();
    let mut card = vec![
        Node::row([
            icon(if answer { "chat" } else { "runs" }, Tone::Neutral),
            mono(
                item.flag
                    .tool
                    .clone()
                    .unwrap_or_else(|| "final answer".into()),
            ),
            Node::text(format!("in {}", item.run_title)),
            Node::Spacer,
            Node::Badge {
                text: format!("p {:.2}", item.flag.score),
                tone: Tone::Warn,
            },
        ]),
        if answer {
            Node::text(item.flag.shown.clone())
        } else {
            Node::Code {
                lang: None,
                text: item.flag.shown.clone(),
            }
        },
    ];
    if let Some(rule) = rule {
        card.push(Node::row([
            mono(rule.id.clone()),
            Node::text(rule.text.clone()),
        ]));
        if !compact {
            card.push(Node::Progress {
                share: item.flag.score,
                label: Some(format!(
                    "p {:.2} · flags from {:.2}, blocks from {:.2}",
                    item.flag.score, rule.review, rule.block
                )),
            });
        }
    }
    if !compact {
        card.push(Node::text(if answer {
            "The answer stood, but its score is between this rule's flag and \
             block marks."
        } else {
            "It ran: its score is between this rule's flag and block marks."
        }));
    }
    card.push(Node::row([
        button(
            "Adjust rule",
            Kind::Secondary,
            &Do::Open {
                repo: repo.to_owned(),
                rule: rule.cloned(),
            },
        ),
        button(
            "Open in run",
            Kind::Secondary,
            &Do::OpenRun {
                run: item.run.0.to_string(),
            },
        ),
        button(
            "Looks fine",
            Kind::Primary,
            &Do::Reviewed {
                run: item.run.0.to_string(),
                key: item.flag.key.clone(),
            },
        ),
    ]));
    Node::Card {
        children: vec![Node::stack(card)],
    }
}

/// A field's name, and what it is for.
fn label_row(name: &str, hint: &str) -> Node {
    Node::row([heading(name), Node::text(hint)])
}

/// The rule editor, in the page's place while a rule is written: the
/// rule, where it applies, how strict it is, trying it, and saving it.
fn editor(
    view: &ViewCx<'_, ConstitutionUi>,
    now: &Snapshot,
    draft: &Draft,
    trying: &Trying,
) -> Node {
    let compact = view.compact;
    let problem = now.problem();
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
    let save = button(save_label, Kind::Primary, &Do::Save { typed: None });
    let cancel = button("Cancel", Kind::Secondary, &Do::Close);

    let mut rule = vec![
        label_row("Rule", "one sentence, the way you would tell a person"),
        Node::Input {
            id: RULE_TEXT.into(),
            placeholder: Some(
                "A rule in plain words: \"No unwrap or expect outside tests.\""
                    .into(),
            ),
            multiline: false,
            submit: Some(SAVE_TYPED.into()),
        },
    ];
    if let Some(kept) = draft.kept.as_ref().filter(|_| now.text.is_empty()) {
        rule.push(Node::row([
            Node::text("Kept:"),
            mono(kept.clone()),
            Node::text("type to write it again"),
        ]));
    }

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
    let mut applies =
        vec![label_row("Applies to", "what Jev reads for this rule")];
    applies.extend(offered.into_iter().map(|(place, what)| Node::Switch {
        label: match place.split_once('.') {
            Some((tool, field)) => format!("{tool} · {field}"),
            None => "Final answer".into(),
        },
        detail: Some(what.to_owned()),
        on: draft.places.contains(&place),
        action: DO.into(),
        args: value(&Do::Place { place }).unwrap_or_default(),
    }));
    applies.push(Node::row([
        Node::Input {
            id: RULE_ON.into(),
            placeholder: Some("tool.field".into()),
            multiline: false,
            submit: Some(ADD_TYPED.into()),
        },
        button("Add field", Kind::Secondary, &Do::AddPlace { typed: None }),
    ]));
    if let Some(bad) = &draft.bad_place {
        applies.push(Node::row([
            icon("warning", Tone::Bad),
            Node::text(format!("\"{bad}\" names no field: write tool.field.")),
        ]));
    }

    let presets = Preset::ALL.into_iter().map(|preset| {
        button(
            preset.label(),
            if draft.preset() == Some(preset) {
                Kind::Primary
            } else {
                Kind::Secondary
            },
            &Do::Preset { preset },
        )
    });
    let stepper = |name: &str, mark: Mark, value: f64| {
        Node::row([
            Node::text(name),
            button("−", Kind::Secondary, &Do::Nudge { mark, delta: -0.05 }),
            mono(format!("{value:.2}")),
            button("+", Kind::Secondary, &Do::Nudge { mark, delta: 0.05 }),
        ])
    };
    let mut strictness = vec![
        label_row("Strictness", "how sure Jev must be that the rule is broken"),
        Node::row(presets),
        Node::Progress {
            share: draft.block,
            label: Some(format!(
                "flags from {:.2}, blocks from {:.2}",
                draft.review, draft.block
            )),
        },
        Node::row([
            stepper("Flag at", Mark::Review, draft.review),
            stepper("Block at", Mark::Block, draft.block),
        ]),
    ];
    if !compact {
        strictness.push(Node::Rows {
            rows: vec![
                KeyValue {
                    key: format!("Below {:.2}", draft.review),
                    value: "The call runs.".into(),
                },
                KeyValue {
                    key: format!("{:.2} to {:.2}", draft.review, draft.block),
                    value: "It runs, and waits in Review for you.".into(),
                },
                KeyValue {
                    key: format!("{:.2} and up", draft.block),
                    value: "Refused. The model gets the rule and tries again."
                        .into(),
                },
            ],
        });
    }

    let mut body = vec![
        Node::row([heading(title), Node::Spacer, cancel.clone(), save.clone()]),
        Node::stack(rule),
        Node::stack(applies),
    ];
    if let Some(problem) = shown_problem {
        body.push(Node::row([icon("warning", Tone::Bad), Node::text(problem)]));
    }
    body.push(Node::Card {
        children: vec![Node::stack(strictness)],
    });
    body.push(try_section(view, draft, trying));
    if !compact {
        body.push(Node::row([
            Node::text(match (shown_problem, &draft.editing) {
                (Some(problem), _) => problem.to_owned(),
                (None, Some(id)) => format!(
                    "Saves over {id}; runs check with it from their next tool \
                     call"
                ),
                (None, None) => "Adds it; runs check with it from their next \
                                 tool call"
                    .to_owned(),
            }),
            Node::Spacer,
            cancel,
            save,
        ]));
    }
    Node::Card {
        children: vec![Node::stack(body)],
    }
}

/// Trying the rule on the repository's latest calls, with Jev.
fn try_section(
    view: &ViewCx<'_, ConstitutionUi>,
    draft: &Draft,
    trying: &Trying,
) -> Node {
    let jev = view.jev;
    let (calls, answers) = trial_samples(view, &draft.repo, &draft.places);
    let head = Node::row([
        label_row("Try it", "before it runs for real"),
        Node::Spacer,
        button(
            match trying {
                Trying::Not => "Try on recent calls",
                _ => "Try again",
            },
            Kind::Secondary,
            &Do::Try { calls, answers },
        ),
    ]);
    let body = match trying {
        Trying::Not => Node::text(if jev {
            "Jev scores the latest calls and answers this rule would read, \
             as a check would. Nothing runs again."
        } else {
            "Trying a rule asks Jev: add a TypeSafe key first."
        }),
        Trying::Asking => {
            Node::row([icon("spinner", Tone::Info), Node::text("Asking Jev…")])
        }
        Trying::Failed(error) => {
            Node::row([icon("warning", Tone::Bad), Node::text(error.clone())])
        }
        Trying::Done { trials, .. } if trials.is_empty() => Node::text(
            "No calls in this repository's runs that the rule reads yet.",
        ),
        Trying::Done { trials, cost } => {
            let rows = trials
                .iter()
                .map(|trial| {
                    let (tone, verdict) = if trial.score >= draft.block {
                        (Tone::Bad, "would block")
                    } else if trial.score >= draft.review {
                        (Tone::Warn, "would flag")
                    } else {
                        (Tone::Neutral, "would run")
                    };
                    vec![
                        mono(
                            trial
                                .tool
                                .clone()
                                .unwrap_or_else(|| "answer".into()),
                        ),
                        mono(trial.shown.clone()),
                        Node::Progress {
                            share: trial.score,
                            label: None,
                        },
                        Node::Badge {
                            text: format!("{:.2} {verdict}", trial.score),
                            tone,
                        },
                    ]
                })
                .collect();
            Node::stack([
                Node::Table {
                    head: Vec::new(),
                    rows,
                },
                Node::text(format!(
                    "{} {} · {}. Only what a check shows went to Jev; nothing \
                     ran again.",
                    trials.len(),
                    if trials.len() == 1 { "call" } else { "calls" },
                    usd(*cost)
                )),
            ])
        }
    };
    Node::stack([head, body])
}
