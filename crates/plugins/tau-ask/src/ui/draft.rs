//! A person's answers as they write them: the question in view, what is
//! picked and written for each, and what each key does. Kept apart from
//! drawing so it can be tested on its own.

use std::collections::BTreeSet;

use crate::{Answer, Ask, Reply};

/// The answers to one call, as they stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// The call it answers.
    pub call: String,
    /// The question in view; one past the last is the review.
    pub tab: usize,
    /// Each question's row in focus: a choice, or one past the last for
    /// the person's own answer.
    pub cursor: Vec<usize>,
    /// Each question's choices picked.
    pub picked: Vec<BTreeSet<usize>>,
    /// Each question's own answer, and whether it counts.
    pub other: Vec<String>,
    pub other_on: Vec<bool>,
    /// Each question's note.
    pub notes: Vec<String>,
    /// Whether the note of the question in view is open.
    pub note_open: bool,
}

/// A key the panel takes while it has the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Space,
    /// `1` is the first row.
    Digit(usize),
    /// `n`: the note of the question in view.
    Note,
}

impl Key {
    /// The key a keystroke names, if the panel takes it.
    pub fn of(key: &str) -> Option<Self> {
        Some(match key {
            "up" => Self::Up,
            "down" => Self::Down,
            "left" => Self::Left,
            "right" => Self::Right,
            "enter" => Self::Enter,
            "space" => Self::Space,
            "n" => Self::Note,
            digit => match digit.parse::<usize>() {
                Ok(n @ 1..=9) => Self::Digit(n),
                _ => return None,
            },
        })
    }
}

/// What the panel does after a key, besides drawing again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    Stay,
    /// Put the focus in the field for the person's own answer.
    WriteOther,
    /// Put the focus in the note's field.
    WriteNote,
    /// Send the answers.
    Send,
}

impl Draft {
    pub fn new(call: impl Into<String>, ask: &Ask) -> Self {
        let n = ask.questions.len();
        Self {
            call: call.into(),
            tab: 0,
            cursor: vec![0; n],
            picked: vec![BTreeSet::new(); n],
            other: vec![String::new(); n],
            other_on: vec![false; n],
            notes: vec![String::new(); n],
            note_open: false,
        }
    }

    /// Whether the review is in view.
    pub fn reviewing(&self, ask: &Ask) -> bool {
        self.tab >= ask.questions.len()
    }

    /// The row of the person's own answer in question `i`.
    fn other_row(ask: &Ask, i: usize) -> usize {
        ask.questions[i].options.len()
    }

    /// Whether question `i` has an answer.
    pub fn answered(&self, i: usize) -> bool {
        !self.picked[i].is_empty()
            || (self.other_on[i] && !self.other[i].trim().is_empty())
    }

    /// How many questions have an answer.
    pub fn count(&self) -> usize {
        (0..self.picked.len()).filter(|&i| self.answered(i)).count()
    }

    /// Whether every question has an answer.
    pub fn ready(&self) -> bool {
        self.count() == self.picked.len()
    }

    /// The answer to question `i`, as it stands.
    pub fn answer(&self, ask: &Ask, i: usize) -> Answer {
        let question = &ask.questions[i];
        let other = self.other[i].trim();
        let note = self.notes[i].trim();
        Answer {
            picked: self.picked[i]
                .iter()
                .map(|&k| question.options[k].label.clone())
                .collect(),
            other: (self.other_on[i] && !other.is_empty())
                .then(|| other.to_owned()),
            note: (!note.is_empty()).then(|| note.to_owned()),
        }
    }

    /// The reply, once every question has an answer.
    pub fn reply(&self, ask: &Ask) -> Option<Reply> {
        self.ready().then(|| Reply::Answered {
            answers: (0..ask.questions.len())
                .map(|i| self.answer(ask, i))
                .collect(),
        })
    }

    /// Shows question `tab`, or the review one past the last.
    pub fn go(&mut self, ask: &Ask, tab: usize) {
        self.tab = tab.min(ask.questions.len());
        self.note_open = false;
    }

    /// Picks row `row` of the question in view: a choice, or the
    /// person's own answer. One-answer questions go on to the next when
    /// `advance`.
    pub fn choose(&mut self, ask: &Ask, row: usize, advance: bool) -> Then {
        let i = self.tab;
        if self.reviewing(ask) || row > Self::other_row(ask, i) {
            return Then::Stay;
        }
        self.cursor[i] = row;
        let multi = ask.questions[i].multi_select;
        if row == Self::other_row(ask, i) {
            if multi && self.other_on[i] {
                self.other_on[i] = false;
                return Then::Stay;
            }
            self.other_on[i] = true;
            if !multi {
                self.picked[i].clear();
            }
            return Then::WriteOther;
        }
        if multi {
            if !self.picked[i].remove(&row) {
                self.picked[i].insert(row);
            }
            return Then::Stay;
        }
        self.picked[i] = BTreeSet::from([row]);
        self.other_on[i] = false;
        if advance {
            self.go(ask, i + 1);
        }
        Then::Stay
    }

    /// Takes what the person wrote as their own answer to the question
    /// in view: written, it counts, in place of a choice where one is
    /// picked.
    pub fn write_other(&mut self, ask: &Ask, text: &str) {
        let i = self.tab;
        if self.reviewing(ask) || self.other[i] == text {
            return;
        }
        self.other[i] = text.to_owned();
        if !text.trim().is_empty() {
            self.other_on[i] = true;
            if !ask.questions[i].multi_select {
                self.picked[i].clear();
            }
        }
    }

    /// Takes the note of the question in view.
    pub fn write_note(&mut self, ask: &Ask, text: &str) {
        if !self.reviewing(ask) {
            self.notes[self.tab] = text.to_owned();
        }
    }

    /// Enter in the field of the person's own answer: a one-answer
    /// question goes on to the next.
    pub fn other_done(&mut self, ask: &Ask) {
        let i = self.tab;
        if !self.reviewing(ask)
            && !ask.questions[i].multi_select
            && self.answered(i)
        {
            self.go(ask, i + 1);
        }
    }

    /// What `key` does while the panel has the focus.
    pub fn key(&mut self, ask: &Ask, key: Key) -> Then {
        match key {
            Key::Left => {
                self.go(ask, self.tab.saturating_sub(1));
                return Then::Stay;
            }
            Key::Right => {
                self.go(ask, self.tab + 1);
                return Then::Stay;
            }
            _ => {}
        }
        if self.reviewing(ask) {
            return match key {
                Key::Enter if self.ready() => Then::Send,
                _ => Then::Stay,
            };
        }
        let i = self.tab;
        let multi = ask.questions[i].multi_select;
        let rows = Self::other_row(ask, i) + 1;
        match key {
            Key::Up => self.cursor[i] = self.cursor[i].saturating_sub(1),
            Key::Down => self.cursor[i] = (self.cursor[i] + 1).min(rows - 1),
            Key::Digit(n) if n <= rows => {
                return self.choose(ask, n - 1, !multi);
            }
            Key::Space => return self.choose(ask, self.cursor[i], false),
            Key::Enter if multi => self.go(ask, i + 1),
            Key::Enter => return self.choose(ask, self.cursor[i], true),
            Key::Note => {
                self.note_open = true;
                return Then::WriteNote;
            }
            _ => {}
        }
        Then::Stay
    }
}
