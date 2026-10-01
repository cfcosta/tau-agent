//! tau-constitution's UI (ADR 0017): each repository's rules on their
//! page, with the editor that writes and tries a rule and the calls and
//! answers that wait for a person; what the checks did on a run's cards,
//! in its transcript and inspector; and the repository's entry in the
//! sidebar.

pub mod page;
mod run;
pub mod stats;

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use gpui::{AppContext as _, Context, Entity};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_jev::Jev;
use tau_ui_kit::{
    assets::Icon,
    input::{InputEvent, TextInput},
    theme::Tone,
};
use tau_ui_plugin::{
    CardMark,
    Fold,
    Handle,
    HostCx,
    Link,
    Manifest,
    NO_KEY,
    NavEntry,
    Page,
    PluginHost,
    PluginInfo,
    PluginUi,
    RepoCtx,
    RunCtx,
    RunCx,
    Seam,
    UiPlugin,
    needs_jev,
    points::{self, AtRepo},
};

pub use self::stats::Stats;
use crate::{
    Constitution,
    ConstitutionPlugin,
    Live,
    NAME,
    OnError,
    Record,
    Trial,
    VerdictKind,
    db::Db,
    rules::Target,
};

/// tau-constitution with its UI: what tau adds to an agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConstitutionUi;

/// The plugin on the host: each repository's rules as runs check them,
/// and where it keeps what a person looked at.
pub struct Host {
    /// The plugin's own database, opened when first needed.
    db: OnceLock<Db>,
    path: PathBuf,
    runtime: tokio::runtime::Handle,
    /// By [`rules_key`]: an edit replaces them here, and every run's
    /// next check reads the new ones.
    constitutions: Mutex<HashMap<String, Live>>,
}

/// What tau-constitution knows across repositories.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    /// Flagged calls and answers a person found fine, as `(run, key)`:
    /// off the review queue.
    pub reviewed: Vec<(String, String)>,
}

/// One repository's constitution, for its page.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub rules: Vec<RuleInfo>,
    /// How many times one run's final answer may be sent back.
    pub max_holds: u32,
    /// What Jev cannot answer is refused, not let through.
    pub blocks_unchecked: bool,
    /// Why the rules could not be read from the store, if they could
    /// not: runs fail until they can.
    pub error: Option<String>,
    /// What the checks did in each stored run of the repository that has
    /// any: the page counts runs no longer loaded with these.
    pub history: Vec<(String, Stats)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleInfo {
    pub id: String,
    pub text: String,
    /// `tool.field` names, or `final answer`.
    pub applies_to: Vec<String>,
    pub review: f64,
    pub block: f64,
}

impl Rules {
    pub fn rule(&self, id: &str) -> Option<&RuleInfo> {
        self.rules.iter().find(|rule| rule.id == id)
    }
}

/// What the checks did in one run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// What it says as the run starts: off, or how many rules it watches.
    pub starting: Option<String>,
    pub stats: Stats,
    /// What the checks made of each call, by call id.
    pub calls: BTreeMap<String, Call>,
    /// Each note in the transcript, by its anchor.
    pub notes: BTreeMap<String, Note>,
}

/// What the checks made of one call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Call {
    pub tool: String,
    /// What the card shows of it: a path, a command.
    pub shown: String,
    /// Every rule's score on it, passed or not.
    pub scores: Vec<(String, f64)>,
    /// The verdict, when a rule was broken.
    pub verdict: Option<CallVerdict>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallVerdict {
    pub kind: VerdictKind,
    pub rule: String,
    pub score: f64,
}

/// One of the plugin's notes in a transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub text: String,
    pub detail: String,
    pub tone: Tone,
}

impl Fold for State {
    type Record = Record;

    /// Folds one of the plugin's reports or records, or what it says as
    /// a run starts.
    fn apply(&mut self, record: Record, run: &mut dyn RunCx) {
        if let Record::Starting { status } = record {
            self.starting = status;
            return;
        }
        self.stats.add(&record, run.last_text());
        let verdict = match record {
            Record::Error(failure) => {
                // What `on_error` did with what it could not check.
                let call = failure.call_id.is_some();
                let detail = match (failure.on_error.as_str(), call) {
                    ("block", true) => "not checked · blocked",
                    ("block", false) if failure.held => {
                        "not checked · sent back"
                    }
                    (_, true) => "not checked · ran",
                    (_, false) => "not checked · the answer stands",
                };
                self.note(
                    run,
                    failure.message,
                    detail.to_owned(),
                    Tone::Danger,
                );
                return;
            }
            Record::Checked(check) => {
                // Every score shows on the call's card, passed or not.
                if let Some(call_id) = &check.call_id {
                    let scores = check
                        .scores
                        .iter()
                        .map(|score| (score.rule.clone(), score.score))
                        .collect();
                    self.call(run, call_id).scores = scores;
                }
                return;
            }
            Record::Blocked(verdict)
            | Record::Flagged(verdict)
            | Record::Held(verdict) => verdict,
            Record::Starting { .. } => return,
        };
        if let Some(call_id) = &verdict.call_id {
            run.mark(
                call_id,
                match verdict.kind {
                    VerdictKind::Blocked => CardMark::Blocked {
                        reason: verdict
                            .reason
                            .clone()
                            .unwrap_or_else(|| verdict.text.clone()),
                    },
                    _ => CardMark::Flagged,
                },
            );
            self.call(run, call_id).verdict = Some(CallVerdict {
                kind: verdict.kind,
                rule: verdict.rule,
                score: verdict.score,
            });
            return;
        }
        let (text, tone) = match verdict.kind {
            VerdictKind::Held => (
                format!(
                    "held the stop: the answer breaks {} (\"{}\")",
                    verdict.rule, verdict.text
                ),
                Tone::Warn,
            ),
            _ => (
                format!(
                    "flagged the answer for review: {} (\"{}\")",
                    verdict.rule, verdict.text
                ),
                Tone::Warn,
            ),
        };
        let detail = match (verdict.hold, verdict.max_holds) {
            (Some(hold), Some(max)) => {
                format!("before_stop · continuation {hold} / {max}")
            }
            _ => format!("before_stop · {:.2}", verdict.score),
        };
        self.note(run, text, detail, tone);
    }
}

impl State {
    /// The call `call_id`, as its card shows it, with its anchor on the
    /// card.
    fn call(&mut self, run: &mut dyn RunCx, call_id: &str) -> &mut Call {
        if !self.calls.contains_key(call_id) {
            let card =
                run.cards().into_iter().find(|card| card.call_id == call_id);
            self.calls.insert(
                call_id.to_owned(),
                Call {
                    tool: card
                        .as_ref()
                        .map_or_else(String::new, |card| card.tool.clone()),
                    shown: card.map_or_else(String::new, |card| card.summary),
                    ..Call::default()
                },
            );
        }
        run.attach(call_id, call_id);
        self.calls.get_mut(call_id).expect("inserted")
    }

    fn note(
        &mut self,
        run: &mut dyn RunCx,
        text: String,
        detail: String,
        tone: Tone,
    ) {
        let key = format!("c{}", self.notes.len());
        self.notes.insert(key.clone(), Note { text, detail, tone });
        run.transcript(&key);
    }

    /// Calls and answers flagged for a person in this run, as `(key,
    /// tool, shown, rule, score)`: a call by its id, an answer as
    /// `answer-N`.
    pub fn flags(&self) -> Vec<Flag> {
        let calls = self.calls.iter().filter_map(|(id, call)| {
            let verdict = call.verdict.as_ref()?;
            (verdict.kind == VerdictKind::Flagged).then(|| Flag {
                key: id.clone(),
                tool: Some(call.tool.clone()),
                shown: call.shown.clone(),
                rule: verdict.rule.clone(),
                score: verdict.score,
            })
        });
        let answers =
            self.stats
                .flagged_answers
                .iter()
                .enumerate()
                .map(|(n, answer)| Flag {
                    key: format!("answer-{n}"),
                    tool: None,
                    shown: answer.answer.clone(),
                    rule: answer.rule.clone(),
                    score: answer.score,
                });
        calls.chain(answers).collect()
    }

    /// The plugin's line in the run's plugin list, and its tone.
    pub fn status(&self) -> Option<(String, Tone)> {
        if self.stats.is_empty() {
            return Some((self.starting.clone()?, Tone::Quiet));
        }
        let tone = if self.stats.blocked.is_empty() {
            Tone::Quiet
        } else {
            Tone::Danger
        };
        Some((self.stats.summary(), tone))
    }
}

/// A call or final answer flagged for a person.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    pub key: String,
    /// The tool, or `None` for a final answer.
    pub tool: Option<String>,
    pub shown: String,
    pub rule: String,
    pub score: f64,
}

/// What a run's checks say as it starts: off without a key, else how
/// many rules they watch.
pub fn starting_status(jev: bool, rules: Option<usize>) -> String {
    match (jev, rules) {
        (false, _) => NO_KEY.into(),
        (true, None) => "the rules cannot be read".into(),
        (true, Some(0)) => "no rules".into(),
        (true, Some(1)) => "watching 1 rule".into(),
        (true, Some(n)) => format!("watching {n} rules"),
    }
}

/// What the UI asks the host half to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "act")]
pub enum Act {
    /// Adds a rule to `repo`; `on` names where it applies.
    Add {
        repo: String,
        text: String,
        on: Vec<String>,
        review: f64,
        block: f64,
    },
    /// Rewrites rule `id` in place.
    Update {
        repo: String,
        id: String,
        text: String,
        on: Vec<String>,
        review: f64,
        block: f64,
    },
    Remove {
        repo: String,
        id: String,
    },
    /// What Jev cannot answer, and how many times an answer may go back.
    Settings {
        repo: String,
        blocks_unchecked: bool,
        max_holds: u32,
    },
    /// Replaces rules that cannot be read with none.
    Reset {
        repo: String,
    },
    /// Asks Jev what a rule being written makes of past calls (tool,
    /// arguments) and final answers.
    Try {
        text: String,
        on: Vec<String>,
        review: f64,
        block: f64,
        calls: Vec<(String, Value)>,
        answers: Vec<String>,
    },
    /// A flagged call or answer looked fine.
    Reviewed {
        run: String,
        key: String,
    },
}

/// What a rule tried on past calls gave: each trial and what Jev cost,
/// or why it could not be tried.
pub type TrialResult = Result<(Vec<Trial>, f64), String>;

/// What a repository's rules are stored under: its checkout.
fn rules_key(repo: &RepoCtx) -> String {
    std::fs::canonicalize(&repo.checkout)
        .unwrap_or_else(|_| repo.checkout.clone())
        .display()
        .to_string()
}

impl Host {
    /// The plugin's database, opened the first time.
    fn db(&self) -> anyhow::Result<&Db> {
        if let Some(db) = self.db.get() {
            return Ok(db);
        }
        let db = self.runtime.block_on(Db::open(&self.path))?;
        Ok(self.db.get_or_init(|| db))
    }

    /// Repository `repo`'s rules as runs check them, read from the
    /// database the first time.
    fn constitution(&self, repo: &RepoCtx) -> anyhow::Result<Live> {
        let key = rules_key(repo);
        let mut open = self.constitutions.lock().expect("not poisoned");
        if let Some(live) = open.get(&key) {
            return Ok(live.clone());
        }
        let loaded = self
            .runtime
            .block_on(Constitution::load(self.db()?, &key))?;
        let live = Live::new(loaded);
        open.insert(key, live.clone());
        Ok(live)
    }

    /// The repository named `name`.
    fn repo<'a>(cx: &'a HostCx, name: &str) -> anyhow::Result<&'a RepoCtx> {
        cx.repos
            .iter()
            .find(|repo| repo.name == name)
            .ok_or_else(|| anyhow::anyhow!("No repository {name}"))
    }

    /// Changes `repo`'s constitution with `edit`, which checks what it
    /// adds, and saves it. Runs going on check with the new rules from
    /// their next tool call.
    fn edit(
        &self,
        cx: &HostCx,
        repo: &str,
        edit: impl FnOnce(&mut Constitution) -> Result<(), crate::RuleError>,
    ) -> anyhow::Result<()> {
        let repo = Self::repo(cx, repo)?;
        let live = self.constitution(repo)?;
        let mut constitution = (*live.get()).clone();
        edit(&mut constitution)?;
        self.runtime
            .block_on(constitution.save(self.db()?, &rules_key(repo)))?;
        live.set(constitution);
        Ok(())
    }

    /// Replaces `repo`'s stored constitution with an empty one: the way
    /// out when the stored one cannot be read, which no edit can fix.
    fn reset(&self, cx: &HostCx, repo: &str) -> anyhow::Result<()> {
        let repo = Self::repo(cx, repo)?;
        let key = rules_key(repo);
        let fresh = Constitution::default();
        self.runtime.block_on(fresh.save(self.db()?, &key))?;
        let mut open = self.constitutions.lock().expect("not poisoned");
        match open.get(&key) {
            Some(live) => live.set(fresh),
            None => {
                open.insert(key, Live::new(fresh));
            }
        }
        Ok(())
    }

    /// What a person reviewed; none when the database cannot be read.
    fn reviewed(&self) -> Vec<(String, String)> {
        let reviewed = self
            .db()
            .and_then(|db| Ok(self.runtime.block_on(db.reviewed())?));
        reviewed.unwrap_or_else(|error| {
            eprintln!("{NAME}: could not read what was reviewed: {error:#}");
            Vec::new()
        })
    }

    fn set_reviewed(&self, run: String, key: String) -> anyhow::Result<()> {
        Ok(self
            .runtime
            .block_on(self.db()?.mark_reviewed(&run, &key))?)
    }

    /// What the checks did in each stored run of `repo`, from the
    /// plugin's records, counted as a run's view counts them.
    fn history(&self, repo: &RepoCtx, cx: &HostCx) -> Vec<(String, Stats)> {
        let (Ok(records), Ok(ours)) =
            (cx.records_everywhere(NAME), cx.runs_in(repo))
        else {
            return Vec::new();
        };
        let mut history: Vec<(String, Stats)> = Vec::new();
        for (run, body) in records {
            if !ours.contains(&run) {
                continue;
            }
            let run = run.0.to_string();
            // Records come by run, so a run's are together.
            if history.last().is_none_or(|(last, _)| *last != run) {
                history.push((run, Stats::default()));
            }
            if let Some((_, stats)) = history.last_mut()
                && let Some(record) = Record::parse(&body)
            {
                stats.add(&record, None);
            }
        }
        history
    }
}

impl UiPlugin for ConstitutionUi {
    type State = State;
    type Data = Data;
    type RepoData = Rules;
    type Settings = ();
    type Host = Host;
    type Ui = page::Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    /// The repository's rules, checked with Jev when there is a key, in
    /// a run and its sub-agents alike. Rules that cannot be read fail
    /// the run: they are never skipped.
    fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let Some(jev) = run.services.get::<Arc<dyn Jev>>() else {
            return Ok(Vec::new());
        };
        let rules = host.constitution(&run.repo)?;
        Ok(vec![Box::new(ConstitutionPlugin::live(jev.clone(), rules))])
    }

    fn starting(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &(),
    ) -> Vec<Record> {
        let jev = run.services.get::<Arc<dyn Jev>>().is_some();
        let rules = host
            .constitution(&run.repo)
            .ok()
            .map(|live| live.get().rules.len());
        vec![Record::Starting {
            status: Some(starting_status(jev, rules)),
        }]
    }

    fn catalog(&self, host: &Host, cx: &HostCx, _settings: &()) -> PluginInfo {
        let jev = cx.services.get::<Arc<dyn Jev>>().is_some();
        let rules: usize = cx
            .repos
            .iter()
            .filter_map(|repo| host.constitution(repo).ok())
            .map(|live| live.get().rules.len())
            .sum();
        PluginInfo {
            description: if jev {
                format!(
                    "{rules} rules across your repositories, checked with Jev"
                )
            } else {
                needs_jev(false, "Checks calls against each repository's rules")
            },
            seams: vec![Seam::BeforeTool, Seam::BeforeStop],
            page: Some(Link::page("rules").param("repo", "")),
            ..Default::default()
        }
    }

    fn data(&self, host: &Host, _cx: &HostCx) -> Data {
        Data {
            reviewed: host.reviewed(),
        }
    }

    fn repo_data(&self, host: &Host, repo: &RepoCtx, cx: &HostCx) -> Rules {
        let history = host.history(repo, cx);
        match host.constitution(repo) {
            Ok(live) => {
                let loaded = live.get();
                Rules {
                    rules: loaded
                        .rules
                        .iter()
                        .map(|rule| RuleInfo {
                            id: rule.id.clone(),
                            text: rule.text.clone(),
                            applies_to: rule
                                .on
                                .iter()
                                .map(Target::label)
                                .collect(),
                            review: rule.review,
                            block: rule.block,
                        })
                        .collect(),
                    max_holds: loaded.max_holds,
                    blocks_unchecked: loaded.on_error == OnError::Block,
                    error: None,
                    history,
                }
            }
            Err(error) => Rules {
                error: Some(format!("{error:#}")),
                history,
                ..Rules::default()
            },
        }
    }

    fn act(
        &self,
        host: &Host,
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let act: Act = serde_json::from_value(action)?;
        let (done, failed) = match act {
            Act::Try {
                text,
                on,
                review,
                block,
                calls,
                answers,
            } => {
                let result =
                    try_rule(cx, &text, &on, review, block, &calls, &answers);
                return Ok(Some(serde_json::to_value(result)?));
            }
            Act::Reviewed { run, key } => {
                (host.set_reviewed(run, key), "Could not save the review")
            }
            Act::Add {
                repo,
                text,
                on,
                review,
                block,
            } => (
                host.edit(cx, &repo, |rules| {
                    rules.add(&text, &on, review, block).map(drop)
                }),
                "Could not add the rule",
            ),
            Act::Update {
                repo,
                id,
                text,
                on,
                review,
                block,
            } => (
                host.edit(cx, &repo, |rules| {
                    rules.replace(&id, &text, &on, review, block)
                }),
                "Could not save the rule",
            ),
            Act::Remove { repo, id } => (
                host.edit(cx, &repo, |rules| {
                    rules.remove(&id);
                    Ok(())
                }),
                "Could not remove the rule",
            ),
            Act::Settings {
                repo,
                blocks_unchecked,
                max_holds,
            } => (
                host.edit(cx, &repo, |rules| {
                    rules.on_error = if blocks_unchecked {
                        OnError::Block
                    } else {
                        OnError::Allow
                    };
                    rules.max_holds = max_holds;
                    Ok(())
                }),
                "Could not save the constitution",
            ),
            Act::Reset { repo } => {
                (host.reset(cx, &repo), "Could not remove the rules")
            }
        };
        if let Err(error) = done {
            cx.alert(failed, format!("{error:#}"));
        }
        // What the page shows comes from the host again, saved or not.
        cx.refresh();
        Ok(None)
    }

    fn reply(
        &self,
        ui: &mut page::Ui,
        reply: Value,
        _cx: &mut Context<page::Ui>,
    ) {
        if let Ok(result) = serde_json::from_value::<TrialResult>(reply) {
            ui.tried(result);
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(
                Page::new("rules", page::render)
                    .title(|_| "Constitution".to_owned()),
            )
            .contribute(points::SIDEBAR_REPO, sidebar)
            .contribute_at(points::STATUS, 10, run::status)
            .contribute(points::TRANSCRIPT, run::note)
            .contribute(points::CARD_BADGE, run::badge)
            .contribute(points::CARD_BODY, run::blocked)
            .contribute(points::INSPECTOR, run::inspector)
    }
}

impl PluginHost for Host {
    fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Host {
            db: OnceLock::new(),
            path: cx.plugin_dir(NAME).join("constitution.db"),
            runtime: cx.runtime.clone(),
            constitutions: Mutex::default(),
        })
    }
}

impl PluginUi for page::Ui {
    fn new(handle: Handle, cx: &mut Context<Self>) -> Self {
        let rule_text = cx.new(|cx| {
            TextInput::new(
                "A rule in plain words: \"No unwrap or expect outside tests.\"",
                cx,
            )
            .keep_on_submit()
        });
        let rule_on =
            cx.new(|cx| TextInput::new("tool.field", cx).keep_on_submit());
        // Enter in the rule saves it; Enter in a field adds the field.
        cx.subscribe(
            &rule_text,
            |ui: &mut page::Ui, _: Entity<TextInput>, _: &InputEvent, cx| {
                ui.save(cx)
            },
        )
        .detach();
        cx.subscribe(
            &rule_on,
            |ui: &mut page::Ui, _: Entity<TextInput>, _: &InputEvent, cx| {
                ui.add_other_place(cx);
            },
        )
        .detach();
        cx.observe(&rule_text, |_, _, cx| cx.notify()).detach();
        page::Ui::new(handle, rule_text, rule_on)
    }
}

/// Asks Jev what a rule being written makes of past `calls` and
/// `answers`, as a check would ask. Blocks.
fn try_rule(
    cx: &HostCx,
    text: &str,
    on: &[String],
    review: f64,
    block: f64,
    calls: &[(String, Value)],
    answers: &[String],
) -> TrialResult {
    let on = on
        .iter()
        .map(|place| Target::parse(place))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let rule = crate::Rule {
        id: "new".into(),
        text: text.to_owned(),
        on,
        review,
        block,
    };
    let jev = cx.services.get::<Arc<dyn Jev>>().ok_or_else(|| {
        "Trying a rule asks Jev: add a TypeSafe key on the Models screen."
            .to_owned()
    })?;
    cx.runtime
        .block_on(crate::try_rule(&**jev, &rule, calls, answers))
}

/// The repository's rules in the sidebar: how many, and what waits for
/// a person.
fn sidebar(
    at: &AtRepo,
    view: &mut tau_ui_plugin::ViewCx<'_, ConstitutionUi>,
) -> Option<NavEntry> {
    let rules = view.repo(&at.repo).map_or(0, |repo| repo.rules.len());
    let waiting = page::review_items(view, &at.repo).len();
    Some(
        NavEntry::new(
            "Constitution",
            Icon::Blocked,
            Link::page("rules").param("repo", at.repo.clone()),
        )
        .detail(format!("{rules} rules"))
        .badge((waiting > 0).then(|| (waiting.to_string(), Tone::Warn))),
    )
}
