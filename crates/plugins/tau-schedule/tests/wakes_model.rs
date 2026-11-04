//! Chats' wake-ups (ADR 0039) against a model, with tau closing in the
//! middle of waking a chat: chats schedule and cancel, time passes,
//! chats get busy, idle or end, and tau starts again on what it kept,
//! cleanly or cut off before or after the woken chat's turn started.
//!
//! The model is what each chat has waiting, and how often each wake-up
//! woke its chat: the turns the conversations started are what outlives
//! tau, as a chat's stored messages do. A wake-up wakes its chat at
//! least once (ADR 0039): never missed, once when tau stays up, and at
//! most once more for each time tau closed after its turn started and
//! before it kept that the wake-up went.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex},
};

use hegel::{TestCase, generators as gs};
use tau_schedule::{
    Plan,
    Schedules,
    Woken,
    clock::{Clock as _, FakeClock},
    host::Env,
};
use tau_testing::block_on_io;
use tau_ui_plugin::{Conversations, Turn, Waking, registry::HostFuture};
use tokio::sync::oneshot;

/// 2026-10-12T08:00:00Z, in UTC.
const NOW: i64 = 1_791_792_000;
const CHATS: [&str; 2] = ["chat-a", "chat-b"];
const NAMES: [&str; 3] = ["recheck", "merge", "ping"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chat {
    Idle,
    Busy,
    Gone,
}

/// Where tau closes as it wakes a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// Before the chat's turn started.
    Before,
    /// After the turn started on the message, before tau heard so.
    After,
}

/// The host's conversations: each chat idle, busy or gone, and the
/// turns that woke them, which outlive tau.
#[derive(Default)]
struct Chats {
    states: Mutex<BTreeMap<String, Chat>>,
    turns: Mutex<Vec<(String, String)>>,
    /// tau closes at the next wake, here, and says so.
    cut: Mutex<Option<(Cut, oneshot::Sender<()>)>>,
}

impl Conversations for Chats {
    fn list_agent(&self, _name: &str) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn main_of(&self, _repo: &str) -> Option<String> {
        None
    }

    fn running(&self, _repo: &str) -> bool {
        false
    }

    fn turn(&self, _turn: Turn) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn wake(
        &self,
        run: &str,
        text: String,
    ) -> HostFuture<'_, anyhow::Result<Waking>> {
        let state = self
            .states
            .lock()
            .unwrap()
            .get(run)
            .copied()
            .unwrap_or(Chat::Idle);
        if let Some((cut, closed)) = self.cut.lock().unwrap().take() {
            if cut == Cut::After && state == Chat::Idle {
                self.turns.lock().unwrap().push((run.to_owned(), text));
            }
            let _ = closed.send(());
            // tau is gone: nothing answers.
            return Box::pin(std::future::pending());
        }
        let waking = match state {
            Chat::Busy => Waking::Busy,
            Chat::Gone => Waking::Gone,
            Chat::Idle => {
                self.turns.lock().unwrap().push((run.to_owned(), text));
                Waking::Started
            }
        };
        Box::pin(async move { Ok(waking) })
    }
}

struct Host(Arc<Chats>);

impl Env for Host {
    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        Some(self.0.clone())
    }

    fn refresh(&self) {}
}

fn open(home: &Path, clock: &Arc<FakeClock>, chats: &Arc<Chats>) -> Schedules {
    block_on_io(Schedules::open(
        home.join("wakes.json"),
        home.join("agents"),
        Arc::new(Host(chats.clone())),
        clock.clone(),
    ))
}

/// A wake-up as the model keeps it: its chat, when it comes due, and
/// its note, which no other wake-up has.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Waiting {
    at: i64,
    note: String,
}

struct Machine {
    home: tempfile::TempDir,
    clock: Arc<FakeClock>,
    chats: Arc<Chats>,
    schedules: Schedules,
    /// What each chat has waiting, by name.
    waiting: BTreeMap<(String, String), Waiting>,
    states: BTreeMap<String, Chat>,
    /// The wake-ups that woke their chat as tau closed, and how many
    /// times each: tau wakes them again as it starts.
    again: BTreeMap<String, usize>,
    next: usize,
}

impl Machine {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new(NOW, 0));
        let chats = Arc::new(Chats::default());
        let schedules = open(home.path(), &clock, &chats);
        Self {
            home,
            clock,
            chats,
            schedules,
            waiting: BTreeMap::new(),
            states: CHATS
                .iter()
                .map(|chat| ((*chat).to_owned(), Chat::Idle))
                .collect(),
            again: BTreeMap::new(),
            next: 0,
        }
    }

    /// tau closes as it wakes a chat, at `cut`, and starts again.
    fn close_in_tick(&mut self, cut: Cut) {
        let (closing, closed) = oneshot::channel();
        *self.chats.cut.lock().unwrap() = Some((cut, closing));
        let before = self.fired();
        block_on_io(async {
            tokio::select! {
                () = self.schedules.tick() => {}
                _ = closed => {}
            }
        });
        // Nothing came due: nothing woke, and tau closes all the same.
        self.chats.cut.lock().unwrap().take();
        // What the cut-off turn took has woken its chat.
        let fired = self.fired();
        let woke = |note: &String| {
            fired.get(note).copied().unwrap_or(0)
                != before.get(note).copied().unwrap_or(0)
        };
        // Still waiting: the next start wakes the chat again.
        for wake in self.waiting.values().filter(|wake| woke(&wake.note)) {
            *self.again.entry(wake.note.clone()).or_default() += 1;
        }
        self.reopen();
    }

    /// Each wake-up woke its chat once, and once more at most for each
    /// time tau closed in its turn; what waits is the model's.
    fn check(&self) {
        for (note, times) in self.fired() {
            let most = 1 + self.again.get(&note).copied().unwrap_or(0);
            assert!(times <= most, "{note:?} woke its chat {times} times");
        }
        let kept: BTreeSet<(String, String, i64)> =
            block_on_io(self.schedules.overview())
                .wakes
                .into_iter()
                .map(|wake| (wake.run, wake.note, wake.at))
                .collect();
        let model: BTreeSet<(String, String, i64)> = self
            .waiting
            .iter()
            .map(|((chat, _), wake)| (chat.clone(), wake.note.clone(), wake.at))
            .collect();
        assert_eq!(kept, model, "what chats have waiting");
    }

    /// How many turns woke on each note.
    fn fired(&self) -> BTreeMap<String, usize> {
        let mut fired = BTreeMap::new();
        for (_, text) in self.chats.turns.lock().unwrap().iter() {
            for woken in Woken::parse(text).unwrap() {
                *fired.entry(woken.note).or_default() += 1;
            }
        }
        fired
    }

    /// What goes now, in the model: each idle chat's wake-ups that came
    /// due, and every one of a chat that ended once one of them came
    /// due.
    fn due(&self) -> Vec<(String, String)> {
        let now = self.clock.now();
        let came_due = |chat: &str| {
            self.waiting
                .iter()
                .any(|((of, _), wake)| of == chat && wake.at <= now)
        };
        self.waiting
            .iter()
            .filter(|((chat, _), wake)| match self.states[chat] {
                Chat::Idle => wake.at <= now,
                Chat::Gone => came_due(chat),
                Chat::Busy => false,
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// tau closes and opens what it kept.
    fn reopen(&mut self) {
        self.schedules = open(self.home.path(), &self.clock, &self.chats);
    }
}

#[hegel::state_machine]
impl Machine {
    /// A chat schedules a wake-up, maybe in place of one of the same
    /// name, a minute to three hours ahead.
    #[rule(weight = 3)]
    fn schedule(&mut self, tc: TestCase) {
        let chat = tc.draw(gs::sampled_from(CHATS.to_vec()));
        let name = tc.draw(gs::sampled_from(NAMES.to_vec()));
        let minutes: i64 = tc.draw(gs::integers().min_value(1).max_value(180));
        if self.states[chat] == Chat::Gone {
            tc.reject();
        }
        self.next += 1;
        let note = format!("note {}", self.next);
        let plan: Plan = serde_json::from_value(serde_json::json!({
            "name": name,
            "note": note,
            "in": format!("{minutes}m"),
        }))
        .unwrap();
        block_on_io(self.schedules.schedule(chat, plan)).unwrap();
        self.waiting.insert(
            (chat.to_owned(), name.to_owned()),
            Waiting {
                at: self.clock.now() + minutes * 60,
                note,
            },
        );
    }

    #[rule]
    fn cancel(&mut self, tc: TestCase) {
        let chat = tc.draw(gs::sampled_from(CHATS.to_vec()));
        let name = tc.draw(gs::sampled_from(NAMES.to_vec()));
        let had = self
            .waiting
            .remove(&(chat.to_owned(), name.to_owned()))
            .is_some();
        assert_eq!(block_on_io(self.schedules.unschedule(chat, name)), had);
    }

    #[rule(weight = 2)]
    fn time_passes(&mut self, tc: TestCase) {
        let minutes: i64 = tc.draw(gs::integers().min_value(1).max_value(120));
        self.clock.advance(minutes * 60);
    }

    /// A chat starts a turn, ends it, or lands for good.
    #[rule]
    fn a_chat_changes(&mut self, tc: TestCase) {
        let chat = tc.draw(gs::sampled_from(CHATS.to_vec()));
        if self.states[chat] == Chat::Gone {
            tc.reject();
        }
        let state = [Chat::Idle, Chat::Busy, Chat::Gone]
            [tc.draw(gs::integers::<usize>().max_value(2))];
        self.states.insert(chat.to_owned(), state);
        self.chats
            .states
            .lock()
            .unwrap()
            .insert(chat.to_owned(), state);
    }

    /// The host's tick: what came due wakes its chat once.
    #[rule(weight = 3)]
    fn tick(&mut self, _tc: TestCase) {
        let due = self.due();
        let before = self.fired();
        block_on_io(self.schedules.tick());
        let fired = self.fired();
        for key in due {
            let wake = self.waiting.remove(&key).unwrap();
            let woke = fired.get(&wake.note).copied().unwrap_or(0)
                - before.get(&wake.note).copied().unwrap_or(0);
            let want = usize::from(self.states[&key.0] == Chat::Idle);
            assert_eq!(woke, want, "{key:?} woke {woke} times");
        }
    }

    /// tau closes as it wakes a chat, before or after the chat's turn
    /// started, and starts again.
    #[rule(weight = 2)]
    fn crash(&mut self, tc: TestCase) {
        let cut = [Cut::Before, Cut::After]
            [tc.draw(gs::integers::<usize>().max_value(1))];
        tc.note(&format!("tau closes {cut:?} the turn"));
        self.close_in_tick(cut);
    }

    /// tau closes between ticks, and starts again.
    #[rule]
    fn restart(&mut self, _tc: TestCase) {
        self.reopen();
    }

    #[invariant(always_run)]
    fn each_wake_up_wakes_its_chat_once_and_waits_until_then(
        &mut self,
        _tc: TestCase,
    ) {
        self.check();
    }
}

fn run_machine(tc: TestCase, steps: i64) {
    hegel::stateful::machine(Machine::new())
        .steps(steps)
        .run(tc);
}

#[hegel::test(test_cases = 200)]
fn a_wake_up_wakes_its_chat_at_least_once_whatever_tau_closed_in(tc: TestCase) {
    run_machine(tc, 30);
}

#[ignore = "nightly"]
#[hegel::test(profile = "nightly")]
fn a_wake_up_wakes_its_chat_at_least_once_whatever_tau_closed_in_nightly(
    tc: TestCase,
) {
    run_machine(tc, 60);
}

/// tau closed while it woke a chat (found by
/// `a_wake_up_wakes_its_chat_at_least_once_whatever_tau_closed_in`):
/// before the chat's turn started, the next start wakes it once; after,
/// but before tau kept that the wake-up went, the next start wakes it
/// once more, on the same note, and then it is gone. Wake-ups are at
/// least once (ADR 0039).
#[test]
fn a_wake_up_cut_off_by_tau_closing_wakes_its_chat_again() {
    for (cut, times) in [(Cut::Before, 1), (Cut::After, 2)] {
        let mut machine = Machine::new();
        let plan: Plan = serde_json::from_value(serde_json::json!({
            "name": "recheck",
            "note": "note 1",
            "in": "1m",
        }))
        .unwrap();
        block_on_io(machine.schedules.schedule("chat-a", plan)).unwrap();
        machine.clock.advance(60);
        machine.close_in_tick(cut);
        block_on_io(machine.schedules.tick());
        assert_eq!(machine.fired().get("note 1"), Some(&times), "{cut:?}");
        assert!(
            block_on_io(machine.schedules.overview()).wakes.is_empty(),
            "{cut:?}: it went once its chat woke"
        );
        block_on_io(machine.schedules.tick());
        assert_eq!(machine.fired().get("note 1"), Some(&times), "{cut:?}");
    }
}
