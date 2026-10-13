//! tau-schedule's host half (ADR 0039, ADR 0030): what chats scheduled,
//! kept on the host in one file, and the turns that wake them.
//!
//! Every second, each chat with a wake-up due takes a turn of tau's on
//! the notes of all that came due ([`crate::message`]), unless a turn of
//! it is going: then they wait for it to end. A chat that landed or was
//! dropped takes no turn, and what it had waiting goes. A wake-up leaves
//! the file once its turn started, so tau closed before then wakes the
//! chat when it starts again.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    plugin::Plugin,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ui_plugin::{
    Conversations,
    HostCx,
    HostHalf,
    PluginHost,
    PluginInfo,
    RunCtx,
    RunKind,
    Waking,
};
use tokio::sync::Mutex;

use crate::{
    Act,
    MAX_WAITING,
    NAME,
    Overview,
    ScheduleUi,
    Wake,
    Woken,
    clock::{self, Clock, SystemClock},
    message,
};

/// How often what came due is looked at.
const TICK: Duration = Duration::from_secs(1);

/// The soonest a wake-up may be, from now.
const SOONEST: i64 = 60;

/// The furthest a wake-up may be, from now.
const FURTHEST: i64 = 366 * 86_400;

/// How long a turn that did not start waits to be tried again.
const RETRY: i64 = 30;

/// What the host half reaches of the host: its conversations, and the
/// interface's copy of what chats have waiting.
pub trait Env: Send + Sync {
    /// The host's conversations, once it has them.
    fn conversations(&self) -> Option<Arc<dyn Conversations>>;

    /// Has the interface read [`Schedules::overview`] again.
    fn refresh(&self);
}

/// [`Env`] on a real host.
struct HostEnv(HostCx);

impl Env for HostEnv {
    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        self.0.conversations()
    }

    fn refresh(&self) {
        self.0.refresh();
    }
}

/// What a chat asks to schedule: a name, a note, and `in` or `on`.
/// `every`, `at` and `days` are read only to say why they are refused.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub name: String,
    pub note: String,
    /// Once, after this long: `"2h"`.
    #[serde(default, rename = "in")]
    pub after: Option<String>,
    /// Once, at this local date and time: `"2026-10-13 09:00"`.
    #[serde(default)]
    pub on: Option<String>,
    #[serde(default)]
    pub every: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub days: Vec<String>,
}

/// What [`Schedules::schedule`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub wake: Wake,
    /// It took the place of one of the same name.
    pub replaced: bool,
}

/// Why repeating is refused, in words for the model.
pub const NO_REPEATS: &str = "A chat wakes itself once: repeating is for \
    agents. Schedule the next time when this one comes, or make an agent \
    (/agent) for work that should keep going on its own.";

/// Whether `name` can name a wake-up: lower-case letters, digits, `-`
/// and `_`, starting with a letter, at most 40.
fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
        })
}

/// tau-schedule on the host. Clones share it.
#[derive(Clone)]
pub struct Schedules(Arc<Inner>);

struct Inner {
    /// Where what chats have waiting is kept.
    path: PathBuf,
    /// Where agents' repositories are: their runs have their own tool.
    agents: PathBuf,
    env: Arc<dyn Env>,
    clock: Arc<dyn Clock>,
    /// By chat, then by name.
    wakes: Mutex<BTreeMap<String, BTreeMap<String, Wake>>>,
    /// When to try again a chat whose turn did not start.
    retry: Mutex<BTreeMap<String, i64>>,
}

impl PluginHost for Schedules {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let schedules = Self::open(
            cx.plugin_dir(NAME).join("wakes.json"),
            cx.dir.join(tau_agents::ROOT),
            Arc::new(HostEnv(cx.clone())),
            Arc::new(SystemClock),
        )
        .await;
        let ticker = schedules.clone();
        cx.runtime.spawn(async move {
            loop {
                ticker.tick().await;
                tokio::time::sleep(TICK).await;
            }
        });
        Ok(schedules)
    }
}

impl Schedules {
    /// What chats scheduled, kept at `path`; agents' repositories are
    /// under `agents`. Nothing wakes until [`Self::tick`] is called.
    pub async fn open(
        path: PathBuf,
        agents: PathBuf,
        env: Arc<dyn Env>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let read = path.clone();
        let wakes = tokio::task::spawn_blocking(move || load(&read))
            .await
            .unwrap_or_default();
        Self(Arc::new(Inner {
            path,
            agents,
            env,
            clock,
            wakes: Mutex::new(wakes),
            retry: Mutex::default(),
        }))
    }

    /// What time it is, by its clock.
    pub fn now(&self) -> i64 {
        self.0.clock.now()
    }

    /// `at` as the host's local date and time.
    pub fn local_time(&self, at: i64) -> String {
        clock::local(self.0.clock.as_ref(), at)
    }

    /// Whether a run in the repository checked out at `checkout` is an
    /// agent's, which schedules with its own tool.
    fn is_agent(&self, checkout: &Path) -> bool {
        checkout.parent() == Some(self.0.agents.as_path())
    }

    /// Schedules `plan` for the chat `run`, in place of one of the same
    /// name. Why not, in words for the model.
    pub async fn schedule(
        &self,
        run: &str,
        plan: Plan,
    ) -> Result<Made, String> {
        let now = self.now();
        let name = plan.name.trim().to_owned();
        if !valid_name(&name) {
            return Err(format!(
                "`{name}` cannot name a wake-up: lower-case letters, digits, \
                 `-` and `_`, starting with a letter, at most 40"
            ));
        }
        let note = plan.note.trim().to_owned();
        if note.is_empty() {
            return Err("Leave yourself a note: what to do then".into());
        }
        if plan.every.is_some() || plan.at.is_some() || !plan.days.is_empty() {
            return Err(NO_REPEATS.into());
        }
        let clock = self.0.clock.as_ref();
        let at = match (plan.after, plan.on) {
            (Some(after), None) => {
                now + clock::duration(&after).ok_or_else(|| {
                    format!("`{after}` is not a duration, like \"2h\"")
                })?
            }
            (None, Some(on)) => {
                clock::parse_local(clock, &on).ok_or_else(|| {
                    format!(
                        "`{on}` is not a local date and time, like \"{}\"",
                        clock::local(clock, now + 86_400)
                    )
                })?
            }
            _ => {
                return Err("Say when with one of `in` (\"2h\") or `on` \
                            (a local date and time)"
                    .into());
            }
        };
        if at < now + SOONEST {
            return Err(format!(
                "That is {}: a minute from now is the soonest",
                clock::local(clock, at)
            ));
        }
        if at > now + FURTHEST {
            return Err("A year ahead is the furthest".into());
        }
        let wake = Wake {
            run: run.to_owned(),
            name: name.clone(),
            note,
            at,
            made: now,
        };
        let made = self
            .change(|wakes| {
                let waiting = wakes.entry(run.to_owned()).or_default();
                let replaced = waiting.contains_key(&name);
                if !replaced && waiting.len() >= MAX_WAITING {
                    return Err(format!(
                        "This chat has {MAX_WAITING} waiting, the most it \
                         may: cancel some first"
                    ));
                }
                waiting.insert(name, wake.clone());
                Ok(Made { wake, replaced })
            })
            .await?;
        self.0.env.refresh();
        Ok(made)
    }

    /// Cancels what the chat `run` scheduled as `name`: whether there
    /// was one.
    pub async fn unschedule(&self, run: &str, name: &str) -> bool {
        let gone = self
            .change(|wakes| {
                let waiting = wakes.get_mut(run)?;
                let gone = waiting.remove(name.trim());
                if waiting.is_empty() {
                    wakes.remove(run);
                }
                gone
            })
            .await
            .is_some();
        if gone {
            self.0.env.refresh();
        }
        gone
    }

    /// What the chat `run` has waiting, soonest first.
    pub async fn waiting(&self, run: &str) -> Vec<Wake> {
        let mut waiting: Vec<Wake> = self
            .0
            .wakes
            .lock()
            .await
            .get(run)
            .map(|waiting| waiting.values().cloned().collect())
            .unwrap_or_default();
        waiting.sort_by(|a, b| (a.at, &a.name).cmp(&(b.at, &b.name)));
        waiting
    }

    /// What every chat has waiting, soonest first.
    pub async fn overview(&self) -> Overview {
        let mut wakes: Vec<Wake> = self
            .0
            .wakes
            .lock()
            .await
            .values()
            .flat_map(|waiting| waiting.values().cloned())
            .collect();
        wakes.sort_by(|a, b| {
            (a.at, &a.run, &a.name).cmp(&(b.at, &b.run, &b.name))
        });
        Overview { wakes }
    }

    /// Wakes each chat that has something due and takes a turn now.
    pub async fn tick(&self) {
        let now = self.now();
        let due: Vec<(String, Vec<Wake>)> = {
            let wakes = self.0.wakes.lock().await;
            let retry = self.0.retry.lock().await;
            wakes
                .iter()
                .filter(|(run, _)| retry.get(*run).is_none_or(|at| now >= *at))
                .map(|(run, waiting)| {
                    let mut due: Vec<Wake> = waiting
                        .values()
                        .filter(|wake| wake.at <= now)
                        .cloned()
                        .collect();
                    due.sort_by(|a, b| (a.at, &a.name).cmp(&(b.at, &b.name)));
                    (run.clone(), due)
                })
                .filter(|(_, due)| !due.is_empty())
                .collect()
        };
        if due.is_empty() {
            return;
        }
        let Some(conversations) = self.0.env.conversations() else {
            return;
        };
        let mut changed = false;
        for (run, due) in due {
            let woken: Vec<Woken> = due
                .iter()
                .map(|wake| Woken {
                    name: wake.name.clone(),
                    due: self.local_time(wake.at),
                    note: wake.note.clone(),
                })
                .collect();
            match conversations.wake(&run, message(&woken)).await {
                Ok(Waking::Started) => {
                    self.0.retry.lock().await.remove(&run);
                    self.change(|wakes| {
                        if let Some(waiting) = wakes.get_mut(&run) {
                            for wake in &due {
                                // One scheduled again meanwhile stays.
                                if waiting.get(&wake.name) == Some(wake) {
                                    waiting.remove(&wake.name);
                                }
                            }
                            if waiting.is_empty() {
                                wakes.remove(&run);
                            }
                        }
                    })
                    .await;
                    changed = true;
                }
                Ok(Waking::Busy) => {}
                Ok(Waking::Gone) => {
                    self.0.retry.lock().await.remove(&run);
                    self.change(|wakes| wakes.remove(&run)).await;
                    changed = true;
                }
                Err(error) => {
                    eprintln!("{NAME}: could not wake {run}: {error:#}");
                    self.0.retry.lock().await.insert(run, now + RETRY);
                }
            }
        }
        if changed {
            self.0.env.refresh();
        }
    }

    /// Runs `change` on what chats have waiting, and keeps what it
    /// leaves.
    async fn change<T>(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, BTreeMap<String, Wake>>) -> T,
    ) -> T {
        let mut wakes = self.0.wakes.lock().await;
        let before = wakes.clone();
        let answer = change(&mut wakes);
        if *wakes != before {
            let (path, text) = (
                self.0.path.clone(),
                serde_json::to_string(&*wakes).expect("plain data"),
            );
            let written =
                tokio::task::spawn_blocking(move || write_atomic(&path, &text))
                    .await;
            if let Err(error) = written
                .map_err(anyhow::Error::from)
                .and_then(|written| written.map_err(anyhow::Error::from))
            {
                eprintln!(
                    "{NAME}: cannot keep what chats scheduled: {error:#}"
                );
            }
        }
        answer
    }
}

/// What is kept at `path`; nothing yet is the default.
fn load(path: &Path) -> BTreeMap<String, BTreeMap<String, Wake>> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Writes `text` to `path` whole or not at all. It blocks: call it from
/// `spawn_blocking`.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(partial, path)
}

/// tau-schedule's half on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScheduleHost;

impl HostHalf for ScheduleHost {
    type Plugin = ScheduleUi;
    type Host = Schedules;

    /// `schedule`, `scheduled` and `unschedule` in every run of a
    /// repository, so main, its chats and its sub-agents share a prompt
    /// prefix; a sub-agent's `schedule` says why it cannot. An agent's
    /// runs have their own.
    async fn agent_plugins(
        &self,
        host: &Schedules,
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        if host.is_agent(&run.repo.checkout) {
            return Ok(Vec::new());
        }
        Ok(vec![Box::new(ChatTools {
            schedules: host.clone(),
            sub_agent: run.kind == RunKind::SubAgent,
        })])
    }

    async fn catalog(
        &self,
        host: &Schedules,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        let waiting = host.overview().await.wakes.len();
        let description = match waiting {
            0 => "Chats wake themselves later, with a note".to_owned(),
            1 => {
                "Chats wake themselves later, with a note: 1 waiting".to_owned()
            }
            n => {
                format!("Chats wake themselves later, with a note: {n} waiting")
            }
        };
        PluginInfo {
            description,
            ..Default::default()
        }
    }

    async fn data(&self, host: &Schedules, _cx: &HostCx) -> Overview {
        host.overview().await
    }

    /// The person cancelled a wake-up.
    async fn act(
        &self,
        host: &Schedules,
        action: Value,
        _cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        match serde_json::from_value::<Act>(action)? {
            Act::Cancel { run, name } => anyhow::ensure!(
                host.unschedule(&run, &name).await,
                "Nothing is scheduled as {name} in this chat"
            ),
        }
        Ok(None)
    }
}

/// A chat's tools: `schedule`, `scheduled` and `unschedule`. Each reads
/// which chat it is from its call.
pub struct ChatTools {
    pub schedules: Schedules,
    /// A sub-agent's: it ends and lands, so nothing would wake it.
    pub sub_agent: bool,
}

/// Why a sub-agent's `schedule` refuses, in words for the model.
pub const SUB_AGENTS: &str = "A sub-agent ends and lands, so nothing would \
    wake it: say in your answer what should be looked at later, and when, \
    for the chat that started you to schedule.";

#[async_trait]
impl Plugin for ChatTools {
    fn name(&self) -> &str {
        NAME
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let schedules = &self.schedules;
        vec![
            Arc::new(Schedule {
                schedules: schedules.clone(),
                sub_agent: self.sub_agent,
            }),
            Arc::new(Listed(schedules.clone())),
            Arc::new(Unschedule(schedules.clone())),
        ]
    }
}

/// A tool's parameters, as JSON Schema, made once.
fn schema(
    cell: &'static OnceLock<Value>,
    make: fn() -> Value,
) -> &'static Value {
    cell.get_or_init(make)
}

/// `schedule { name, note, in | on }`.
struct Schedule {
    schedules: Schedules,
    sub_agent: bool,
}

#[async_trait]
impl AgentTool for Schedule {
    fn name(&self) -> &str {
        "schedule"
    }

    fn description(&self) -> &str {
        "Wake this chat later, once, with a note to yourself for then: `in` \
         a while, or `on` a local date and time. When it comes, this chat \
         takes a turn whose message is your note, after any turn going \
         ends. Use it when work waits on something: a review, a release, a \
         long build. Scheduling a name again replaces it. The person sees \
         what you scheduled beside the chat and may cancel it."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Lower-case letters, digits, `-` \
                                        and `_`: `recheck-pr-412`.",
                    },
                    "note": {
                        "type": "string",
                        "description": "What to do then, for you to read \
                                        when it comes.",
                    },
                    "in": {
                        "type": "string",
                        "description": "After this long: `90m`, `2h`, `3d`.",
                    },
                    "on": {
                        "type": "string",
                        "description": "At this local date and time: \
                                        `2026-10-13 09:00`.",
                    },
                },
                "required": ["name", "note"],
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        if self.sub_agent {
            return Err(ToolError::Message(SUB_AGENTS.into()));
        }
        let plan: Plan = serde_json::from_value(args)?;
        let made = self
            .schedules
            .schedule(&ctx.run.0, plan)
            .await
            .map_err(ToolError::Message)?;
        Ok(ToolOutput::text(format!(
            "{} `{}` for {}. This chat wakes then with your note.",
            if made.replaced {
                "Replaced"
            } else {
                "Scheduled"
            },
            made.wake.name,
            self.schedules.local_time(made.wake.at),
        )))
    }
}

/// `scheduled {}`.
struct Listed(Schedules);

#[async_trait]
impl AgentTool for Listed {
    fn name(&self) -> &str {
        "scheduled"
    }

    fn description(&self) -> &str {
        "What this chat has scheduled with `schedule` that is still to come, \
         soonest first, and the host's local time now. Look before you \
         schedule something again."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        _args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let waiting = self.0.waiting(&ctx.run.0).await;
        let now = self.0.local_time(self.0.now());
        if waiting.is_empty() {
            return Ok(ToolOutput::text(format!(
                "It is {now}. Nothing is scheduled."
            )));
        }
        let mut lines = vec![format!(
            "It is {now}. {} of the {MAX_WAITING} this chat may have:",
            waiting.len()
        )];
        lines.extend(waiting.iter().map(|wake| {
            format!(
                "- {} · {} · {}",
                wake.name,
                self.0.local_time(wake.at),
                wake.note.lines().next().unwrap_or_default(),
            )
        }));
        Ok(ToolOutput::text(lines.join("\n")))
    }
}

/// `unschedule { name }`.
struct Unschedule(Schedules);

#[derive(Deserialize)]
struct UnscheduleArgs {
    name: String,
}

#[async_trait]
impl AgentTool for Unschedule {
    fn name(&self) -> &str {
        "unschedule"
    }

    fn description(&self) -> &str {
        "Cancel what this chat scheduled as `name`."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: UnscheduleArgs = serde_json::from_value(args)?;
        Ok(ToolOutput::text(
            if self.0.unschedule(&ctx.run.0, &args.name).await {
                format!("Cancelled `{}`.", args.name)
            } else {
                format!("Nothing is scheduled as `{}`.", args.name)
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_short_and_plain() {
        assert!(valid_name("recheck-pr-412"));
        assert!(valid_name("nightly_build"));
        assert!(!valid_name(""));
        assert!(!valid_name("412"));
        assert!(!valid_name("Recheck"));
        assert!(!valid_name(&"a".repeat(41)));
    }
}
