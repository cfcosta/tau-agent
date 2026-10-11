//! tau-agents: agents, each a conversation that never ends in a
//! repository of its own, woken by the events it declares
//! ([ADR 0037](../../../docs/decisions/0037-agents.md)). This is its
//! interface half: what every interface shows of an agent. Loading an
//! agent's `agent.luau`, firing its triggers and starting its turns is
//! `tau-agents-host`'s (ADR 0030).
//!
//! - [`Overview`], [`Agent`], [`Trigger`], [`Version`]: the agents as
//!   the host last read them, for every interface.
//! - [`Settings`]: what the person set for an agent, outside its
//!   repository: its model and the most it may spend a day.
//! - [`Act`]: what the interface asks the host.
//! - [`event`]: the message an agent's events reach it as.

pub mod alert;
pub mod event;
pub mod repository;
pub mod shown;
pub mod ui;

/// The directory under tau's data directory where agents live, one
/// repository each.
pub use repository::ROOT;
use serde::{Deserialize, Serialize};
/// One case of an agent's tests, as a Luau plugin's come out.
pub use tau_luau_plugins::TestResult;
pub use ui::AgentsUi;

/// The host plugin's name, for both its halves.
pub const NAME: &str = "tau-agents";

/// The skill tau ships for an agent writing itself.
pub const SKILL: &str = "tau-agents";

/// The composer command that makes a new agent: `/agent <purpose>`.
pub const COMMAND: &str = "agent";

/// The file at an agent's root that declares what wakes it.
pub const ENTRY: &str = "agent.luau";

/// Whether `name` can name an agent, and so its repository: lower-case
/// letters, digits and dashes, starting with a letter, at most 40.
pub fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The model a new agent starts on: the plan's newest `luna`, its small
/// and cheap family, or the plan's first model when it has none.
pub fn default_model() -> String {
    let plan = tau_ai::model::plan_models();
    plan.iter()
        .find(|model| {
            tau_ai::model::family_version(&model.id)
                .is_some_and(|(family, _)| family == "luna")
        })
        .or(plan.first())
        .map_or_else(|| "gpt-6-luna".to_owned(), |model| model.id.clone())
}

/// The effort a new agent starts at.
pub const DEFAULT_EFFORT: &str = "low";

/// What wakes an agent, as its `agent.luau` declares it, with when it
/// last fired and when it fires next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    /// Its key in `triggers`: what events from it are named by.
    pub name: String,
    pub kind: TriggerKind,
    /// When it last fired, in seconds since the epoch.
    #[serde(default)]
    pub last: Option<i64>,
    /// When it fires next, for a schedule or a poll.
    #[serde(default)]
    pub next: Option<i64>,
    /// Why it did not fire when it last tried: a poll whose request or
    /// whose `key` failed, a stream that lost its connection or whose
    /// `parse` failed.
    #[serde(default)]
    pub error: Option<String>,
    /// For a stream, whether its connection is open now.
    #[serde(default)]
    pub live: Option<bool>,
}

/// What kind of thing wakes an agent (ADR 0037's table). Step 1 fires
/// schedules and commands; the others are declared now so what the
/// interface reads keeps its shape as they come.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerKind {
    /// Every interval (`every = "15m"`), or at a local time
    /// (`at = "09:00"`) on some days of the week.
    Schedule {
        #[serde(default)]
        every: Option<String>,
        #[serde(default)]
        at: Option<String>,
        /// `Mon` to `Sun`; empty is every day.
        #[serde(default)]
        days: Vec<String>,
    },
    /// `/<command> args` in any composer.
    Command {
        command: String,
        #[serde(default)]
        hint: Option<String>,
    },
    /// A tool or an HTTP request, every interval, once per new item.
    /// `source` names it for a person; `tool` and `args`, or `http`,
    /// are what the host calls.
    Poll {
        source: String,
        every: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        /// The request, its secrets as markers (`{ "$secret": name }`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http: Option<serde_json::Value>,
    },
    /// A stream it keeps open: server-sent events or a websocket. Its
    /// `parse` turns each frame into an event, or into nothing.
    Stream {
        url: String,
        #[serde(default)]
        via: StreamVia,
        /// Sent as it opens, its secrets as markers
        /// (`{ "$secret": name }`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers: Option<serde_json::Value>,
    },
    /// An MCP server's resource, through `resources/subscribe`.
    Mcp { server: String, uri: String },
    /// Another agent's messages.
    Message,
}

/// What a stream trigger reads.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum StreamVia {
    /// Server-sent events (`text/event-stream`), over HTTP.
    #[default]
    Sse,
    /// A websocket's messages.
    Websocket,
}

/// A version of an agent's `agent.luau`: a commit of its trunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub commit: String,
    /// The commit's first line.
    pub summary: String,
    pub tests: Vec<TestResult>,
    /// Whether it is the version whose triggers fire.
    pub live: bool,
    /// Why it never went live: it did not load, or its tests failed.
    #[serde(default)]
    pub error: Option<String>,
}

/// Where an agent stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    /// Waiting for what wakes it.
    Waiting,
    /// A turn of its conversation is going.
    Working,
    /// The person paused it: events queue, no turn starts.
    Paused,
    /// It spent its day's cap: events queue until tomorrow, or until the
    /// person allows more.
    AtCap,
    /// No version of its `agent.luau` loads: nothing wakes it.
    Broken { error: String },
}

/// The latest thing that happened to an agent, for its sidebar row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Last {
    /// When, in seconds since the epoch.
    pub at: i64,
    pub line: String,
}

/// An agent, as the Agents page and its sidebar row show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    /// Its name, which is its repository's.
    pub name: String,
    /// What it is for, from its first message or its `AGENTS.md`.
    pub purpose: String,
    pub status: Status,
    /// What wakes it, as its live version declares.
    pub triggers: Vec<Trigger>,
    /// Its latest versions, newest first.
    pub versions: Vec<Version>,
    /// What it spent today, in dollars.
    pub spend_today: f64,
    /// What it may spend today: its cap, and what the person allowed on
    /// top of it.
    pub cap_today: f64,
    /// Events waiting for its next turn.
    pub queued: usize,
    #[serde(default)]
    pub last: Option<Last>,
    pub model: String,
    pub effort: String,
    /// The id of its main chat, which is the agent's conversation, once
    /// made: what its row and its card open.
    #[serde(default)]
    pub main: Option<String>,
    /// The secrets its live version's triggers name, and whether tau's
    /// secret store holds each: its page asks the person for the rest.
    #[serde(default)]
    pub secrets: Vec<SecretState>,
    /// Why its commands cannot run, when they cannot: its command tools
    /// are off without a sandbox to run in.
    #[serde(default)]
    pub sandbox: Option<String>,
    /// What it left for the person and they have not dealt with yet:
    /// its notes and its questions, oldest first.
    #[serde(default)]
    pub notices: Vec<Notice>,
}

/// Something an agent left for the person (ADR 0037): a note
/// (`notify`), kept until they dismiss it, or a question (`ask`), kept
/// until they answer it. The answer reaches the agent as an event
/// ([`event::Kind::Answer`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    /// Its own among the agent's: what an answer or a dismissal names.
    pub id: String,
    /// When it was left, in seconds since the epoch.
    pub at: i64,
    pub text: String,
    /// For a question, the answers it offers, none meaning any; for a
    /// note, nothing.
    #[serde(default)]
    pub choices: Option<Vec<String>>,
}

impl Notice {
    /// Whether it is a question, waiting for an answer.
    pub fn asks(&self) -> bool {
        self.choices.is_some()
    }
}

/// A secret an agent names, by name only: its value never leaves tau's
/// secret store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretState {
    pub name: String,
    /// The store holds a value for it.
    pub set: bool,
}

/// The agents as the host last read them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    /// By name.
    pub agents: Vec<Agent>,
}

impl Overview {
    pub fn agent(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    /// The agent whose conversation the run `run` of the repository
    /// `repo` is: its main chat, not a chat or sub-agent beside it.
    pub fn agent_of_main(&self, repo: &str, run: &str) -> Option<&Agent> {
        self.agent(repo)
            .filter(|agent| agent.main.as_deref() == Some(run))
    }
}

/// What the person set for an agent: tau-agents' settings, each agent's
/// a repository's copy (ADR 0029), kept in tau's data and not in the
/// agent's repository, so the agent cannot change them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Its model; `None` is [`default_model`].
    pub model: Option<String>,
    /// Its effort; `None` is [`DEFAULT_EFFORT`].
    pub effort: Option<String>,
    /// The most it may spend a day, in dollars.
    pub daily_cap: f64,
    /// How long an event waits for others to come with it, in seconds.
    pub window_secs: u64,
    /// What the person allowed on top of the cap, for
    /// [`Self::extra_day`] only.
    pub extra_today: f64,
    /// The day, `YYYY-MM-DD` in the host's local time, that
    /// [`Self::extra_today`] is for.
    pub extra_day: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: None,
            effort: None,
            daily_cap: 1.0,
            window_secs: 30,
            extra_today: 0.0,
            extra_day: None,
        }
    }
}

impl Settings {
    /// The model it runs on.
    pub fn model(&self) -> String {
        self.model.clone().unwrap_or_else(default_model)
    }

    /// The effort it runs at.
    pub fn effort(&self) -> &str {
        self.effort.as_deref().unwrap_or(DEFAULT_EFFORT)
    }

    /// What it may spend on `day` (`YYYY-MM-DD`): its cap, and what the
    /// person allowed on top for that day.
    pub fn cap_on(&self, day: &str) -> f64 {
        let extra = if self.extra_day.as_deref() == Some(day) {
            self.extra_today
        } else {
            0.0
        };
        self.daily_cap + extra
    }
}

/// What the interface asks the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// Makes a new agent, `name`, whose first message is `purpose`.
    /// What is not given takes [`Settings`]' defaults. `servers` are the
    /// person's MCP servers it starts with.
    Create {
        name: String,
        purpose: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        daily_cap: Option<f64>,
        #[serde(default)]
        window_secs: Option<u64>,
        #[serde(default)]
        servers: Vec<String>,
    },
    /// Lets `agent` spend `amount` more today.
    AllowMore { agent: String, amount: f64 },
    /// Stops starting turns for `agent`; its events queue.
    Pause { agent: String },
    /// Starts turns for `agent` again.
    Resume { agent: String },
    /// Saves `value` as the secret `name` in `agent`'s own secret store,
    /// for its requests alone. Its value is never shown back.
    SetSecret {
        agent: String,
        name: String,
        value: String,
    },
    /// Forgets `agent`'s secret `name`.
    RemoveSecret { agent: String, name: String },
    /// The person answered `agent`'s question `notice`: it hears the
    /// answer as an event, and the question goes.
    Answer {
        agent: String,
        notice: String,
        answer: String,
    },
    /// The person read `agent`'s note `notice`: it goes.
    Dismiss { agent: String, notice: String },
    /// The person sent `/command args` in a composer: fires `agent`'s
    /// command trigger of that name. `from` is the run it was typed in,
    /// if any.
    Command {
        agent: String,
        command: String,
        #[serde(default)]
        args: String,
        #[serde(default)]
        from: Option<String>,
    },
}
