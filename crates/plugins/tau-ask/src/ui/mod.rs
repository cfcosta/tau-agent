//! tau-ask's UI (ADR 0017): the panel that takes the composer's place
//! while a question waits ([`points::COMPOSER`]), the card of an `ask`
//! call, and the plugin's line in a run's plugin list.

pub mod card;
pub mod draft;
pub mod panel;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    Fold,
    Handle,
    Manifest,
    PluginStatus,
    RunCx,
    UiPlugin,
    ViewCx,
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

impl Fold for State {
    type Record = Record;

    /// Folds one of the plugin's records.
    fn apply(&mut self, record: Record, _run: &mut dyn RunCx) {
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
}

impl State {
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

/// The panel's state, which is the interface's (ADR 0045), by the run
/// and the call it answers: the draft, whether its answers went, why the
/// host refused them, and its focus numbers.
pub fn draft_key((run, call): &CallKey) -> String {
    format!("draft:{run}:{call}")
}

pub fn sent_key((run, call): &CallKey) -> String {
    format!("sent:{run}:{call}")
}

pub fn refused_key((run, call): &CallKey) -> String {
    format!("refused:{run}:{call}")
}

pub fn focus_key((run, call): &CallKey) -> String {
    format!("focus:{run}:{call}")
}

/// Which of the panel's parts takes the keys next: each count rises to
/// give its part the keys once (ADR 0045).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub struct Focus {
    pub panel: u64,
    pub other: u64,
    pub note: u64,
}

impl Focus {
    /// Before anything moved it: the panel takes the keys as it first
    /// appears.
    pub fn start() -> Self {
        Self {
            panel: 1,
            ..Self::default()
        }
    }
}

impl UiPlugin for AskUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    /// The host refused the answers: they did not go, and the panel says
    /// why.
    fn reply(&self, reply: Value, handle: &Handle) {
        if let Ok(refused) = serde_json::from_value::<Refused>(reply) {
            let key = (refused.run, refused.call);
            handle.set_ui(sent_key(&key), None);
            handle.set_ui(
                refused_key(&key),
                Some(Value::String(refused.message)),
            );
        }
    }

    /// The panel's keys and clicks: every action it has is the panel's.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        panel::act(action, args, view);
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
            // The run asks the person while a live call waits: its
            // first question says what.
            .contribute(points::ASKS, |at: &AtRun, view| {
                let waiting = view.state?.waiting().filter(|_| at.run.live)?;
                Some(waiting.ask.questions.first()?.question.clone())
            })
    }
}
