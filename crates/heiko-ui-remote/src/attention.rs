//! Whether a chat needs the person, as its row in the sidebar and on a
//! phone says it: working, asking, conflicts on main, queued to land,
//! ready to land, would conflict, interrupted, landed, dropped.
//!
//! [`Attention::of`] is the one place that decides it, from a run's
//! [`Facts`]. The workspace gathers the facts
//! ([`Workspace::attention`]): the run's status and turn, what a plugin
//! holds it for ([`points::ASKS`]), the landing forecast the host works
//! out in the background ([`RunView::forecast`]), how it ended for good
//! ([`RunView::ending`]), its place in its main chat's landing queue
//! ([`RunView::landing_queue`]), and, for a main chat, the conflicts a
//! turn left on it ([`RunView::main_conflicts`]). All of them travel in a snapshot, so a
//! phone shows the same states.
//!
//! A row shows its state with [`Marks`] alone, packed at its right
//! edge in one order; Home lists what happened across runs and agents
//! ([`Workspace::since`]), each with the one thing to do about it.

use serde::{Deserialize, Serialize};
use heiko_agent::event::StopReason;
use heiko_ui_plugin::points;

use crate::{
    assets::State,
    route::Route,
    theme::Tone,
    view::{Ending, Origin, RunStatus, RunView},
    workspace::Workspace,
};

/// What landing a finished fork on its parent would do, as the host
/// last worked it out, in the background.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forecast {
    /// The changes it would land.
    pub changes: usize,
    /// The files that would conflict.
    pub conflicts: Vec<String>,
}

impl From<&heiko_ui_plugin::vcs::Landing> for Forecast {
    fn from(landing: &heiko_ui_plugin::vcs::Landing) -> Self {
        Self {
            changes: landing.changes.len(),
            conflicts: landing.conflicts.clone(),
        }
    }
}

/// Where a run is, as far as attention goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    /// Going: planning, or in a turn.
    #[default]
    Live,
    /// It stopped: done, cancelled, or at a limit. It waits for the
    /// person's next message, at their pace.
    Stopped,
    /// It stopped with an error.
    Failed,
    /// It was going when heiko closed.
    Interrupted,
}

impl Status {
    pub fn of(status: &RunStatus) -> Self {
        match status {
            RunStatus::Planning | RunStatus::Running => Self::Live,
            RunStatus::Interrupted => Self::Interrupted,
            RunStatus::Finished(StopReason::Error(_)) => Self::Failed,
            RunStatus::Finished(_) => Self::Stopped,
        }
    }
}

/// What [`Attention::of`] reads of a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub status: Status,
    /// The turn it is in.
    pub turn: u32,
    /// What a plugin holds it for the person to answer
    /// ([`points::ASKS`]).
    pub asks: Option<String>,
    /// What landing it on its parent would do, when the host worked it
    /// out.
    pub forecast: Option<Forecast>,
    /// How it ended for good: landed on its parent, or dropped.
    pub ending: Option<Ending>,
    /// Where it waits in its main chat's landing queue.
    pub queued: Option<Queued>,
    /// A main chat's: the files a turn left in conflict on it, while it
    /// is marked so.
    pub main_conflicts: Option<Vec<String>>,
}

/// A chat's place in its main chat's landing queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Queued {
    /// From 1: the first lands next.
    pub position: usize,
    /// It would conflict in a file the person did not confirm, so it
    /// waits for them ([`crate::queue::Waiting::needs_confirmation`]).
    pub needs_confirmation: bool,
}

/// Whether a run needs the person, and why. Rows never move for it:
/// the sidebar keeps its order, and only the row's icon and line change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attention {
    /// Going, in this turn.
    Working { turn: u32 },
    /// Waiting on the person to answer this.
    Asks { question: String },
    /// A finished fork whose changes would land cleanly.
    ReadyToLand { changes: usize },
    /// A finished fork whose landing would conflict in these files.
    WouldConflict { files: Vec<String> },
    /// A main chat a turn left conflicts on, in these files: nothing
    /// lands on it and no chat forks it until they are resolved.
    ConflictsOnMain { files: Vec<String> },
    /// Waiting in its main chat's landing queue.
    Queued(Queued),
    /// It was going when heiko closed.
    Interrupted,
    /// It stopped with an error.
    Failed,
    /// It landed on its parent and closed.
    Landed,
    /// It was dropped: its changes abandoned, and it closed.
    Dropped,
    /// Nothing: it waits for the person's next message, at their pace.
    Idle,
}

impl Attention {
    /// The one place a run's attention is decided. In order:
    ///
    /// 1. A run that landed or was dropped says so, whatever else it
    ///    says: it takes no more messages.
    /// 2. A live run asks, when a plugin holds it for an answer, and
    ///    otherwise works.
    /// 3. A main chat a turn left conflicts on says so.
    /// 4. A run heiko's closing interrupted, or that failed, says so: its
    ///    work did not finish, so what landing it would do is beside the
    ///    point.
    /// 5. A chat queued to land says where it waits.
    /// 6. A stopped fork says what landing it would do: conflict, or
    ///    land its changes. With nothing to land it is idle.
    pub fn of(facts: &Facts) -> Self {
        match facts.ending {
            Some(Ending::Landed { .. }) => return Self::Landed,
            Some(Ending::Dropped) => return Self::Dropped,
            None => {}
        }
        if facts.status == Status::Live {
            return match &facts.asks {
                Some(question) => Self::Asks {
                    question: question.clone(),
                },
                None => Self::Working { turn: facts.turn },
            };
        }
        if let Some(files) = &facts.main_conflicts {
            return Self::ConflictsOnMain {
                files: files.clone(),
            };
        }
        match facts.status {
            Status::Interrupted => return Self::Interrupted,
            Status::Failed => return Self::Failed,
            Status::Live | Status::Stopped => {}
        }
        if let Some(queued) = facts.queued {
            return Self::Queued(queued);
        }
        match &facts.forecast {
            Some(forecast) if !forecast.conflicts.is_empty() => {
                Self::WouldConflict {
                    files: forecast.conflicts.clone(),
                }
            }
            Some(forecast) if forecast.changes > 0 => Self::ReadyToLand {
                changes: forecast.changes,
            },
            _ => Self::Idle,
        }
    }

    /// Whether the repository's "N need you" counts it: it asks, would
    /// conflict, is ready to land, waits in the queue for the person to
    /// confirm its conflicts, or is a main chat with conflicts on it.
    pub fn needs_you(&self) -> bool {
        matches!(
            self,
            Self::Asks { .. }
                | Self::WouldConflict { .. }
                | Self::ReadyToLand { .. }
                | Self::ConflictsOnMain { .. }
                | Self::Queued(Queued {
                    needs_confirmation: true,
                    ..
                })
        )
    }

    /// What the state adds to its row's marks: a chat ready to land
    /// is met, one whose landing conflicts is failing by as many files,
    /// and one queued behind a conflict to confirm asks.
    pub fn marks(&self) -> Marks {
        let none = Marks::default();
        match self {
            Self::Working { .. } => Marks { working: 1, ..none },
            Self::Asks { .. }
            | Self::Queued(Queued {
                needs_confirmation: true,
                ..
            }) => Marks { asks: 1, ..none },
            Self::ReadyToLand { .. } => Marks { met: 1, ..none },
            Self::WouldConflict { files } | Self::ConflictsOnMain { files } => {
                Marks {
                    failing: files.len().max(1),
                    ..none
                }
            }
            Self::Failed => Marks { failing: 1, ..none },
            Self::Queued(_)
            | Self::Interrupted
            | Self::Landed
            | Self::Dropped
            | Self::Idle => none,
        }
    }

    /// The line under the run's title, if it has one.
    /// One word for it, as a list shows it: `running`, `to land`. Empty
    /// for a run that waits on nothing.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Working { .. } => "running",
            Self::Asks { .. } => "asks you",
            Self::ReadyToLand { .. } => "to land",
            Self::WouldConflict { .. } | Self::ConflictsOnMain { .. } => {
                "conflicts"
            }
            Self::Queued(_) => "queued",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::Landed => "landed",
            Self::Dropped => "dropped",
            Self::Idle => "",
        }
    }

    pub fn line(&self) -> Option<String> {
        Some(match self {
            Self::Working { turn } => format!("Working · turn {turn}"),
            Self::Asks { .. } => "Asks you a question".to_owned(),
            Self::ReadyToLand { changes } => {
                format!("Ready to land · {}", count(*changes, "change"))
            }
            Self::WouldConflict { files } => {
                format!("Would conflict in {}", count(files.len(), "file"))
            }
            Self::ConflictsOnMain { files } => {
                format!("Conflicts on main · {}", count(files.len(), "file"))
            }
            Self::Queued(Queued {
                needs_confirmation: true,
                ..
            }) => "Queued · needs confirmation".to_owned(),
            Self::Queued(_) => "Queued · lands after main's turn".to_owned(),
            Self::Interrupted => "Interrupted · heiko closed".to_owned(),
            Self::Failed | Self::Landed | Self::Dropped | Self::Idle => {
                return None;
            }
        })
    }
}

/// How many of the things a row stands for are in each state its
/// marks show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks {
    pub met: usize,
    pub working: usize,
    pub limit: usize,
    pub failing: usize,
    pub asks: usize,
}

impl Marks {
    /// The marks a row packs at its right edge, in their one order:
    /// met, working, limit, failing, asks. A state nothing is in has
    /// no mark, and a count shows only above one.
    pub fn shown(&self) -> Vec<(usize, State)> {
        [
            (self.met, State::Met),
            (self.working, State::Working),
            (self.limit, State::Limit),
            (self.failing, State::Failing),
            (self.asks, State::Asks),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .collect()
    }

    /// These and `other`'s, as a row over both shows them.
    pub fn and(self, other: Self) -> Self {
        Self {
            met: self.met + other.met,
            working: self.working + other.working,
            limit: self.limit + other.limit,
            failing: self.failing + other.failing,
            asks: self.asks + other.asks,
        }
    }
}

/// What a thing that happened is drawn with, before its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// The mark of the state it is in.
    State(State),
    /// An agent's word for the person.
    Note,
    /// Changes the forge lacks.
    Push,
}

/// What the one action of a thing that happened does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Opens a screen.
    Open(Route),
    /// Opens the chat on its landing card.
    Land(heiko_agent::tool::RunId),
    /// Opens an agent's conversation.
    Agent(String),
    /// Pushes a repository's trunk to the forge.
    Push(String),
}

/// One thing that happened, as Home lists it: when, its mark, whose it
/// is, one line, and the one thing to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Happened {
    /// `now`, or the time of day its run started; empty when unknown.
    pub when: String,
    /// The day its run started, as a heading says it; empty when
    /// unknown.
    pub day: String,
    pub sign: Sign,
    pub name: String,
    pub line: String,
    /// The action's word: `answer`, `read`, `push ↑3`.
    pub action: String,
    /// Whether it waits on the person: its action is the loud one.
    pub hot: bool,
    pub act: Act,
}

/// How many things Home lists at most.
pub const SINCE_SHOWN: usize = 14;

/// The day a run started on, as a heading: Today, Yesterday, Sep 26.
pub fn day(started: &str) -> String {
    let mut words = started.split_whitespace();
    match words.next() {
        Some("today") => "Today".into(),
        Some("yesterday") => "Yesterday".into(),
        Some(month) => match words.next() {
            Some(date) => format!("{month} {date}"),
            None => month.to_owned(),
        },
        None => "Earlier".into(),
    }
}

/// A key that orders runs by when they started, from what the run
/// list shows: `today 14:02`, `yesterday 22:10`, `Sep 26 19:02`.
pub fn when(started: &str) -> (u8, u8, u8, String) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
        "Nov", "Dec",
    ];
    let words: Vec<&str> = started.split_whitespace().collect();
    let clock = words.last().copied().unwrap_or_default().to_owned();
    match words.first().copied() {
        Some("today") => (2, 0, 0, clock),
        Some("yesterday") => (1, 0, 0, clock),
        Some(month) => {
            let month = MONTHS.iter().position(|m| *m == month).unwrap_or(0);
            let date = words.get(1).and_then(|d| d.parse().ok()).unwrap_or(0);
            (0, month as u8 + 1, date, clock)
        }
        None => (0, 0, 0, clock),
    }
}

/// When in its day a run started: `14:02`.
pub fn time(started: &str) -> String {
    started
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .to_owned()
}

/// `1 change`, `2 changes`.
pub fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

impl Workspace {
    /// What `run`'s row says about whether it needs the person.
    pub fn attention(&self, run: &RunView) -> Attention {
        Attention::of(&self.facts(run))
    }

    /// What [`Attention::of`] reads of `run`.
    pub fn facts(&self, run: &RunView) -> Facts {
        let asks = self
            .contributions(points::ASKS, &points::AtRun { run: run.info() })
            .into_iter()
            .next();
        Facts {
            status: Status::of(&run.status),
            turn: run.turn,
            asks,
            forecast: run.forecast.clone(),
            ending: run.ending.clone(),
            queued: self.queued(&run.id).map(|(position, waiting)| Queued {
                position,
                needs_confirmation: waiting.needs_confirmation(),
            }),
            main_conflicts: run
                .main_conflicts
                .as_ref()
                .map(|conflicts| conflicts.files.clone()),
        }
    }

    /// How many of `repo`'s open chats need the person
    /// ([`Attention::needs_you`]).
    pub fn need_you(&self, repo: &str) -> usize {
        self.runs
            .iter()
            .filter(|run| self.repo_of(run) == repo && !self.is_closed(&run.id))
            .filter(|run| self.attention(run).needs_you())
            .count()
    }

    /// How many of the things the agent `name` left wait on the person
    /// ([`points::WAITING`]).
    pub fn agent_waits(&self, name: &str) -> usize {
        self.waiting()
            .iter()
            .flat_map(|(_, waiting)| &waiting.groups)
            .filter(|group| group.name == name)
            .map(|group| group.items.len())
            .sum()
    }

    /// How many things need the person, everywhere: the chats that do
    /// in every repository, what agents left for them, and their own
    /// plugins that wait on them. Home's badge.
    pub fn need_you_everywhere(&self) -> usize {
        let chats: usize = self
            .catalog
            .repos
            .iter()
            .map(|repo| self.need_you(&repo.name))
            .sum();
        let left: usize = self
            .waiting()
            .iter()
            .flat_map(|(_, waiting)| &waiting.groups)
            .map(|group| group.items.len())
            .sum();
        chats + left + self.catalog.yours_need_you()
    }

    /// The marks of `run`'s row: its state's, a square when it stopped
    /// at a limit, and what a plugin says of it at rest (a goal met, or
    /// stopped short). With its crew `folded` under it, the sub-agents
    /// at work count as working too.
    pub fn run_marks(&self, run: &RunView, folded: bool) -> Marks {
        let mut marks = self.attention(run).marks();
        if run.ending.is_none() && !run.status.is_live() {
            if matches!(run.status, RunStatus::Finished(StopReason::Limit(_))) {
                marks.limit = 1;
            }
            match self.run_rows(run).first().map(|note| note.tone) {
                Some(Tone::Good) => marks.met = marks.met.max(1),
                Some(Tone::Danger) => marks.limit = marks.limit.max(1),
                _ => {}
            }
        }
        if folded {
            marks.working += self.working_crew(&run.id);
        }
        marks
    }

    /// What happened across runs and agents, as Home lists it: the
    /// runs at work first, then the others newest first (those that
    /// ask, are ready to land or would conflict, failed, met a goal or
    /// stopped at a limit, or have replies not read yet), then what
    /// agents left for the person, then each repository's changes the
    /// forge lacks. At most [`SINCE_SHOWN`] of them.
    pub fn since(&self) -> Vec<Happened> {
        let mut runs: Vec<(bool, &RunView, Happened)> = self
            .runs
            .iter()
            .filter(|run| !self.is_closed(&run.id) && run.ending.is_none())
            .filter_map(|run| {
                let happened = self.happened(run)?;
                Some((run.status.is_live(), run, happened))
            })
            .collect();
        runs.sort_by(|(a_live, a, _), (b_live, b, _)| {
            (b_live, when(&b.started)).cmp(&(a_live, when(&a.started)))
        });
        let mut list: Vec<Happened> =
            runs.into_iter().map(|(_, _, happened)| happened).collect();
        for (_, waiting) in self.waiting() {
            for group in waiting.groups {
                let act = match &group.run {
                    Some(run) => Act::Open(Route::Run(run.clone())),
                    None => Act::Agent(group.name.clone()),
                };
                list.extend(group.items.into_iter().map(|item| Happened {
                    when: String::new(),
                    day: String::new(),
                    sign: Sign::Note,
                    name: group.name.clone(),
                    line: if item.body.is_empty() {
                        item.title
                    } else {
                        item.body
                    },
                    action: "read".into(),
                    hot: false,
                    act: act.clone(),
                }));
            }
        }
        list.truncate(SINCE_SHOWN);
        let forge = self.catalog.forge_name();
        for repo in self.catalog.listed() {
            let Some((ahead, branch)) = self.unpushed(&repo.name) else {
                continue;
            };
            list.push(Happened {
                when: String::new(),
                day: String::new(),
                sign: Sign::Push,
                name: repo.name.clone(),
                line: format!(
                    "{} on {branch} not pushed to {forge}",
                    count(ahead as usize, "change")
                ),
                action: format!("push ↑{ahead}"),
                hot: false,
                act: Act::Push(repo.name.clone()),
            });
        }
        list
    }

    /// What Home says of `run`, if anything about it is news.
    fn happened(&self, run: &RunView) -> Option<Happened> {
        let attention = self.attention(run);
        // A sub-agent's main chat stands for it, unless it needs the
        // person itself.
        if matches!(run.origin, Origin::SubAgent { .. })
            && !attention.needs_you()
        {
            return None;
        }
        let open = Act::Open(Route::Run(run.id.clone()));
        let note = self.run_rows(run).into_iter().next();
        let said = |fallback: &str| {
            note.as_ref()
                .and_then(|note| {
                    note.phone_line.clone().or_else(|| note.line.clone())
                })
                .unwrap_or_else(|| fallback.to_owned())
        };
        let (state, line, action, hot, act) = match &attention {
            Attention::Working { turn } => (
                State::Working,
                format!("working, turn {turn}"),
                "open",
                false,
                open,
            ),
            Attention::Asks { question } => {
                (State::Asks, question.clone(), "answer", true, open)
            }
            Attention::ReadyToLand { changes } => (
                State::Met,
                format!("{} ready to land", count(*changes, "change")),
                "land",
                true,
                Act::Land(run.id.clone()),
            ),
            Attention::WouldConflict { files } => (
                State::Failing,
                format!("would conflict in {}", count(files.len(), "file")),
                "compare",
                true,
                match &run.origin {
                    Origin::Fork { from, .. } => Act::Open(Route::Compare {
                        main: from.clone(),
                        fork: run.id.clone(),
                    }),
                    _ => open,
                },
            ),
            Attention::ConflictsOnMain { files } => (
                State::Failing,
                format!("conflicts in {}", count(files.len(), "file")),
                "resolve",
                true,
                open,
            ),
            Attention::Queued(queued) if queued.needs_confirmation => (
                State::Asks,
                "queued to land, with conflicts to confirm".to_owned(),
                "confirm",
                true,
                open,
            ),
            Attention::Queued(queued) => (
                State::Idle,
                format!("queued to land, #{}", queued.position),
                "open",
                false,
                open,
            ),
            Attention::Interrupted => (
                State::Idle,
                "interrupted when heiko closed".to_owned(),
                "open",
                true,
                open,
            ),
            Attention::Failed => (
                State::Failing,
                match &run.status {
                    RunStatus::Finished(StopReason::Error(error)) => error
                        .lines()
                        .next()
                        .unwrap_or("stopped with an error")
                        .to_owned(),
                    _ => "stopped with an error".to_owned(),
                },
                "read",
                false,
                open,
            ),
            Attention::Landed | Attention::Dropped => return None,
            Attention::Idle => {
                let limit = matches!(
                    run.status,
                    RunStatus::Finished(StopReason::Limit(_))
                );
                match note.as_ref().map(|note| note.tone) {
                    Some(Tone::Good) => {
                        (State::Met, said("met its goal"), "read", false, open)
                    }
                    Some(Tone::Danger) => (
                        State::Limit,
                        said("stopped, goal not met"),
                        "read",
                        false,
                        open,
                    ),
                    _ if limit => (
                        State::Limit,
                        "stopped at its limit".to_owned(),
                        "read",
                        false,
                        open,
                    ),
                    _ if self.unread(run) > 0 => (
                        State::Idle,
                        "finished, your turn".to_owned(),
                        "read",
                        false,
                        open,
                    ),
                    _ => return None,
                }
            }
        };
        Some(Happened {
            when: if run.status.is_live() {
                "now".to_owned()
            } else {
                time(&run.started)
            },
            day: day(&run.started),
            sign: Sign::State(state),
            name: run.title.clone(),
            line,
            action: action.to_owned(),
            hot,
            act,
        })
    }

    /// Does what a thing Home lists offers.
    pub fn act_on(&mut self, act: Act, cx: &mut gpui::Context<Self>) {
        match act {
            Act::Open(route) => self.navigate(route, cx),
            Act::Land(run) => {
                self.navigate(Route::Run(run.clone()), cx);
                self.preview_landing(&run, cx);
            }
            Act::Agent(name) => self.open_agent(&name, cx),
            Act::Push(repo) => self.push(&repo, false, cx),
        }
    }
}

impl Workspace {
    /// `repo`'s forks that finished and are still open, newest first:
    /// those the host works out a landing forecast for.
    pub fn finished_forks(&self, repo: &str) -> Vec<heiko_agent::tool::RunId> {
        self.runs
            .iter()
            .filter(|run| {
                matches!(run.origin, crate::view::Origin::Fork { .. })
                    && !run.status.is_live()
                    && self.repo_of(run) == repo
                    && !self.is_closed(&run.id)
            })
            .map(|run| run.id.clone())
            .collect()
    }
}
