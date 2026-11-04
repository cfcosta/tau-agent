//! tau-agents' interface (ADR 0037): the Agents page and the New agent
//! form, what an agent's conversation shows around it (a banner, a
//! panel in the inspector, its events as cards), its settings, and its
//! commands in every composer.
//!
//! What it shows comes from the host as the [`Overview`]; what the
//! person asks goes back as an [`Act`]. Each drawn one is a view tree
//! (ADR 0045): the form's fields and choices are the interface's UI
//! state.

mod notices;
mod page;
mod pane;
mod run;

use std::sync::Arc;

use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_ui_kit::{assets::Icon, theme::Tone};
use tau_ui_plugin::{
    Handle,
    Link,
    ListedCommand,
    Manifest,
    Page,
    PluginStatus,
    RepoKind,
    SlashCommand,
    UiPlugin,
    ViewCx,
    points::{self, AtApp, AtRepo, AtRun},
    tree::{self, Node, draft_key},
};

use crate::{Act, Agent, COMMAND, NAME, Overview, Settings, Status};

/// The Agents page.
pub const OVERVIEW: &str = "agents";

/// The New agent form.
pub const NEW: &str = "new-agent";

/// The plugin whose data lists the person's MCP servers: the form reads
/// their names from it, as JSON, without depending on its crate.
const MCP: &str = "tau-mcp";

/// The person's MCP servers, by name, from tau-mcp's data: `servers`,
/// each with its `name`, and `enabled` unless it is off.
pub fn mcp_servers(data: Option<&Value>) -> Vec<String> {
    data.and_then(|data| data.get("servers"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|server| {
            server.get("enabled").and_then(Value::as_bool) != Some(false)
        })
        .filter_map(|server| server.get("name")?.as_str().map(str::to_owned))
        .collect()
}

/// tau-agents' interface half.
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentsUi;

/// The New agent form's inputs, by id: their drafts are its name and
/// what it should do.
pub const NAME_INPUT: &str = "new-agent-name";
pub const PURPOSE_INPUT: &str = "new-agent-purpose";
/// The fields of a secret to save from an agent's panel.
pub const SECRET_NAME_INPUT: &str = "agent-secret-name";
pub const SECRET_VALUE_INPUT: &str = "agent-secret-value";

/// The UI state keys of the form's choices: its model (none for the
/// default), cap, window and MCP servers; the agent the host made,
/// until its conversation opens; and why the host could not make it.
pub const MODEL: &str = "new-agent.model";
pub const CAP: &str = "new-agent.cap";
pub const WINDOW: &str = "new-agent.window";
pub const SERVERS: &str = "new-agent.servers";
pub const MAKING: &str = "new-agent.making";
pub const ERROR: &str = "new-agent.error";

/// The trees' actions. [`ACT`] carries out the [`Act`] its arguments
/// hold.
pub(crate) const ACT: &str = "act";
/// Opens the page its arguments name.
pub(crate) const GO: &str = "go";
/// Opens the run its arguments name: `{ "run": id }`.
pub(crate) const OPEN: &str = "open";
/// Opens the run's details, where a phone keeps the panel.
pub(crate) const DETAILS: &str = "details";
/// Sets a key of the UI state: `{ "key", "value" }`, a null value
/// forgetting it.
pub(crate) const SET: &str = "set";
/// Asks the host to make the agent the form describes.
pub(crate) const START: &str = "start";
/// Saves the secret the panel's fields hold: Enter in its value sends
/// `save-secret:<agent>`, the name coming from its field's draft.
pub(crate) const SAVE_SECRET: &str = "save-secret";
/// Saves the settings its arguments hold.
pub(crate) const SETTINGS: &str = "settings";
/// Answers a question with its field's draft: `{ "agent", "notice",
/// "answer" }`; Enter in the field sends `answer:<agent>/<notice>`.
pub(crate) const ANSWER: &str = "answer";

/// The input of the answer to `agent`'s question `notice`.
pub(crate) fn answer_input(agent: &str, notice: &str) -> String {
    format!("agent-answer-{agent}-{notice}")
}

/// A button that carries out `act`.
pub(crate) fn act_button(
    label: &str,
    kind: tree::Kind,
    act: &Act,
) -> tree::Node {
    tree::Node::Button {
        label: label.to_owned(),
        action: ACT.into(),
        args: serde_json::to_value(act).unwrap_or(Value::Null),
        kind,
        icon: None,
    }
}

/// A button that carries out `action` with `args`.
pub(crate) fn button(
    label: &str,
    kind: tree::Kind,
    action: &str,
    args: Value,
) -> tree::Node {
    tree::Node::Button {
        label: label.to_owned(),
        action: action.to_owned(),
        args,
        kind,
        icon: None,
    }
}

/// The New agent form as the UI state holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    /// The name typed; empty takes the one from the purpose.
    pub name: String,
    pub purpose: String,
    /// Its model; `None` is the default.
    pub model: Option<String>,
    pub daily_cap: f64,
    pub window_secs: u64,
    /// The person's MCP servers it starts with.
    pub servers: std::collections::BTreeSet<String>,
    /// The agent the host made, until its conversation opens.
    pub making: Option<String>,
    /// Why the host could not make it.
    pub error: Option<String>,
}

impl Form {
    /// The form in `view`'s UI state.
    pub fn of(view: &ViewCx<'_, AgentsUi>) -> Self {
        let defaults = Settings::default();
        let text = |key: &str| {
            view.ui_value(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Self {
            name: text(&draft_key(NAME_INPUT)).unwrap_or_default(),
            purpose: text(&draft_key(PURPOSE_INPUT)).unwrap_or_default(),
            model: text(MODEL),
            daily_cap: view
                .ui_value(CAP)
                .and_then(Value::as_f64)
                .unwrap_or(defaults.daily_cap),
            window_secs: view
                .ui_value(WINDOW)
                .and_then(Value::as_u64)
                .unwrap_or(defaults.window_secs),
            servers: view
                .ui_value(SERVERS)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|server| server.as_str().map(str::to_owned))
                .collect(),
            making: text(MAKING),
            error: text(ERROR),
        }
    }

    /// The name the purpose gives, for a form whose name is left empty.
    pub fn suggested(&self) -> String {
        if self.purpose.trim().is_empty() {
            String::new()
        } else {
            crate::shown::suggest_name(&self.purpose)
        }
    }

    /// The name it makes the agent with: the one typed, or else the
    /// purpose's.
    pub fn chosen_name(&self) -> String {
        match self.name.trim() {
            "" => self.suggested(),
            name => name.to_owned(),
        }
    }

    /// The form, as what the host is asked to make.
    pub fn act(&self) -> Result<Act, String> {
        let name = self.chosen_name();
        let purpose = self.purpose.trim().to_owned();
        if !crate::valid_name(&name) {
            return Err("A name is lower-case letters, digits and dashes, \
                        starting with a letter."
                .to_owned());
        }
        if purpose.is_empty() {
            return Err("Say what it should do.".to_owned());
        }
        Ok(Act::Create {
            name,
            purpose,
            model: self.model.clone(),
            daily_cap: Some(self.daily_cap),
            window_secs: Some(self.window_secs),
            servers: self.servers.iter().cloned().collect(),
        })
    }
}

/// Clears the form once its agent's conversation opened.
pub(crate) fn reset(handle: &Handle) {
    for key in [
        draft_key(NAME_INPUT),
        draft_key(PURPOSE_INPUT),
        MODEL.into(),
        CAP.into(),
        WINDOW.into(),
        SERVERS.into(),
        MAKING.into(),
        ERROR.into(),
    ] {
        handle.set_ui(key, None);
    }
}

/// What the trees' buttons and inputs do: the form and the panel's
/// secret name are read again from `view`'s UI state.
pub(crate) fn on_action(
    action: &str,
    args: &Value,
    view: &mut ViewCx<'_, AgentsUi>,
) {
    let handle = &view.handle;
    let text =
        |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    if let Some(at) = action.strip_prefix(&format!("{ANSWER}:")) {
        // Enter in a question's field: `<agent>/<notice>`, a name having
        // no slash.
        let answer = text("text").unwrap_or_default().trim().to_owned();
        if let Some((agent, notice)) = at.split_once('/')
            && !answer.is_empty()
        {
            handle.act(Act::Answer {
                agent: agent.to_owned(),
                notice: notice.to_owned(),
                answer,
            });
        }
        return;
    }
    // Enter in the secret's value: the agent comes with the action, the
    // name from its field, the value only from the input.
    if let Some(agent) = action.strip_prefix(&format!("{SAVE_SECRET}:")) {
        let name = view
            .ui_value(&draft_key(SECRET_NAME_INPUT))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let Some(value) = text("text") else { return };
        if name.is_empty() || value.is_empty() {
            return;
        }
        handle.act(Act::SetSecret {
            agent: agent.to_owned(),
            name,
            value,
        });
        handle.set_ui(draft_key(SECRET_NAME_INPUT), None);
        return;
    }
    match action {
        ACT => {
            if let Ok(act) = serde_json::from_value::<Act>(args.clone()) {
                handle.act(act);
            }
        }
        GO => {
            if let Some(page) = text("page") {
                handle.navigate(Link::page(page));
            }
        }
        OPEN => {
            if let Some(run) = text("run") {
                handle.open_run(&RunId(Arc::from(run.as_str())));
            }
        }
        DETAILS => handle.run_details(),
        SET => {
            if let Some(key) = text("key") {
                let value = args.get("value").filter(|value| !value.is_null());
                handle.set_ui(key, value.cloned());
            }
        }
        START => match Form::of(view).act() {
            Ok(act) => {
                view.handle.set_ui(ERROR, None);
                view.handle.act(act);
            }
            Err(error) => view.handle.set_ui(ERROR, Some(Value::String(error))),
        },
        SETTINGS => {
            if let Ok(settings) =
                serde_json::from_value::<Settings>(args.clone())
            {
                view.save_settings(&settings);
            }
        }
        ANSWER => {
            let (Some(agent), Some(notice), Some(answer)) =
                (text("agent"), text("notice"), text("answer"))
            else {
                return;
            };
            let answer = answer.trim().to_owned();
            if answer.is_empty() {
                return;
            }
            handle.set_ui(draft_key(&answer_input(&agent, &notice)), None);
            handle.act(Act::Answer {
                agent,
                notice,
                answer,
            });
        }
        _ => {}
    }
}

impl UiPlugin for AgentsUi {
    type State = ();
    type Data = Overview;
    type RepoData = ();
    type Settings = Settings;

    fn name(&self) -> &'static str {
        NAME
    }

    /// The host made the agent, or says why not.
    fn reply(&self, reply: Value, handle: &Handle) {
        if let Some(repo) = reply.get("repo").and_then(Value::as_str) {
            handle.set_ui(MAKING, Some(json!(repo)));
        } else if let Some(error) = reply.get("error").and_then(Value::as_str) {
            handle.set_ui(MAKING, None);
            handle.set_ui(ERROR, Some(json!(error)));
        }
    }

    /// Every button, switch and input of its trees.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        on_action(action, args, view);
    }

    fn read_prompt(&self, prompt: &str) -> Option<String> {
        prompt
            .strip_prefix(&format!("/{} ", crate::SKILL))
            .map(|purpose| purpose.trim().to_owned())
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(
                Page::new(OVERVIEW, page::overview)
                    .framed()
                    .title(|_| "Agents".into()),
            )
            .page(
                Page::new(NEW, page::new_agent)
                    .framed()
                    .title(|_| "New agent".into()),
            )
            .settings(pane::render)
            .command(
                SlashCommand::new(
                    COMMAND,
                    "Make an agent that keeps going on its own",
                    new_agent,
                )
                .args("<what it should do>")
                .icon(Icon::SubAgent),
            )
            .listed_commands(|overview, _| commands(overview), fire)
            .contribute(points::STATUS, |at: &AtRun, view| {
                let agent = view
                    .data
                    .agent_of_main(&at.run.repo, &at.run.id.to_string())?;
                Some(PluginStatus {
                    name: NAME.into(),
                    state: agent.status.label().into(),
                    tone: tone(&agent.status),
                })
            })
            .contribute(points::REPO_KIND, |_: &AtApp, _| {
                Some(RepoKind {
                    kind: KIND.into(),
                    title: "Agents".into(),
                    page: Link::page(OVERVIEW),
                    new: Some(Link::page(NEW)),
                    empty: "None yet. /agent and what it should do makes one."
                        .into(),
                })
            })
            .contribute(points::KIND_ROW_END, |at: &AtRepo, view| {
                row_end(view.data.agent(&at.repo)?)
            })
            .contribute(points::KIND_ROW_LINE, |at: &AtRepo, view| {
                let last = view.data.agent(&at.repo)?.last.as_ref()?;
                Some(Node::row([
                    Node::text(last.line.clone()),
                    Node::Spacer,
                    Node::Ago {
                        id: format!("last-{}", at.repo),
                        at: last.at * 1000,
                    },
                ]))
            })
            .contribute(points::WAITING, |_: &AtApp, view| {
                Some(crate::alert::waiting(view.data))
            })
            .contribute(points::RUN_BANNER, run::banner)
            .contribute_at(points::INSPECTOR, -200, run::panel)
            .contribute(points::USER_MESSAGE, run::event_card)
    }
}

/// The kind an agent's repository has in the catalog (ADR 0037).
const KIND: &str = "agent";

/// Where an agent stands, on its row's end: what it left the person
/// first, then whether it works or is paused, else when it wakes next.
fn row_end(agent: &Agent) -> Option<Node> {
    let pill = |text: String, tone| Node::Pill { text, tone };
    let waits = agent.waits_on_you();
    Some(match agent.status {
        _ if waits > 0 => pill(format!("{waits} need you"), tree::Tone::Info),
        Status::Working => pill("working".into(), tree::Tone::Good),
        Status::Paused => pill("paused".into(), tree::Tone::Warn),
        _ => Node::Due {
            id: format!("next-{}", agent.name),
            at: agent.next()? * 1000,
        },
    })
}

/// `/agent <purpose>`: the New agent form, filled in from what follows.
fn new_agent(args: &str, view: &mut ViewCx<'_, AgentsUi>) {
    let purpose = args.trim().to_owned();
    view.handle.set_ui(ERROR, None);
    view.handle.set_ui(
        draft_key(PURPOSE_INPUT),
        (!purpose.is_empty()).then(|| json!(purpose)),
    );
    view.handle.navigate(Link::page(NEW));
}

/// Every agent's command triggers, as composer commands.
pub fn commands(overview: &Overview) -> Vec<ListedCommand> {
    overview
        .agents
        .iter()
        .flat_map(|agent| {
            agent.commands().map(|(command, hint)| ListedCommand {
                name: command.to_owned(),
                hint: format!("Tell {}", agent.name),
                args: hint.unwrap_or_default().to_owned(),
                icon: Icon::SubAgent,
            })
        })
        .collect()
}

/// `/command args`: the event goes to the agent whose command it is,
/// and nothing starts where it was typed.
fn fire(
    command: &str,
    args: &str,
    _repo: Option<&str>,
    view: &mut ViewCx<'_, AgentsUi>,
) {
    let Some(agent) = view
        .data
        .agents
        .iter()
        .find(|agent| agent.commands().any(|(name, _)| name == command))
    else {
        return;
    };
    let act = Act::Command {
        agent: agent.name.clone(),
        command: command.to_owned(),
        args: args.to_owned(),
        from: view.run.map(|run| run.id.to_string()),
    };
    let sent = format!("/{command} sent to {}", agent.name);
    view.handle.act(act);
    view.handle.note(sent);
}

/// Opens `agent`'s conversation: its main chat, or, before the host
/// said which that is, the first run in its repository.
pub(crate) fn open(agent: &Agent, view: &mut ViewCx<'_, AgentsUi>) {
    let run = agent
        .main
        .as_deref()
        .map(|main| RunId(Arc::from(main)))
        .or_else(|| conversation_of(&agent.name, view));
    if let Some(run) = run {
        view.handle.open_run(&run);
    }
}

/// The first run of the repository `name` the window knows.
pub(crate) fn conversation_of(
    name: &str,
    view: &ViewCx<'_, AgentsUi>,
) -> Option<RunId> {
    view.runs()
        .into_iter()
        .map(|(run, _)| run)
        .find(|run| run.repo == name)
        .map(|run| run.id)
}

/// The tone of an agent's state.
pub(crate) fn tone(status: &Status) -> Tone {
    match status {
        Status::Working => Tone::Info,
        Status::Waiting => Tone::Quiet,
        Status::Paused => Tone::Warn,
        Status::AtCap => Tone::Warn,
        Status::Broken { .. } => Tone::Danger,
    }
}

/// The tone of an agent's state, as a tree's pieces read it.
pub(crate) fn tree_tone(status: &Status) -> tree::Tone {
    match status {
        Status::Working => tree::Tone::Info,
        Status::Waiting => tree::Tone::Neutral,
        Status::Paused | Status::AtCap => tree::Tone::Warn,
        Status::Broken { .. } => tree::Tone::Bad,
    }
}

/// An agent's state on a pill: `working`, `at its cap`.
pub(crate) fn status_pill(status: &Status) -> tree::Node {
    tree::Node::Pill {
        text: status.label().to_owned(),
        tone: tree_tone(status),
    }
}
