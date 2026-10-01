//! tau-ask's UI (ADR 0017): the panel that takes the composer's place
//! while a question waits ([`points::COMPOSER`]), the card of an `ask`
//! call, and the plugin's line in a run's plugin list.

pub mod card;
pub mod draft;
pub mod panel;

use std::collections::{BTreeMap, BTreeSet};

use gpui::{App, AppContext as _, Context, Entity, FocusHandle};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_kit::{input::TextInput, theme::Tone};
use tau_ui_plugin::{
    Handle,
    HostCx,
    Manifest,
    PluginInfo,
    PluginStatus,
    RunCtx,
    RunCx,
    Seam,
    UiPlugin,
    points::{self, AtRun},
};

pub use self::draft::{Draft, Key, Then};
use crate::{Ask, NAME, Record, Reply};

/// tau-ask with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct AskUi;

/// The calls of a run that asked, as tau-ask's records leave them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// In the order they asked.
    pub calls: Vec<Call>,
}

/// One call that asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub call: String,
    pub ask: Ask,
    pub reply: Option<Reply>,
    /// It stopped waiting without a reply.
    pub closed: bool,
}

impl Call {
    /// Whether it still waits for the person.
    pub fn waits(&self) -> bool {
        self.reply.is_none() && !self.closed
    }
}

impl State {
    /// Folds one of the plugin's records.
    pub fn apply(&mut self, body: &Value) {
        let Some(record) = Record::parse(body) else {
            return;
        };
        let found = self.calls.iter().position(|c| c.call == record.call());
        match (record, found) {
            (Record::Asked { call, ask }, Some(at)) => {
                self.calls[at] = Call {
                    call,
                    ask,
                    reply: None,
                    closed: false,
                };
            }
            (Record::Asked { call, ask }, None) => self.calls.push(Call {
                call,
                ask,
                reply: None,
                closed: false,
            }),
            (Record::Answered { reply, .. }, Some(at)) => {
                self.calls[at].reply = Some(reply);
            }
            (Record::Closed { .. }, Some(at)) => self.calls[at].closed = true,
            // About a call it never saw ask: nothing to show.
            (Record::Answered { .. } | Record::Closed { .. }, None) => {}
        }
    }

    /// The first call still waiting for the person.
    pub fn waiting(&self) -> Option<&Call> {
        self.calls.iter().find(|call| call.waits())
    }

    /// The call `call`.
    pub fn call(&self, call: &str) -> Option<&Call> {
        self.calls.iter().find(|c| c.call == call)
    }
}

/// What the panel asks the host half: give a waiting call its reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Act {
    pub run: String,
    pub call: String,
    pub reply: Reply,
}

/// A waiting call, by its run and its id: two runs can wait at once, and
/// their calls can share an id.
pub type CallKey = (String, String);

/// What the host half says when it could not give a call its reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refused {
    pub run: String,
    pub call: String,
    pub message: String,
}

/// The panel's state in a window: the answers being written to each
/// waiting call, and its fields.
pub struct Ui {
    /// The panel's own focus: the keys it takes come through it.
    pub focus: FocusHandle,
    /// The person's own answer to the question in view.
    pub other: Entity<TextInput>,
    /// The note on the question in view.
    pub note: Entity<TextInput>,
    /// The answers being written, with what each call asked.
    pub drafts: BTreeMap<CallKey, (Ask, Draft)>,
    /// The call whose draft the fields hold now.
    pub shown: Option<CallKey>,
    /// The calls whose panel took the focus as it first appeared.
    pub focused: BTreeSet<CallKey>,
    /// Whether the panel takes the keys back as it draws next.
    pub refocus: bool,
    /// The calls whose answers went, waiting for the call to end.
    pub sent: BTreeSet<CallKey>,
    /// Why the host could not take a call's answers.
    pub refused: BTreeMap<CallKey, String>,
}

impl Ui {
    /// The draft for `key`, made if there is none.
    pub fn draft_for(&mut self, key: &CallKey, ask: &Ask) -> &mut Draft {
        &mut self
            .drafts
            .entry(key.clone())
            .or_insert_with(|| (ask.clone(), Draft::new(key.1.clone(), ask)))
            .1
    }

    /// Takes what the fields hold into the draft they show.
    pub fn take_fields(&mut self, cx: &App) {
        let other = self.other.read(cx).text().to_owned();
        let note = self.note.read(cx).text().to_owned();
        let Some((ask, draft)) =
            self.shown.as_ref().and_then(|key| self.drafts.get_mut(key))
        else {
            return;
        };
        draft.write_other(ask, &other);
        draft.write_note(ask, &note);
    }

    /// Fills the fields from the draft they show, for its question in
    /// view.
    pub fn fill_fields(&mut self, cx: &mut Context<Self>) {
        let Some((_, draft)) =
            self.shown.as_ref().and_then(|key| self.drafts.get(key))
        else {
            return;
        };
        let at = draft.tab.min(draft.other.len().saturating_sub(1));
        let (other, note) = (draft.other[at].clone(), draft.notes[at].clone());
        self.other.update(cx, |input, cx| input.set_text(other, cx));
        self.note.update(cx, |input, cx| input.set_text(note, cx));
    }

    /// Shows `key`'s draft in the fields, keeping what they held for the
    /// draft they showed before.
    pub fn show(&mut self, key: &CallKey, cx: &mut Context<Self>) {
        if self.shown.as_ref() == Some(key) {
            self.take_fields(cx);
            return;
        }
        self.take_fields(cx);
        self.shown = Some(key.clone());
        self.fill_fields(cx);
    }
}

/// The host half: the calls waiting, shared by every run.
#[cfg(feature = "host")]
pub type Host = crate::host::Waiting;
#[cfg(not(feature = "host"))]
pub type Host = ();

impl UiPlugin for AskUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Host = Host;
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    fn host(&self, _cx: &HostCx) -> anyhow::Result<Host> {
        Ok(Host::default())
    }

    /// The `ask` tool, for a run a person watches: a sub-agent has no
    /// one to ask.
    fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        #[cfg(feature = "host")]
        if run.kind != tau_ui_plugin::RunKind::SubAgent {
            return Ok(vec![Box::new(crate::host::AskPlugin::new(
                host.clone(),
            ))]);
        }
        let _ = (host, run);
        Ok(Vec::new())
    }

    fn catalog(
        &self,
        _host: &Host,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            name: NAME.into(),
            description:
                "Lets the agent ask you questions and waits for your answers"
                    .into(),
            seams: vec![Seam::Start, Seam::Tools],
            spend: 0.0,
            page: None,
        }
    }

    fn act(
        &self,
        host: &Host,
        action: Value,
        _cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let act: Act = serde_json::from_value(action)?;
        // Refused, the panel says why and takes answers again.
        #[cfg(feature = "host")]
        {
            Ok(host
                .answer(&act.run, &act.call, act.reply)
                .err()
                .map(|error| {
                    serde_json::to_value(Refused {
                        run: act.run,
                        call: act.call,
                        message: format!("{error:#}"),
                    })
                    .expect("a refusal serializes")
                }))
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = (host, act);
            anyhow::bail!(
                "tau-ask's answers go to the computer that runs the agent"
            )
        }
    }

    fn reply(&self, ui: &mut Ui, reply: Value, cx: &mut Context<Ui>) {
        if let Ok(refused) = serde_json::from_value::<Refused>(reply) {
            let key = (refused.run, refused.call);
            ui.sent.remove(&key);
            ui.refused.insert(key, refused.message);
            cx.notify();
        }
    }

    fn apply(&self, state: &mut State, body: &Value, _run: &mut dyn RunCx) {
        state.apply(body);
    }

    fn new_ui(&self, handle: Handle, cx: &mut Context<Ui>) -> Ui {
        let other = cx
            .new(|cx| TextInput::new("Your own answer…", cx).keep_on_submit());
        let note = cx.new(|cx| {
            TextInput::new(
                "Anything the agent should know about this answer…",
                cx,
            )
            .multiline()
            .keep_on_submit()
        });
        panel::subscribe(&other, &note, handle, cx);
        Ui {
            focus: cx.focus_handle(),
            other,
            note,
            drafts: BTreeMap::new(),
            shown: None,
            focused: BTreeSet::new(),
            refocus: false,
            sent: BTreeSet::new(),
            refused: BTreeMap::new(),
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::COMPOSER, panel::panel)
            .contribute(points::CARD, card::card)
            .contribute(points::STATUS, |at: &AtRun, view| {
                let waiting = view.state?.waiting()?;
                let n = waiting.ask.questions.len();
                Some(PluginStatus {
                    name: NAME.into(),
                    state: match (at.run.live, n) {
                        (true, 1) => "1 question waiting".to_owned(),
                        (true, n) => format!("{n} questions waiting"),
                        (false, _) => "not answered".to_owned(),
                    },
                    tone: Tone::Warn,
                })
            })
    }
}
