//! tau-agents' interface (ADR 0037): the Agents page and the New agent
//! form, what an agent's conversation shows around it (a banner, a
//! panel in the inspector, its events as cards), its settings, and its
//! commands in every composer.
//!
//! What it shows comes from the host as the [`Overview`]; what the
//! person asks goes back as an [`Act`].

mod notices;
mod page;
mod pane;
mod run;

use std::sync::Arc;

use gpui::{
    AppContext as _,
    Context,
    Div,
    Entity,
    ParentElement,
    Styled,
    div,
    rems,
};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_ui_kit::{
    assets::Icon,
    input::TextInput,
    theme::{Design as _, Theme, Tone, Type, radius, weight},
};
use tau_ui_plugin::{
    Handle,
    Link,
    ListedCommand,
    Manifest,
    Page,
    PluginStatus,
    PluginUi,
    SlashCommand,
    UiPlugin,
    ViewCx,
    points::{self, AtRun},
};

use crate::{Act, Agent, COMMAND, NAME, Overview, Settings, Status};

/// The Agents page.
pub const OVERVIEW: &str = "agents";

/// The New agent form.
pub const NEW: &str = "new-agent";

/// tau-agents' interface half.
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentsUi;

/// What a window keeps of tau-agents: the New agent form.
pub struct Ui {
    handle: Handle,
    pub(crate) name: Entity<TextInput>,
    pub(crate) purpose: Entity<TextInput>,
    /// The name the form last gave from the purpose: a name still equal
    /// to it follows the purpose as it changes.
    suggested: String,
    /// Its model; `None` is the default.
    pub(crate) model: Option<String>,
    pub(crate) daily_cap: f64,
    pub(crate) window_secs: u64,
    /// The agent the host made, until its conversation opens.
    pub(crate) making: Option<String>,
    /// Why the host could not make it.
    pub(crate) error: Option<String>,
    /// A secret to save from an agent's panel: its name, and its value,
    /// drawn as dots and cleared once sent.
    pub(crate) secret_name: Entity<TextInput>,
    pub(crate) secret_value: Entity<TextInput>,
    /// The answer to an agent's question that offers no choices.
    pub(crate) answer: Entity<TextInput>,
}

impl Ui {
    /// Fills the form for an agent that does `purpose`.
    pub(crate) fn prefill(&mut self, purpose: &str, cx: &mut Context<Self>) {
        self.error = None;
        self.purpose
            .update(cx, |input, cx| input.set_text(purpose.to_owned(), cx));
        self.follow_purpose(cx);
    }

    /// Names the agent after its purpose, unless the person named it.
    fn follow_purpose(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).text().to_owned();
        if !name.is_empty() && name != self.suggested {
            return;
        }
        let purpose = self.purpose.read(cx).text().to_owned();
        let suggested = if purpose.trim().is_empty() {
            String::new()
        } else {
            crate::shown::suggest_name(&purpose)
        };
        self.suggested = suggested.clone();
        self.name
            .update(cx, |input, cx| input.set_text(suggested, cx));
    }

    /// The form, as what the host is asked to make.
    pub(crate) fn act(&self, cx: &gpui::App) -> Result<Act, String> {
        let name = self.name.read(cx).text().trim().to_owned();
        let purpose = self.purpose.read(cx).text().trim().to_owned();
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
            servers: Vec::new(),
        })
    }

    /// Asks the host to make the agent the form describes.
    pub(crate) fn submit(&mut self, cx: &mut Context<Self>) {
        match self.act(cx) {
            Ok(act) => {
                self.error = None;
                self.handle.act(act, cx);
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// Saves the secret the panel's fields hold as `agent`'s, and
    /// forgets its value.
    pub(crate) fn save_secret(&mut self, agent: &str, cx: &mut Context<Self>) {
        let name = self.secret_name.read(cx).text().trim().to_owned();
        let value = self.secret_value.read(cx).text().to_owned();
        if name.is_empty() || value.is_empty() {
            return;
        }
        let agent = agent.to_owned();
        self.handle.act(Act::SetSecret { agent, name, value }, cx);
        self.secret_name.update(cx, |input, cx| input.clear(cx));
        self.secret_value.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    /// Answers `agent`'s question `notice` with what the answer field
    /// holds, and clears it.
    pub(crate) fn send_answer(
        &mut self,
        agent: &str,
        notice: &str,
        cx: &mut Context<Self>,
    ) {
        let answer = self.answer.read(cx).text().trim().to_owned();
        if answer.is_empty() {
            return;
        }
        self.handle.act(
            Act::Answer {
                agent: agent.to_owned(),
                notice: notice.to_owned(),
                answer,
            },
            cx,
        );
        self.answer.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    /// Clears the form once its agent's conversation opened.
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.making = None;
        self.error = None;
        self.suggested.clear();
        self.name.update(cx, |input, cx| input.clear(cx));
        self.purpose.update(cx, |input, cx| input.clear(cx));
    }
}

impl PluginUi for Ui {
    fn new(handle: Handle, cx: &mut Context<Self>) -> Self {
        let name =
            cx.new(|cx| TextInput::new("pr-triage", cx).keep_on_submit());
        let purpose = cx.new(|cx| {
            TextInput::new(
                "Watch new pull requests on tau-agent, label them, and \
                 tell me when one touches auth.",
                cx,
            )
            .multiline()
            .keep_on_submit()
        });
        cx.observe(&name, |_, _, cx| cx.notify()).detach();
        cx.observe(&purpose, |ui: &mut Ui, _, cx| {
            ui.follow_purpose(cx);
            cx.notify();
        })
        .detach();
        let secret_name = cx.new(|cx| TextInput::new("github", cx));
        let secret_value =
            cx.new(|cx| TextInput::new("its value", cx).masked());
        cx.observe(&secret_name, |_, _, cx| cx.notify()).detach();
        cx.observe(&secret_value, |_, _, cx| cx.notify()).detach();
        let answer = cx.new(|cx| TextInput::new("Your answer", cx));
        cx.observe(&answer, |_, _, cx| cx.notify()).detach();
        let defaults = Settings::default();
        Self {
            secret_name,
            secret_value,
            answer,
            handle,
            name,
            purpose,
            suggested: String::new(),
            model: None,
            daily_cap: defaults.daily_cap,
            window_secs: defaults.window_secs,
            making: None,
            error: None,
        }
    }
}

impl UiPlugin for AgentsUi {
    type State = ();
    type Data = Overview;
    type RepoData = ();
    type Settings = Settings;
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    /// The host made the agent, or says why not.
    fn reply(&self, ui: &mut Ui, reply: Value, cx: &mut Context<Ui>) {
        if let Some(repo) = reply.get("repo").and_then(Value::as_str) {
            ui.making = Some(repo.to_owned());
        } else if let Some(error) = reply.get("error").and_then(Value::as_str) {
            ui.making = None;
            ui.error = Some(error.to_owned());
        }
        cx.notify();
    }

    fn read_prompt(&self, prompt: &str) -> Option<String> {
        prompt
            .strip_prefix(&format!("/{} ", crate::SKILL))
            .map(|purpose| purpose.trim().to_owned())
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(
                Page::new(OVERVIEW, page::overview).title(|_| "Agents".into()),
            )
            .page(Page::new(NEW, page::new_agent).title(|_| "New agent".into()))
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
            .contribute(points::RUN_BANNER, run::banner)
            .contribute_at(points::INSPECTOR, -200, run::panel)
            .contribute(points::USER_MESSAGE, run::event_card)
    }
}

/// `/agent <purpose>`: the New agent form, filled in from what follows.
fn new_agent(args: &str, view: &mut ViewCx<'_, AgentsUi>) {
    let purpose = args.trim().to_owned();
    view.ui.update(view.cx, |ui, cx| ui.prefill(&purpose, cx));
    view.handle.navigate(Link::page(NEW), view.cx);
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
    view.handle.act(act, view.cx);
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
        view.handle.open_run(&run, view.cx);
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

/// An agent's mark: its letter in a ring of its color, round where a
/// repository's is square.
pub fn mark(name: &str, size: f32, t: &Theme) -> Div {
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or("?".into(), |c| c.to_lowercase().to_string());
    let color = t.mark(name);
    div()
        .size(rems(size / 16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::FULL)
        .border_1()
        .border_color(color.opacity(0.7))
        .bg(color.opacity(0.12))
        .text_color(color)
        .typeset(Type::MICRO.sized(size * 0.55))
        .font_weight(weight::STRONG)
        .child(letter)
}

/// An agent's state on a small tinted pill: `working`, `at its cap`.
pub fn status_pill(status: &Status, t: &Theme) -> Div {
    let color = t.tone(tone(status));
    div()
        .flex_shrink_0()
        .px(tau_ui_kit::theme::sp(1.75))
        .rounded(radius::FULL)
        .border_1()
        .border_color(color.opacity(0.4))
        .bg(color.opacity(0.1))
        .typeset(Type::MICRO)
        .text_color(color)
        .child(status.label())
}
