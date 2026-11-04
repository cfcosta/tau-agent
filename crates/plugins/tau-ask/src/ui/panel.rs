//! The panel that takes the composer's place while a call waits for the
//! person: a tab per question and one to review, each question's
//! choices, a field for the person's own answer, a note per answer, and
//! the keys that work them. A view tree (ADR 0045): the draft, whether
//! the answers went and what takes the keys are the interface's state.
//!
//! The panel takes the keys when it first appears. With them: ↑↓ move,
//! a digit picks a row, Enter picks (and goes on, where one answer is
//! picked), Space toggles, ←→ change questions, and `n` opens the note
//! of the question in view. In a field, Esc goes back to the panel;
//! Enter leaves the note, or takes the person's own answer.

use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_ui_plugin::{
    Handle,
    ViewCx,
    points::AtRun,
    tree::{Kind, Node, Tone, draft_key as input_key},
};

use super::{
    Act,
    AskUi,
    CallKey,
    Draft,
    Focus,
    Key,
    State,
    Then,
    draft_key,
    focus_key,
    refused_key,
    sent_key,
};
use crate::{Ask, Reply};

/// The keys the panel takes while it has them.
const KEYS: [&str; 17] = [
    "up", "down", "left", "right", "enter", "space", "n", "1", "2", "3", "4",
    "5", "6", "7", "8", "9", "escape",
];

/// The panel's actions.
const KEY: &str = "key";
const GO: &str = "go";
const CHOOSE: &str = "choose";
const OTHER_DONE: &str = "other-done";
const NOTE_DONE: &str = "note-done";
const NOTE_OPEN: &str = "note-open";
const NEXT: &str = "next";
const SEND: &str = "send";
const DECLINE: &str = "decline";
const CANCEL: &str = "cancel";

/// The input of the person's own answer to question `i`, and of its note.
fn other_input((run, call): &CallKey, i: usize) -> String {
    format!("ask-other-{run}-{call}-{i}")
}

pub fn note_input((run, call): &CallKey, i: usize) -> String {
    format!("ask-note-{run}-{call}-{i}")
}

/// What the panel draws from, as the interface keeps it: the draft,
/// with what the fields hold taken in, whether the answers went, why
/// the host refused them, and what takes the keys.
pub struct Held {
    pub draft: Draft,
    pub sent: bool,
    pub refused: Option<String>,
    pub focus: Focus,
}

impl Held {
    fn of(view: &ViewCx<'_, AskUi>, key: &CallKey, ask: &Ask) -> Self {
        let read = |state: &str| view.ui_value(state);
        let mut draft = read(&draft_key(key))
            .and_then(|value| {
                serde_json::from_value::<Draft>(value.clone()).ok()
            })
            .filter(|draft| draft.picked.len() == ask.questions.len())
            .unwrap_or_else(|| Draft::new(key.1.clone(), ask));
        // The fields' drafts are the person's own answers and notes.
        let tab = draft.tab;
        for i in 0..ask.questions.len() {
            let field = |id: String| {
                read(&input_key(&id))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            draft.tab = i;
            if let Some(other) = field(other_input(key, i)) {
                draft.write_other(ask, &other);
            }
            if let Some(note) = field(note_input(key, i)) {
                draft.write_note(ask, &note);
            }
        }
        draft.tab = tab;
        Self {
            draft,
            sent: read(&sent_key(key))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            refused: read(&refused_key(key))
                .and_then(Value::as_str)
                .map(str::to_owned),
            focus: read(&focus_key(key))
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_else(Focus::start),
        }
    }
}

/// The panel for the run's waiting call, in the composer's place; none
/// when no call waits or the run has ended.
pub fn panel(at: &AtRun, view: &mut ViewCx<'_, AskUi>) -> Option<Node> {
    if !at.run.live {
        return None;
    }
    let call = view.state?.waiting()?.clone();
    let key: CallKey = (at.run.id.0.to_string(), call.call.clone());
    let held = Held::of(view, &key, &call.ask);
    Some(tree(&key, &call.ask, &held, view.compact))
}

/// What the panel's keys and clicks do, on the call the run the view is
/// of waits on, as the interface holds its draft now: change the draft
/// and keep it, give the keys to a field or the panel, send the answers.
pub fn act(action: &str, args: &Value, view: &ViewCx<'_, AskUi>) {
    let Some(run) = view.run else {
        return;
    };
    let Some(call) = view.state.and_then(State::waiting) else {
        return;
    };
    let key: CallKey = (run.id.0.to_string(), call.call.clone());
    let ask = &call.ask;
    let held = Held::of(view, &key, ask);
    let handle = &view.handle;
    let mut draft = held.draft.clone();
    let mut focus = held.focus;
    let text = || args.get("text").and_then(Value::as_str).unwrap_or("");
    let row = |name: &str| {
        args.get(name)
            .and_then(Value::as_u64)
            .map_or(0, |n| n as usize)
    };
    let then = match action {
        // Nothing changes once the answers went, but cancelling.
        CANCEL => {
            handle.cancel(&RunId(key.0.as_str().into()));
            return;
        }
        _ if held.sent => return,
        KEY => {
            let name = args.get("key").and_then(Value::as_str).unwrap_or("");
            let in_field =
                args.get("field").and_then(Value::as_bool).unwrap_or(false);
            if in_field || name == "escape" {
                // Esc in a field: back to the panel.
                draft.note_open = false;
                focus.panel += 1;
                Then::Stay
            } else {
                match Key::of(name) {
                    Some(key) => draft.key(ask, key),
                    None => return,
                }
            }
        }
        GO => {
            draft.go(ask, row("to"));
            Then::Stay
        }
        CHOOSE => draft.choose(ask, row("row"), false),
        OTHER_DONE => {
            draft.write_other(ask, text());
            draft.other_done(ask);
            focus.panel += 1;
            Then::Stay
        }
        NOTE_DONE => {
            draft.write_note(ask, text());
            draft.note_open = false;
            focus.panel += 1;
            Then::Stay
        }
        NOTE_OPEN => {
            draft.note_open = true;
            Then::WriteNote
        }
        NEXT => {
            draft.go(ask, draft.tab + 1);
            Then::Stay
        }
        SEND => Then::Send,
        DECLINE => {
            send(handle, &key, Reply::Declined);
            return;
        }
        _ => return,
    };
    match then {
        Then::Stay => {}
        Then::WriteOther => focus.other += 1,
        Then::WriteNote => {
            // The field shows the note as it stands.
            let note = draft.notes.get(draft.tab).cloned().unwrap_or_default();
            handle.set_ui(
                input_key(&note_input(&key, draft.tab)),
                (!note.is_empty()).then(|| Value::String(note)),
            );
            focus.note += 1;
        }
        Then::Send => {
            if let Some(reply) = draft.reply(ask) {
                send(handle, &key, reply);
            }
        }
    }
    handle.set_ui(draft_key(&key), serde_json::to_value(&draft).ok());
    if focus != held.focus {
        handle.set_ui(focus_key(&key), serde_json::to_value(focus).ok());
    }
}

/// Sends the reply: until the call ends or the host refuses it, the
/// panel shows it going.
fn send(handle: &Handle, key: &CallKey, reply: Reply) {
    handle.set_ui(refused_key(key), None);
    handle.set_ui(sent_key(key), Some(Value::Bool(true)));
    handle.act(Act {
        run: key.0.clone(),
        call: key.1.clone(),
        reply,
    });
}

/// The panel's tree: an area that takes the keys, holding the tabs, the
/// question in view or the review, why the host refused, and the foot.
pub fn tree(key: &CallKey, ask: &Ask, held: &Held, compact: bool) -> Node {
    let draft = &held.draft;
    let mut children = vec![tabs(ask, draft, compact)];
    children.push(if draft.reviewing(ask) {
        review(ask, draft)
    } else {
        question(key, ask, draft, &held.focus)
    });
    if let Some(why) = &held.refused {
        children.push(Node::Toned {
            text: why.clone(),
            tone: Tone::Bad,
            mono: false,
        });
    }
    children.push(foot(ask, draft, held.sent, compact));
    // One area for every run's panel, as one composer is: the keys stay
    // with the panel from one waiting run to the next.
    Node::Keys {
        id: "ask-panel".into(),
        keys: KEYS.iter().map(|key| (*key).to_owned()).collect(),
        through: vec!["escape".into()],
        action: KEY.into(),
        focus: Some(held.focus.panel),
        children: vec![Node::Card { children }],
    }
}

fn button(
    label: impl Into<String>,
    kind: Kind,
    action: &str,
    args: Value,
) -> Node {
    Node::Button {
        label: label.into(),
        action: action.into(),
        args,
        kind,
        icon: None,
    }
}

/// A tab per question, marked once answered, and one to review; what
/// the question in view takes, beside them.
fn tabs(ask: &Ask, draft: &Draft, compact: bool) -> Node {
    let mut row = Vec::new();
    for (i, question) in ask.questions.iter().enumerate() {
        let mut label = question.header.clone();
        if draft.answered(i) {
            label = format!("✓ {label}");
        }
        if !draft.notes[i].trim().is_empty() {
            label.push_str(" ✎");
        }
        let kind = if draft.tab == i {
            Kind::Primary
        } else {
            Kind::Secondary
        };
        row.push(button(label, kind, GO, json!({ "to": i })));
    }
    let last = ask.questions.len();
    row.push(Node::Mono { text: "›".into() });
    row.push(button(
        "Submit",
        if draft.reviewing(ask) {
            Kind::Primary
        } else {
            Kind::Secondary
        },
        GO,
        json!({ "to": last }),
    ));
    if !compact {
        let mode = if draft.reviewing(ask) {
            format!("{} of {last} answered", draft.count())
        } else if ask.questions[draft.tab].multi_select {
            format!("pick any · {} chosen", draft.picked[draft.tab].len())
        } else {
            "pick one".to_owned()
        };
        row.push(Node::Spacer);
        row.push(Node::Toned {
            text: mode,
            tone: Tone::Neutral,
            mono: true,
        });
    }
    Node::row(row)
}

/// The question in view: its rows, the person's own answer, the preview
/// of the row in focus when its choices have previews, and its note.
fn question(key: &CallKey, ask: &Ask, draft: &Draft, focus: &Focus) -> Node {
    let i = draft.tab;
    let question = &ask.questions[i];
    let previews = !question.multi_select
        && question
            .options
            .iter()
            .any(|choice| choice.preview.is_some());
    let mut pieces = vec![Node::Rich {
        text: format!("**{}**", question.question),
    }];
    for (k, choice) in question.options.iter().enumerate() {
        pieces.push(row(
            draft,
            i,
            k,
            &choice.label,
            (!previews).then_some(&choice.description),
        ));
    }
    let other_row = question.options.len();
    let mut own = vec![digit(other_row, draft.cursor[i] == other_row)];
    own.push(Node::Input {
        id: other_input(key, i),
        placeholder: Some("Your own answer…".into()),
        multiline: false,
        submit: Some(OTHER_DONE.into()),
        keep: true,
        secret: false,
        focus: (focus.other > 0).then_some(focus.other),
    });
    if draft.other_on[i] {
        own.push(Node::Icon {
            name: "check".into(),
            tone: Tone::Good,
        });
    }
    pieces.push(Node::row(own));
    if previews {
        let focused = question.options.get(draft.cursor[i]);
        let mut side = vec![Node::Toned {
            text: focused
                .map_or("Write your own answer.".to_owned(), |choice| {
                    choice.description.clone()
                }),
            tone: Tone::Neutral,
            mono: false,
        }];
        if let Some(preview) = focused.and_then(|choice| choice.preview.clone())
        {
            side.push(Node::Code {
                lang: None,
                text: preview,
                first_line: None,
            });
        }
        pieces.push(Node::Card { children: side });
    }
    if draft.note_open {
        pieces.push(Node::Input {
            id: note_input(key, i),
            placeholder: Some(
                "Anything the agent should know about this answer…".into(),
            ),
            multiline: true,
            submit: Some(NOTE_DONE.into()),
            keep: true,
            secret: false,
            focus: (focus.note > 0).then_some(focus.note),
        });
    } else if !draft.notes[i].trim().is_empty() {
        pieces.push(Node::row([
            Node::Icon {
                name: "pencil".into(),
                tone: Tone::Info,
            },
            Node::Toned {
                text: draft.notes[i].clone(),
                tone: Tone::Neutral,
                mono: false,
            },
        ]));
    }
    Node::stack(pieces)
}

/// Row `k` of question `i`: its key, whether it is picked, and its
/// label with what it means.
fn row(
    draft: &Draft,
    i: usize,
    k: usize,
    label: &str,
    description: Option<&String>,
) -> Node {
    let picked = draft.picked[i].contains(&k);
    let mut cells = vec![
        digit(k, draft.cursor[i] == k),
        button(
            label,
            if picked {
                Kind::Primary
            } else {
                Kind::Secondary
            },
            CHOOSE,
            json!({ "row": k }),
        ),
    ];
    if let Some(description) = description {
        cells.push(Node::Toned {
            text: description.clone(),
            tone: Tone::Neutral,
            mono: false,
        });
    }
    Node::row(cells)
}

/// A row's key, `›` before the row in focus; rows past the ninth take
/// none.
fn digit(k: usize, focused: bool) -> Node {
    let key = if k < 9 {
        (k + 1).to_string()
    } else {
        " ".into()
    };
    Node::Mono {
        text: if focused {
            format!("› {key}")
        } else {
            format!("  {key}")
        },
    }
}

/// The review: each question's answer, with a way back to it.
fn review(ask: &Ask, draft: &Draft) -> Node {
    let rows = ask
        .questions
        .iter()
        .enumerate()
        .map(|(i, question)| {
            let answer = draft.answer(ask, i);
            vec![
                Node::Toned {
                    text: question.header.clone(),
                    tone: Tone::Neutral,
                    mono: false,
                },
                if draft.answered(i) {
                    Node::text(answer.text())
                } else {
                    Node::Toned {
                        text: "not answered".into(),
                        tone: Tone::Warn,
                        mono: false,
                    }
                },
                button("Edit", Kind::Secondary, GO, json!({ "to": i })),
            ]
        })
        .collect();
    Node::Table {
        head: Vec::new(),
        rows,
        widths: vec![1., 4., 1.],
    }
}

/// The foot: the keys, then what the panel can do from here.
fn foot(ask: &Ask, draft: &Draft, sent: bool, compact: bool) -> Node {
    let mut row = Vec::new();
    if !compact {
        let hints = if draft.reviewing(ask) {
            "← back · Enter send"
        } else if ask.questions[draft.tab].multi_select {
            "↑↓ move · Space toggle · n note · ←→ questions"
        } else {
            "↑↓ move · Enter choose · n note · ←→ questions"
        };
        row.push(Node::Toned {
            text: hints.into(),
            tone: Tone::Neutral,
            mono: true,
        });
    }
    row.push(Node::Spacer);
    row.push(button("Cancel run", Kind::Danger, CANCEL, Value::Null));
    if sent {
        row.push(Node::Toned {
            text: "Sending your answers…".into(),
            tone: Tone::Info,
            mono: false,
        });
        return Node::row(row);
    }
    row.push(button(
        "Decline to answer",
        Kind::Secondary,
        DECLINE,
        Value::Null,
    ));
    if draft.reviewing(ask) {
        let left = ask.questions.len() - draft.count();
        let label = if left == 0 {
            "Send answers".to_owned()
        } else {
            format!("Send answers · {left} left")
        };
        let kind = if draft.ready() {
            Kind::Primary
        } else {
            Kind::Secondary
        };
        row.push(button(label, kind, SEND, Value::Null));
    } else {
        let noted = !draft.notes[draft.tab].trim().is_empty();
        row.push(button(
            if noted { "Edit note" } else { "Add note" },
            Kind::Secondary,
            NOTE_OPEN,
            Value::Null,
        ));
        row.push(button("Next", Kind::Primary, NEXT, Value::Null));
    }
    Node::row(row)
}
