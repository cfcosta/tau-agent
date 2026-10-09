//! What the watcher knows of one run: its notes and what the person did
//! with them, folded from its records. The agent half reads the same
//! fold to decide whether to check.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_plugin::{Fold, RunCx};

use crate::{
    cadence::{self, BAND_LIMIT},
    record::{Answer, Explain, NAME, Record, Tag},
};

/// Where a note stands with the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Not acted on.
    New,
    Learned,
    Knew,
    Chatted,
    Dismissed,
}

/// One note, at its anchor in the transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// The anchor: `n0`, `n1`, ...
    pub key: String,
    pub step: u32,
    pub tag: Tag,
    pub line: String,
    pub explain: Option<Explain>,
    pub status: Status,
    /// How many times a message was written past it unanswered.
    pub typed_past: u32,
}

impl Note {
    /// Whether it is still a line above the composer: new, and not
    /// written past [`BAND_LIMIT`] times.
    pub fn in_band(&self) -> bool {
        self.status == Status::New && self.typed_past < BAND_LIMIT
    }
}

/// The watcher's notes in a run, and how often the person ignored them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub notes: Vec<Note>,
    /// Messages written past a note unanswered, since the person last
    /// answered one: the back-off's count.
    pub ignored: u32,
    /// Replies that could not be used.
    pub dropped: u32,
    /// The step of the last check that made a record.
    pub last_check: Option<u32>,
}

impl State {
    /// The state `bodies` fold to, as stored.
    pub fn from_records(bodies: &[Value]) -> Self {
        let mut state = Self::default();
        for record in tau_agent::plugin::read_records::<Record>(NAME, bodies) {
            state.record(record);
        }
        state
    }

    /// Folds one record. A note's anchor key comes back when it needs a
    /// place in the transcript.
    pub fn record(&mut self, record: Record) -> Option<String> {
        match record {
            Record::Noted {
                step,
                tag,
                line,
                explain,
            } => {
                let key = format!("n{}", self.notes.len());
                self.last_check = Some(step);
                self.notes.push(Note {
                    key: key.clone(),
                    step,
                    tag,
                    line,
                    explain,
                    status: Status::New,
                    typed_past: 0,
                });
                return Some(key);
            }
            Record::Dropped { step, .. } => {
                self.last_check = Some(step);
                self.dropped += 1;
            }
            Record::TypedPast => {
                if let Some(note) = self
                    .notes
                    .last_mut()
                    .filter(|note| note.status == Status::New)
                {
                    note.typed_past += 1;
                    self.ignored += 1;
                }
            }
            Record::Answered { key, answer } => {
                if let Some(note) =
                    self.notes.iter_mut().find(|note| note.key == key)
                {
                    note.status = match answer {
                        Answer::Learned => Status::Learned,
                        Answer::Knew => Status::Knew,
                        Answer::Chatted => Status::Chatted,
                        Answer::Dismissed => Status::Dismissed,
                    };
                    self.ignored = 0;
                }
            }
        }
        None
    }

    /// The note shown above the composer, if one is.
    pub fn band(&self) -> Option<&Note> {
        self.notes.last().filter(|note| note.in_band())
    }

    /// Whether the last note still waits for the person to act, which
    /// holds the checks back.
    pub fn waiting(&self) -> bool {
        self.band().is_some()
    }

    /// Whether a message written now is written past a note.
    pub fn unanswered(&self) -> bool {
        self.notes
            .last()
            .is_some_and(|note| note.status == Status::New)
    }

    /// The lines of the last notes, oldest first: what the prompt says
    /// was already said.
    pub fn seen(&self) -> Vec<String> {
        let lines: Vec<String> =
            self.notes.iter().map(|note| note.line.clone()).collect();
        cadence::recent(&lines).to_vec()
    }

    /// The lines the person said they knew, oldest first.
    pub fn known(&self) -> Vec<String> {
        let lines: Vec<String> = self
            .notes
            .iter()
            .filter(|note| note.status == Status::Knew)
            .map(|note| note.line.clone())
            .collect();
        cadence::recent(&lines).to_vec()
    }

    /// Whether the request at step `steps` gets a check.
    pub fn due(&self, steps: u32) -> bool {
        !self.waiting() && cadence::due(steps, self.last_check, self.ignored)
    }

    /// The plugin's line in the run's plugin list.
    pub fn status(&self) -> Option<String> {
        Some(match self.notes.len() {
            0 => return None,
            1 => "1 note".into(),
            n => format!("{n} notes"),
        })
    }
}

impl Fold for State {
    type Record = Record;

    fn apply(&mut self, record: Record, run: &mut dyn RunCx) {
        if let Some(key) = self.record(record) {
            run.transcript(&key);
        }
    }
}
