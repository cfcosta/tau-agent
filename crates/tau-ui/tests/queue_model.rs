//! The landing queue (ADR 0024) against a model of a repository's main
//! chat: chats finish and ask to land while main works or is idle,
//! main's turns start and end, resolving turns clear what a landing
//! left in conflict or leave it, the person unqueues and dismisses, and
//! tau restarts, its lane rebuilt from what it stored.
//!
//! The model is the world the queue lands in: which chats exist, the
//! files each would conflict in now, the files on main's stack in
//! conflict, and whether main runs. The queue's [`Main`] is that world,
//! and it checks each landing as it happens: in the order the chats
//! were queued, never while main runs or is conflicted, never with a
//! conflict the person did not confirm, and never twice.

use std::collections::BTreeSet;

use hegel::{TestCase, generators as gs};
use tau_ui::host::queue::{
    Drained,
    Lane,
    Main,
    Preview,
    Record,
    Unlandable,
    drain,
};
use tau_ui_remote::queue::Waiting;

const FILES: [&str; 3] = ["a.rs", "b.rs", "c.rs"];

#[derive(Debug, Clone)]
struct Chat {
    id: String,
    /// The files landing it would conflict in now.
    conflicts: BTreeSet<String>,
    landed: bool,
    dropped: bool,
}

/// The repository and its main chat, as the queue sees them.
#[derive(Debug, Default)]
struct World {
    chats: Vec<Chat>,
    busy: bool,
    /// Files main's stack holds in conflict.
    stack: BTreeSet<String>,
    stored: Vec<Record>,
    /// The chats queued and not gone yet, in the order they were
    /// queued: the order they must land in.
    order: Vec<String>,
    /// What the person confirmed for each queued chat.
    confirmed: Vec<(String, BTreeSet<String>)>,
    landed: Vec<String>,
}

impl World {
    fn chat(&mut self, run: &str) -> &mut Chat {
        self.chats.iter_mut().find(|chat| chat.id == run).unwrap()
    }

    fn confirmed(&self, run: &str) -> BTreeSet<String> {
        self.confirmed
            .iter()
            .find(|(id, _)| id == run)
            .map(|(_, files)| files.clone())
            .unwrap_or_default()
    }

    /// `run` left the queue: the model's order follows.
    fn left(&mut self, run: &str) {
        self.order.retain(|id| id != run);
        self.confirmed.retain(|(id, _)| id != run);
    }
}

impl Main for World {
    async fn busy(&self) -> bool {
        self.busy
    }

    async fn conflicts(&mut self) -> anyhow::Result<Vec<String>> {
        Ok(self.stack.iter().cloned().collect())
    }

    async fn preview(&mut self, run: &str) -> Result<Preview, Unlandable> {
        let chat = self.chat(run);
        if chat.landed || chat.dropped {
            return Err(Unlandable::Gone);
        }
        Ok(Preview {
            changes: 1,
            conflicts: chat.conflicts.iter().cloned().collect(),
        })
    }

    async fn land(&mut self, run: &str) -> Result<Vec<String>, Unlandable> {
        assert!(!self.busy, "{run} landed while main ran");
        assert!(
            self.stack.is_empty(),
            "{run} landed on main conflicted in {:?}",
            self.stack
        );
        assert_eq!(
            self.order.first().map(String::as_str),
            Some(run),
            "{run} landed out of order: {:?}",
            self.order
        );
        let confirmed = self.confirmed(run);
        let chat = self.chat(run).clone();
        assert!(!chat.landed, "{run} landed twice");
        assert!(
            chat.conflicts.is_subset(&confirmed),
            "{run} landed with {:?}, confirmed {confirmed:?}",
            chat.conflicts
        );
        self.chat(run).landed = true;
        self.landed.push(run.to_owned());
        self.left(run);
        self.stack.extend(chat.conflicts.iter().cloned());
        // The chats still waiting now meet these changes on main.
        Ok(chat.conflicts.into_iter().collect())
    }

    async fn title(&self, run: &str) -> String {
        format!("chat {run}")
    }

    async fn store(&mut self, record: &Record) -> anyhow::Result<()> {
        // A chat that left without landing leaves the model's order too.
        if let Record::Left { run } = record {
            self.left(run);
        }
        self.stored.push(record.clone());
        Ok(())
    }
}

struct Machine {
    lane: Lane,
    world: World,
    next_id: usize,
    /// A landing left conflicts and the host is about to start tau's
    /// turn resolving them: other events can come first.
    resolving: bool,
}

impl Machine {
    fn files(tc: &TestCase) -> BTreeSet<String> {
        tc.draw(gs::subsequences(FILES.to_vec()))
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// Lands what may land, as the host does after each event, and
    /// starts tau's resolving turn when a landing left conflicts.
    fn drain(&mut self) {
        let drained =
            tau_testing::block_on(drain(&mut self.lane, &mut self.world))
                .unwrap();
        self.after(drained);
        self.stopped_for_a_reason();
    }

    fn perform(&mut self, actions: Vec<tau_ui::host::queue::Action>) {
        let mut drained = Drained::default();
        tau_testing::block_on(drained.perform(&mut self.world, actions))
            .unwrap();
        self.after(drained);
    }

    fn after(&mut self, drained: Drained) {
        if let Some(files) = &drained.notify {
            let marked = self.lane.conflicts().map(|c| c.files.clone());
            assert_eq!(marked.as_ref(), Some(files));
        }
        if drained.resolve.is_some() {
            assert!(!self.world.stack.is_empty());
            assert!(self.lane.is_pending());
            self.resolving = true;
        }
    }

    fn stopped_for_a_reason(&mut self) {
        // The drain stopped for a reason: main busy or conflicted, a
        // landing waiting on tau's turn, or a head whose conflicts the
        // person did not confirm.
        if !self.world.busy
            && !self.lane.is_pending()
            && self.lane.conflicts().is_none()
            && let Some(head) = self.world.order.first().cloned()
        {
            let chat = self.world.chat(&head).clone();
            assert!(
                !chat.landed && !chat.dropped,
                "{head} is gone but still queued"
            );
            assert!(
                !chat.conflicts.is_subset(&self.world.confirmed(&head)),
                "{head} could land but waits"
            );
        }
    }
}

#[hegel::state_machine]
impl Machine {
    /// A chat finishes: landing it would conflict in some files.
    #[rule(weight = 3)]
    fn finish_chat(&mut self, tc: TestCase) {
        let conflicts = Self::files(&tc);
        self.next_id += 1;
        self.world.chats.push(Chat {
            id: format!("c{}", self.next_id),
            conflicts,
            landed: false,
            dropped: false,
        });
    }

    /// The person lands a finished chat, confirming what its preview
    /// shows, or confirms a queued one again with what it shows now.
    #[rule(weight = 4)]
    fn land(&mut self, tc: TestCase) {
        let open: Vec<String> = self
            .world
            .chats
            .iter()
            .filter(|chat| !chat.landed && !chat.dropped)
            .map(|chat| chat.id.clone())
            .collect();
        if open.is_empty() {
            tc.reject();
        }
        let run = tc.draw(gs::sampled_from(open));
        let seen: Vec<String> =
            self.world.chat(&run).conflicts.iter().cloned().collect();
        let known = self.lane.waiting().iter().find(|w| w.run == run).cloned();
        let waiting = match known {
            // Confirmed again: what its last preview in the queue found.
            Some(known) => Waiting {
                confirmed: known.conflicts.clone(),
                ..known
            },
            None => Waiting {
                run: run.clone(),
                title: format!("chat {run}"),
                changes: 1,
                conflicts: seen.clone(),
                confirmed: seen,
                sub_agent: None,
            },
        };
        if !self.world.order.contains(&run) {
            self.world.order.push(run.clone());
        }
        self.world.confirmed.retain(|(id, _)| *id != run);
        self.world
            .confirmed
            .push((run.clone(), waiting.confirmed.iter().cloned().collect()));
        let actions = self.lane.queue(waiting);
        self.perform(actions);
        self.drain();
    }

    /// Main moves, by an update: a chat's landing now conflicts in other
    /// files.
    #[rule(weight = 2)]
    fn conflicts_change(&mut self, tc: TestCase) {
        let open: Vec<String> = self
            .world
            .chats
            .iter()
            .filter(|chat| !chat.landed && !chat.dropped)
            .map(|chat| chat.id.clone())
            .collect();
        if open.is_empty() {
            tc.reject();
        }
        let run = tc.draw(gs::sampled_from(open));
        let files = Self::files(&tc);
        self.world.chat(&run).conflicts = files;
    }

    /// tau's turn resolving what a landing left starts.
    #[rule(weight = 3)]
    fn resolving_turn_starts(&mut self, tc: TestCase) {
        if !self.resolving || self.world.busy {
            tc.reject();
        }
        self.resolving = false;
        self.world.busy = true;
        self.lane.started();
    }

    /// tau's resolving turn could not start: main stays idle, with the
    /// conflicts.
    #[rule]
    fn resolving_turn_fails_to_start(&mut self, tc: TestCase) {
        if !self.resolving || self.world.busy {
            tc.reject();
        }
        self.resolving = false;
        let actions = self
            .lane
            .not_resolving(self.world.stack.iter().cloned().collect());
        self.perform(actions);
        self.drain();
    }

    /// The person writes to main: its turn starts.
    #[rule(weight = 2)]
    fn main_turn_starts(&mut self, tc: TestCase) {
        // tau's resolving turn starts first, as soon as the landing is
        // shown.
        if self.world.busy || self.resolving {
            tc.reject();
        }
        self.world.busy = true;
        self.lane.started();
    }

    /// Main's turn ends: it resolved what was on its stack, or left it.
    #[rule(weight = 3)]
    fn main_turn_ends(&mut self, tc: TestCase) {
        if !self.world.busy {
            tc.reject();
        }
        let resolves = tc.draw(gs::booleans());
        if resolves {
            self.world.stack.clear();
        }
        self.world.busy = false;
        let actions =
            self.lane.ended(self.world.stack.iter().cloned().collect());
        self.perform(actions);
        self.drain();
    }

    #[rule]
    fn unqueue(&mut self, tc: TestCase) {
        if self.world.order.is_empty() {
            tc.reject();
        }
        let run = tc.draw(gs::sampled_from(self.world.order.clone()));
        let actions = self.lane.unqueue(&run);
        self.perform(actions);
        assert!(!self.world.order.contains(&run));
        self.drain();
    }

    /// A queued chat is dropped: it leaves the queue at its turn.
    #[rule]
    fn drop_queued(&mut self, tc: TestCase) {
        if self.world.order.is_empty() {
            tc.reject();
        }
        let run = tc.draw(gs::sampled_from(self.world.order.clone()));
        self.world.chat(&run).dropped = true;
        self.drain();
    }

    #[rule]
    fn dismiss(&mut self, _tc: TestCase) {
        let actions = self.lane.dismiss();
        self.perform(actions);
    }

    /// tau closes and starts again: no turn survives, the lane comes
    /// back from what it stored, and what may land lands.
    #[rule(weight = 2)]
    fn restart(&mut self, _tc: TestCase) {
        let before = self.lane.stored();
        self.world.busy = false;
        self.resolving = false;
        self.lane = Lane::restore(&self.world.stored);
        assert_eq!(self.lane, before, "the lane outlives tau");
        self.drain();
    }

    #[invariant(always_run)]
    fn the_lane_is_the_model(&self, _tc: TestCase) {
        let queued: Vec<&str> =
            self.lane.waiting().iter().map(|w| w.run.as_str()).collect();
        assert_eq!(queued, self.world.order, "the queue's order");
        // What it stored is what it is.
        assert_eq!(Lane::restore(&self.world.stored), self.lane.stored());
        // Idle, main is marked exactly when its stack is conflicted,
        // and a marked main refuses new chats.
        if !self.world.busy && !self.lane.is_pending() {
            let marked = self.lane.conflicts().map(|c| c.files.clone());
            let stack: Vec<String> = self.world.stack.iter().cloned().collect();
            assert_eq!(marked, (!stack.is_empty()).then_some(stack));
        }
        // Until tau's resolving turn has tried, what a landing left is
        // not "still on main".
        if self.resolving {
            assert_eq!(self.lane.conflicts(), None, "marked before tau tried");
        }
        assert_eq!(
            self.lane.refuse_chat().is_some(),
            self.lane.conflicts().is_some()
        );
        let mut landed = self.world.landed.clone();
        landed.sort();
        landed.dedup();
        assert_eq!(
            landed.len(),
            self.world.landed.len(),
            "nothing lands twice"
        );
    }
}

#[hegel::test(test_cases = 300)]
fn the_queue_lands_in_order_and_only_when_it_may(tc: TestCase) {
    let machine = Machine {
        lane: Lane::default(),
        world: World::default(),
        next_id: 0,
        resolving: false,
    };
    hegel::stateful::machine(machine).steps(40).run(tc);
}

/// The message of tau's resolving turn names the chat and the files.
#[test]
fn the_resolving_turn_names_the_chat_and_its_conflicts() {
    assert_eq!(
        tau_ui::host::queue::resolve_prompt(
            "Load AGENTS.md",
            &["a.rs".into(), "b.rs".into(), "c.rs".into()]
        ),
        "Landing `Load AGENTS.md` left conflicts in `a.rs`, `b.rs` and \
         `c.rs`. Resolve them, and commit the resolution."
    );
}

/// A marked main refuses new chats with the files named.
#[test]
fn a_marked_main_refuses_new_chats() {
    let mut lane = Lane::default();
    assert_eq!(lane.refuse_chat(), None);
    let actions = lane.checked(vec!["a.rs".into(), "b.rs".into()]);
    assert_eq!(actions.len(), 2, "stored and told: {actions:?}");
    assert_eq!(
        lane.refuse_chat().as_deref(),
        Some("main has conflicts in a.rs, b.rs; resolve them first")
    );
}

/// A main chat for [`sub_agents_land_whatever_they_bring`]: idle, clean
/// until a landing brings conflicts, every landing recorded.
#[derive(Default)]
struct Plain {
    queued: Vec<Waiting>,
    landed: Vec<String>,
    stack: BTreeSet<String>,
}

impl Main for Plain {
    async fn busy(&self) -> bool {
        false
    }

    async fn conflicts(&mut self) -> anyhow::Result<Vec<String>> {
        Ok(self.stack.iter().cloned().collect())
    }

    async fn preview(&mut self, run: &str) -> Result<Preview, Unlandable> {
        let waiting = self.queued.iter().find(|w| w.run == run).unwrap();
        Ok(Preview {
            changes: 1,
            conflicts: waiting.conflicts.clone(),
        })
    }

    async fn land(&mut self, run: &str) -> Result<Vec<String>, Unlandable> {
        let waiting = self.queued.iter().find(|w| w.run == run).unwrap();
        assert!(
            waiting
                .sub_agent
                .as_ref()
                .is_none_or(|end| end.failed.is_none()),
            "{run} had nothing to land"
        );
        assert!(self.stack.is_empty(), "{run} landed on conflicts");
        self.landed.push(run.to_owned());
        self.stack.extend(waiting.conflicts.iter().cloned());
        Ok(waiting.conflicts.clone())
    }

    async fn title(&self, run: &str) -> String {
        run.to_owned()
    }

    async fn store(&mut self, _record: &Record) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Sub-agents in the queue (ADR 0026) land whatever they conflict in,
/// one with nothing to land is only reported, and the drain reports
/// every sub-agent it took, in order: the turn that reports them holds
/// the queue, as a resolving turn does. Chats keep their rule: they
/// wait for the person to confirm a conflict.
#[hegel::test(test_cases = 300)]
fn sub_agents_land_whatever_they_bring(tc: TestCase) {
    use tau_ui_remote::queue::SubAgentEnd;
    let count: usize = tc.draw(gs::integers().max_value(6));
    let queued: Vec<Waiting> = (0..count)
        .map(|n| {
            let conflicts: Vec<String> = tc
                .draw(gs::subsequences(FILES.to_vec()).max_size(2))
                .into_iter()
                .map(str::to_owned)
                .collect();
            let sub_agent = tc.draw(gs::booleans()).then(|| SubAgentEnd {
                limit: None,
                failed: tc
                    .draw(gs::weighted_booleans(0.3))
                    .then(|| "it failed".to_owned()),
            });
            let confirmed = if tc.draw(gs::booleans()) {
                conflicts.clone()
            } else {
                Vec::new()
            };
            Waiting {
                run: format!("r{n}"),
                title: format!("r{n}"),
                changes: 1,
                conflicts,
                confirmed,
                sub_agent,
            }
        })
        .collect();
    let mut lane = Lane::default();
    let mut main = Plain {
        queued: queued.clone(),
        ..Plain::default()
    };
    for waiting in &queued {
        let _ = lane.queue(waiting.clone());
    }
    let drained = tau_testing::block_on(drain(&mut lane, &mut main)).unwrap();

    // What the queue promises, walked by hand.
    let mut landed = Vec::new();
    let mut reported = Vec::new();
    let mut resolves = false;
    for waiting in &queued {
        match &waiting.sub_agent {
            Some(end) if end.failed.is_some() => {
                reported.push(waiting.run.clone());
                continue;
            }
            Some(_) => {}
            None if waiting
                .conflicts
                .iter()
                .all(|file| waiting.confirmed.contains(file)) => {}
            None => break,
        }
        landed.push(waiting.run.clone());
        if waiting.sub_agent.is_some() {
            reported.push(waiting.run.clone());
        }
        if !waiting.conflicts.is_empty() {
            resolves = true;
            break;
        }
    }
    assert_eq!(main.landed, landed);
    assert_eq!(drained.landed, landed);
    let got: Vec<String> =
        drained.reported.iter().map(|w| w.run.clone()).collect();
    assert_eq!(got, reported);
    assert_eq!(drained.resolve.is_some(), resolves);
    // A turn follows whenever the drain reports or resolves: nothing more
    // lands until it ends.
    assert_eq!(lane.is_pending(), resolves || !reported.is_empty());
    if lane.is_pending() {
        assert!(lane.next().is_none());
    }
}
