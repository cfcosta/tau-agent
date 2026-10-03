//! A main chat's landing queue (ADR 0021): chats wait to land while the
//! main chat works, and land in order once it is idle.
//!
//! [`Lane`] decides, and does nothing itself: each call takes what
//! happened and returns the [`Action`]s it calls for. What must outlive
//! tau goes out as [`Record`]s for the host to store on the main chat
//! (plugin [`QUEUE_PLUGIN`]); [`Lane::restore`] folds them back, so a
//! restart keeps the queue and the conflicts on main. [`drain`] lands
//! what may land now through a [`Main`], which the host implements on
//! the repository and tests implement on a model.

use serde::{Deserialize, Serialize};
use tau_ui_remote::queue::{MainConflicts, Waiting, conflicts_remain};

/// The plugin name the queue's records are stored under, on the main
/// chat.
pub const QUEUE_PLUGIN: &str = "landing-queue";

/// What the queue stores: each changes [`Lane`]'s state as
/// [`Lane::restore`] folds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Record {
    /// A chat joined the queue, at the end, or the person confirmed it
    /// again, in its place.
    Queued(Waiting),
    /// A preview at its turn to land found conflicts the person did not
    /// confirm: it waits for them.
    Previewed {
        run: String,
        changes: usize,
        conflicts: Vec<String>,
    },
    /// It left the queue: unqueued, landed, or gone.
    Left { run: String },
    /// A landing brought conflicts: tau's turn on main resolves them,
    /// started with `prompt`.
    Resolving { from: String, prompt: String },
    /// Main's stack holds these files in conflict after a turn.
    Conflicted { files: Vec<String> },
    /// The person will resolve them themselves: the card goes.
    Dismissed,
    /// Main's stack is clean.
    Clean,
}

/// What a [`Lane`] calls for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Store this record on the main chat.
    Store(Record),
    /// Conflicts are still on main, newly: tell the person.
    Notify(Vec<String>),
}

/// What landing a chat would do now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preview {
    pub changes: usize,
    pub conflicts: Vec<String>,
}

/// Why a queued chat cannot land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unlandable {
    /// Main started a turn meanwhile: the chat keeps its place, and the
    /// queue waits for the turn to end.
    Busy,
    /// It landed or was dropped already: it leaves the queue quietly.
    Gone,
    /// Anything else: it leaves the queue, and its chat says why.
    Failed(String),
}

/// One main chat's queue, and what holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lane {
    waiting: Vec<Waiting>,
    conflicts: Option<MainConflicts>,
    /// The landing whose conflicts tau's turn resolves, and its message.
    resolving: Option<(String, String)>,
    /// Main is in a turn. Not stored: no turn outlives tau.
    busy: bool,
    /// A landing brought conflicts, and tau's turn resolving them has
    /// not ended yet. Not stored.
    pending: bool,
}

impl Lane {
    /// The lane `records` leave, in the order they were stored.
    pub fn restore<'a>(records: impl IntoIterator<Item = &'a Record>) -> Self {
        let mut lane = Self::default();
        for record in records {
            lane.fold(record);
        }
        lane
    }

    /// The chats waiting, in the order they land.
    pub fn waiting(&self) -> &[Waiting] {
        &self.waiting
    }

    /// The conflicts a turn left on main, while it is not clean.
    pub fn conflicts(&self) -> Option<&MainConflicts> {
        self.conflicts.as_ref()
    }

    /// What survives a restart: the queue, the mark of conflicts, and
    /// what resolves them.
    pub fn stored(&self) -> Self {
        Self {
            busy: false,
            pending: false,
            ..self.clone()
        }
    }

    /// Main is in a turn: nothing lands until it ends.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// A landing's conflicts wait for tau's turn to resolve them.
    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// Why a new chat may not fork main now: it would start from
    /// conflicted code.
    pub fn refuse_chat(&self) -> Option<String> {
        self.conflicts.as_ref().map(|conflicts| {
            format!(
                "main has conflicts in {}; resolve them first",
                conflicts.files.join(", ")
            )
        })
    }

    fn fold(&mut self, record: &Record) {
        match record {
            Record::Queued(waiting) => {
                match self.waiting.iter_mut().find(|w| w.run == waiting.run) {
                    Some(known) => *known = waiting.clone(),
                    None => self.waiting.push(waiting.clone()),
                }
            }
            Record::Previewed {
                run,
                changes,
                conflicts,
            } => {
                if let Some(known) =
                    self.waiting.iter_mut().find(|w| w.run == *run)
                {
                    known.changes = *changes;
                    known.conflicts = conflicts.clone();
                }
            }
            Record::Left { run } => self.waiting.retain(|w| w.run != *run),
            Record::Resolving { from, prompt } => {
                self.resolving = Some((from.clone(), prompt.clone()));
            }
            Record::Conflicted { files } => {
                let (from, prompt) = match &self.resolving {
                    Some((from, prompt)) => {
                        (Some(from.clone()), prompt.clone())
                    }
                    None => (None, conflicts_remain(files)),
                };
                let dismissed =
                    self.conflicts.as_ref().is_some_and(|c| c.dismissed);
                self.conflicts = Some(MainConflicts {
                    files: files.clone(),
                    from,
                    prompt,
                    dismissed,
                });
            }
            Record::Dismissed => {
                if let Some(conflicts) = &mut self.conflicts {
                    conflicts.dismissed = true;
                }
            }
            Record::Clean => {
                self.conflicts = None;
                self.resolving = None;
            }
        }
    }

    /// Changes the lane as `record` says, and stores it.
    fn store(&mut self, record: Record) -> Action {
        self.fold(&record);
        Action::Store(record)
    }

    /// The person asked to land `waiting.run`: it joins the queue at the
    /// end, or, queued already, takes the conflicts they confirmed now
    /// in its place.
    pub fn queue(&mut self, waiting: Waiting) -> Vec<Action> {
        vec![self.store(Record::Queued(waiting))]
    }

    /// The person took `run` out of the queue.
    pub fn unqueue(&mut self, run: &str) -> Vec<Action> {
        if !self.waiting.iter().any(|w| w.run == run) {
            return Vec::new();
        }
        vec![self.store(Record::Left {
            run: run.to_owned(),
        })]
    }

    /// Main started a turn.
    pub fn started(&mut self) {
        self.busy = true;
    }

    /// Main's turn ended, leaving `conflicts` on its stack.
    pub fn ended(&mut self, conflicts: Vec<String>) -> Vec<Action> {
        self.busy = false;
        self.pending = false;
        self.checked(conflicts)
    }

    /// Main's stack, idle, holds `files` in conflict: marked when it
    /// does, clean when not. Ignored while a turn runs or waits to
    /// resolve a landing.
    pub fn checked(&mut self, files: Vec<String>) -> Vec<Action> {
        if self.busy || self.pending {
            return Vec::new();
        }
        match (&self.conflicts, files.is_empty()) {
            (None, true) => Vec::new(),
            (Some(_), true) => vec![self.store(Record::Clean)],
            (Some(known), false) if known.files == files => Vec::new(),
            (known, false) => {
                let new = known.is_none();
                let mut actions = vec![self.store(Record::Conflicted {
                    files: files.clone(),
                })];
                if new {
                    actions.push(Action::Notify(files));
                }
                actions
            }
        }
    }

    /// The person will resolve main's conflicts themselves.
    pub fn dismiss(&mut self) -> Vec<Action> {
        match &self.conflicts {
            Some(conflicts) if !conflicts.dismissed => {
                vec![self.store(Record::Dismissed)]
            }
            _ => Vec::new(),
        }
    }

    /// The chat whose turn to land it is now, if any may land: main
    /// idle, no landing's conflicts waiting on tau's turn, main clean.
    pub fn next(&self) -> Option<&Waiting> {
        if self.busy || self.pending || self.conflicts.is_some() {
            return None;
        }
        self.waiting.first()
    }

    /// What landing the chat [`Self::next`] named would do now. Whether
    /// it lands: when it conflicts only where the person confirmed.
    /// Otherwise it waits for them, and the queue with it.
    pub fn previewed(
        &mut self,
        run: &str,
        preview: &Preview,
    ) -> (bool, Vec<Action>) {
        let Some(head) = self.waiting.first().filter(|w| w.run == run) else {
            return (false, Vec::new());
        };
        let lands = preview
            .conflicts
            .iter()
            .all(|file| head.confirmed.contains(file));
        let changed = head.changes != preview.changes
            || head.conflicts != preview.conflicts;
        let actions = if changed {
            vec![self.store(Record::Previewed {
                run: run.to_owned(),
                changes: preview.changes,
                conflicts: preview.conflicts.clone(),
            })]
        } else {
            Vec::new()
        };
        (lands, actions)
    }

    /// `run` landed, leaving `conflicts`: it leaves the queue, and when
    /// it brought conflicts, tau's turn resolves them with `prompt`
    /// before anything else lands.
    pub fn landed(
        &mut self,
        run: &str,
        conflicts: &[String],
        prompt: String,
    ) -> Vec<Action> {
        let mut actions = vec![self.store(Record::Left {
            run: run.to_owned(),
        })];
        if !conflicts.is_empty() {
            self.pending = true;
            actions.push(self.store(Record::Resolving {
                from: run.to_owned(),
                prompt,
            }));
        }
        actions
    }

    /// tau's resolving turn did not start: main is idle again, with
    /// what the landing left on it.
    pub fn not_resolving(&mut self, conflicts: Vec<String>) -> Vec<Action> {
        self.pending = false;
        self.checked(conflicts)
    }
}

/// What tau's turn on main is told after a landing left conflicts.
pub fn resolve_prompt(title: &str, conflicts: &[String]) -> String {
    let quoted: Vec<String> =
        conflicts.iter().map(|path| format!("`{path}`")).collect();
    let list = match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    format!(
        "Landing `{title}` left conflicts in {list}. Resolve them, and \
         commit the resolution."
    )
}

/// The main chat a [`drain`] lands on.
pub trait Main {
    /// Main is in a turn now.
    fn busy(&self) -> bool;
    /// The files main's stack holds in conflict, after catching up with
    /// trunk.
    fn conflicts(&mut self) -> anyhow::Result<Vec<String>>;
    /// What landing `run` would do now.
    fn preview(&mut self, run: &str) -> Result<Preview, Unlandable>;
    /// Lands `run`; returns the files it left in conflict.
    fn land(&mut self, run: &str) -> Result<Vec<String>, Unlandable>;
    /// `run`'s title, for the resolving turn's message.
    fn title(&self, run: &str) -> String;
    /// Stores `record` on the main chat.
    fn store(&mut self, record: &Record) -> anyhow::Result<()>;
}

/// What a [`drain`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drained {
    /// The chats that landed, in order.
    pub landed: Vec<String>,
    /// The chats that could not, and why: they left the queue.
    pub failed: Vec<(String, String)>,
    /// The message of tau's turn resolving what the last landing left.
    pub resolve: Option<String>,
    /// Conflicts newly marked on main, to tell the person of.
    pub notify: Option<Vec<String>>,
}

impl Drained {
    /// Carries `actions` out on `main`.
    pub fn perform(
        &mut self,
        main: &mut impl Main,
        actions: Vec<Action>,
    ) -> anyhow::Result<()> {
        for action in actions {
            match action {
                Action::Store(record) => main.store(&record)?,
                Action::Notify(files) => self.notify = Some(files),
            }
        }
        Ok(())
    }
}

/// Lands what may land on `main` now, in order, one at a time, each
/// previewed again first. Stops at a chat whose conflicts the person
/// did not confirm, after a landing that leaves conflicts (tau's turn
/// resolves them first), and while main is busy or conflicted.
pub fn drain(lane: &mut Lane, main: &mut impl Main) -> anyhow::Result<Drained> {
    let mut drained = Drained::default();
    if main.busy() {
        lane.started();
    }
    if lane.is_busy() || lane.is_pending() {
        return Ok(drained);
    }
    // Main's stack as it is now, after catching up: marked when a
    // landing's conflicts outlived tau, clean once they are resolved.
    let files = main.conflicts()?;
    let actions = lane.checked(files);
    drained.perform(main, actions)?;
    while let Some(head) = lane.next() {
        let run = head.run.clone();
        let preview = match main.preview(&run) {
            Ok(preview) => preview,
            Err(Unlandable::Busy) => {
                lane.started();
                break;
            }
            Err(why) => {
                drained.leave(lane, main, &run, why)?;
                continue;
            }
        };
        let (lands, actions) = lane.previewed(&run, &preview);
        drained.perform(main, actions)?;
        if !lands {
            break;
        }
        match main.land(&run) {
            Ok(conflicts) => {
                let prompt = resolve_prompt(&main.title(&run), &conflicts);
                let actions = lane.landed(&run, &conflicts, prompt.clone());
                drained.perform(main, actions)?;
                drained.landed.push(run);
                if !conflicts.is_empty() {
                    drained.resolve = Some(prompt);
                    break;
                }
            }
            Err(Unlandable::Busy) => {
                lane.started();
                break;
            }
            Err(why) => drained.leave(lane, main, &run, why)?,
        }
    }
    Ok(drained)
}

impl Drained {
    fn leave(
        &mut self,
        lane: &mut Lane,
        main: &mut impl Main,
        run: &str,
        why: Unlandable,
    ) -> anyhow::Result<()> {
        if let Unlandable::Failed(error) = why {
            self.failed.push((run.to_owned(), error));
        }
        let actions = lane.unqueue(run);
        self.perform(main, actions)
    }
}
