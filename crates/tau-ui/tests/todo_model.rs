//! tau-todo in the real host (ADR 0048, "3. Claims and runs" and "4.
//! Dispatch"): the computer's window over the host, with every plugin's
//! host half, a phone, and a model that answers only when the test says,
//! so runs stay live as long as it wants. Runs claim, hold several
//! claims, set steps, end, are stopped, ask to hand an issue on; tau
//! closes and starts again.
//!
//! - Every claim is held by a run going on, and an issue by one at most.
//! - When a claim ends, its issue is open again, with a comment holding
//!   the run's steps not done.
//! - Claims are linearizable: of runs claiming one issue at once, one
//!   wins, whichever order the store took them in.
//! - After a restart, the claims of the runs it cut off are ended.
//! - An allowed dispatch starts exactly one chat, with the issue held,
//!   however many devices allowed it; a denied one starts nothing.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/sync.rs"]
mod sync;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use hegel::{HealthCheck, TestCase, generators as gs};
use serde_json::{Value, json};
use sync::{Keep, Net, Reply, Who};
use tau_agent::tool::RunId;
use tau_luau_plugins_host::store::{Fuse, Kv};
use tau_testing::block_on_io;
use tau_ui_remote::{
    WorkspaceEvent,
    view::{Item, ToolState},
};

/// The repository the issues are kept in.
const REPO: &str = "repo";
/// tau-luau-plugins, whose host half runs tau-todo.
const LUAU: &str = "tau-luau-plugins";

/// A chat the test started: what its first message holds, and its run.
#[derive(Debug, Clone)]
struct Chat {
    mark: String,
    run: RunId,
}

struct Todo {
    net: Net,
    store: Arc<Kv>,
    issues: Vec<String>,
    /// The chats going on: each waits for the model.
    chats: Vec<Chat>,
    /// Who holds each issue, as the model has it.
    claims: BTreeMap<String, RunId>,
    /// The steps each chat set last.
    steps: BTreeMap<RunId, Vec<String>>,
    next: usize,
    /// The issues handed on once: a chat started on one is told apart
    /// by its first message.
    dispatched: Vec<String>,
}

fn issue(id: &str) -> Value {
    json!({
        "id": id, "title": format!("work on {id}"), "description": "",
        "criteria": [], "type": "task", "priority": 2, "status": "open",
        "labels": [], "comments": [], "links": [],
        "created": 1_793_700_000, "updated": 1_793_700_000,
    })
}

impl Todo {
    fn new() -> Self {
        tau::install();
        let net = Net::new(1, Keep::Disk);
        let path: PathBuf = net
            .dir()
            .join("plugins")
            .join(LUAU)
            .join("stores")
            .join(tau_luau_plugins::names::TODO)
            .join(format!("{REPO}.db"));
        let store =
            Arc::new(block_on_io(Kv::open(&path, Fuse::never())).unwrap());
        let issues: Vec<String> =
            (1..=3).map(|n| format!("td-000{n}")).collect();
        block_on_io(async {
            let mut session = store.session().await;
            for id in &issues {
                session.put(&format!("issue/{id}"), issue(id));
            }
            session.commit().await.unwrap();
        });
        Self {
            net,
            store,
            issues,
            chats: Vec::new(),
            claims: BTreeMap::new(),
            steps: BTreeMap::new(),
            next: 0,
            dispatched: Vec::new(),
        }
    }

    /// Each issue's claim, as the store keeps it.
    fn held(&self) -> BTreeMap<String, Value> {
        block_on_io(self.store.everything())
            .unwrap()
            .into_iter()
            .filter_map(|(key, value)| {
                let id = key.strip_prefix("issue/")?.to_owned();
                Some((id, value))
            })
            .collect()
    }

    /// The run whose conversation starts with `mark`.
    fn run_of(&mut self, mark: &str) -> Option<RunId> {
        self.net.computer.runs().into_iter().find_map(|run| {
            let first = run.items.iter().find_map(|item| match item {
                Item::User(text) => Some(text.clone()),
                _ => None,
            })?;
            first.contains(mark).then_some(run.id)
        })
    }

    /// The last call of `tool` the run `run` made: its id, and why it
    /// failed, if it did.
    fn last_call(
        &mut self,
        run: &RunId,
        tool: &str,
    ) -> Option<(String, Option<String>)> {
        let view = self
            .net
            .computer
            .runs()
            .into_iter()
            .find(|view| &view.id == run)?;
        view.items.iter().rev().find_map(|item| match item {
            Item::Tool(card) if card.tool == tool => Some((
                card.call_id.clone(),
                match &card.state {
                    ToolState::Failed(why) => Some(why.clone()),
                    _ => None,
                },
            )),
            _ => None,
        })
    }

    /// The id of the `todo_dispatch` call of `run` that waits for the
    /// person, once its Allow card shows: the run's state holds the ask.
    /// The card runs before that, while the tool's hook does, and an
    /// answer then finds nothing to take.
    fn asking(&mut self, run: &RunId) -> String {
        let deadline = std::time::Instant::now() + sync::SETTLE;
        let mut states = String::new();
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "the card never asked: {states}"
            );
            self.net.computer.cx.run_until_parked();
            let view = self
                .net
                .computer
                .runs()
                .into_iter()
                .find(|view| &view.id == run);
            if let Some(view) = &view {
                states = format!(
                    "{:?}",
                    view.plugin_states
                        .iter()
                        .map(|(name, state)| (name, state.json()))
                        .collect::<Vec<_>>()
                );
            }
            let waiting = view.and_then(|view| {
                let asks = &view.plugin_states.get(LUAU)?.json()["asks"];
                view.items.iter().rev().find_map(|item| match item {
                    Item::Tool(card)
                        if card.tool == "todo_dispatch"
                            && matches!(card.state, ToolState::Running)
                            && asks[&card.call_id]["outcome"] == "waiting" =>
                    {
                        Some(card.call_id.clone())
                    }
                    _ => None,
                })
            });
            if let Some(call) = waiting {
                return call;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn pick_chat(&self, tc: &TestCase) -> Option<Chat> {
        if self.chats.is_empty() {
            return None;
        }
        let at = tc.draw(gs::integers().max_value(self.chats.len() - 1));
        Some(self.chats[at].clone())
    }

    fn pick_issue(&self, tc: &TestCase) -> String {
        tc.draw(gs::sampled_from(self.issues.clone()))
    }

    fn answer(&mut self, chat: &Chat, reply: Reply) {
        assert!(
            self.net.gate.answer_for(&chat.mark, reply),
            "{} waits for the model",
            chat.mark
        );
    }

    /// What ending `run`'s claims leaves: each issue open, not held, with
    /// a comment naming the run and the steps it left.
    fn ended(&mut self, run: &RunId) {
        let held: Vec<String> = self
            .claims
            .iter()
            .filter(|(_, by)| *by == run)
            .map(|(id, _)| id.clone())
            .collect();
        let kept = self.held();
        for id in held {
            self.claims.remove(&id);
            let issue = &kept[&id];
            assert_eq!(issue["status"], json!("open"), "{id} after {run:?}");
            let last = issue["comments"]
                .as_array()
                .and_then(|comments| comments.last())
                .and_then(|comment| comment["text"].as_str())
                .unwrap_or_default()
                .to_owned();
            assert!(last.contains(run.0.as_ref()), "{id}: {last}");
            for step in self.steps.get(run).into_iter().flatten() {
                assert!(
                    last.contains(&format!("- {step}")),
                    "{id} keeps {step}: {last}"
                );
            }
        }
        self.steps.remove(run);
    }

    fn check(&mut self) {
        let kept = self.held();
        let live: Vec<RunId> =
            self.chats.iter().map(|chat| chat.run.clone()).collect();
        for (id, issue) in &kept {
            let claim = issue["claim"].as_str();
            if let Some(claim) = claim {
                assert!(
                    live.iter().any(|run| run.0.as_ref() == claim),
                    "{id} is held by {claim}, which is not going on"
                );
            }
            assert_eq!(
                claim,
                self.claims.get(id).map(|run| run.0.as_ref()),
                "who holds {id}"
            );
        }
    }
}

#[hegel::state_machine]
impl Todo {
    /// The person starts a chat in the repository.
    #[rule(weight = 3)]
    fn start_chat(&mut self, _tc: TestCase) {
        if self.chats.len() >= 4 {
            return;
        }
        self.next += 1;
        let mark = format!("«chat {}»", self.next);
        let device = &mut self.net.computer;
        let model = device
            .workspace
            .read_with(&device.cx, |ws, _| ws.next_model().clone());
        let prompt = format!("{mark} work");
        device.workspace.update(&mut device.cx, |_, cx| {
            cx.emit(WorkspaceEvent::NewRun {
                prompt,
                model,
                repo: REPO.into(),
                images: Vec::new(),
            })
        });
        self.net.host_settles();
        let run = self.run_of(&mark).expect("the chat started");
        self.chats.push(Chat { mark, run });
        self.check();
    }

    /// A chat claims an issue: it wins when no run holds it.
    #[rule(weight = 4)]
    fn claim(&mut self, tc: TestCase) {
        let Some(chat) = self.pick_chat(&tc) else {
            return;
        };
        let id = self.pick_issue(&tc);
        self.answer(
            &chat,
            Reply::Tool {
                name: "todo_claim".into(),
                args: json!({ "id": id }),
            },
        );
        self.net.host_settles();
        let free = !self.claims.contains_key(&id);
        let (_, failed) = self.last_call(&chat.run, "todo_claim").unwrap();
        match self.claims.get(&id) {
            None => {
                assert_eq!(
                    failed, None,
                    "{} could not claim {id}, free",
                    chat.mark
                );
                self.claims.insert(id, chat.run.clone());
            }
            Some(holder) if *holder == chat.run => assert_eq!(failed, None),
            Some(_) => {
                assert!(
                    failed.is_some() && !free,
                    "{} claimed {id}, held",
                    chat.mark
                )
            }
        }
        self.check();
    }

    /// Two or three chats claim one issue at once: one wins, the store
    /// taking them one at a time, and the others are told who holds it.
    #[rule(weight = 2)]
    fn claim_at_once(&mut self, tc: TestCase) {
        if self.chats.len() < 2 {
            return;
        }
        let id = self.pick_issue(&tc);
        let n: usize = tc.draw(
            gs::integers()
                .min_value(2)
                .max_value(self.chats.len().min(3)),
        );
        let contenders: Vec<Chat> = self.chats[..n].to_vec();
        for chat in &contenders {
            self.answer(
                chat,
                Reply::Tool {
                    name: "todo_claim".into(),
                    args: json!({ "id": id }),
                },
            );
        }
        self.net.host_settles();
        let before = self.claims.get(&id).cloned();
        let kept = self.held();
        let holder = kept[&id]["claim"].as_str().map(|run| RunId(run.into()));
        let mut won = Vec::new();
        for chat in &contenders {
            let (_, failed) = self.last_call(&chat.run, "todo_claim").unwrap();
            if failed.is_none() {
                won.push(chat.run.clone());
            }
        }
        match before {
            // One of them held it already: it keeps it, and only its call
            // goes through.
            Some(held) if contenders.iter().any(|chat| chat.run == held) => {
                assert_eq!(won, std::slice::from_ref(&held));
                assert_eq!(holder, Some(held));
            }
            Some(held) => {
                assert!(
                    won.is_empty(),
                    "{won:?} claimed {id}, held by {held:?}"
                );
                assert_eq!(holder, Some(held));
            }
            None => {
                assert_eq!(won.len(), 1, "exactly one wins {id}: {won:?}");
                assert_eq!(holder.as_ref(), Some(&won[0]));
                self.claims.insert(id, won[0].clone());
            }
        }
        self.check();
    }

    /// A chat sets its steps.
    #[rule(weight = 2)]
    fn set_steps(&mut self, tc: TestCase) {
        let Some(chat) = self.pick_chat(&tc) else {
            return;
        };
        let n = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
        let steps: Vec<String> = (0..n)
            .map(|at| format!("step {at} of {}", chat.mark))
            .collect();
        let args = json!({ "steps": steps.iter().enumerate().map(|(at, text)| json!({
            "text": text,
            "status": if at == 0 { "in_progress" } else { "pending" },
        })).collect::<Vec<_>>() });
        self.answer(
            &chat,
            Reply::Tool {
                name: "todo_steps".into(),
                args,
            },
        );
        self.net.host_settles();
        self.steps.insert(chat.run.clone(), steps);
        self.check();
    }

    /// A chat's turn ends: what it held is open again, its steps said.
    #[rule(weight = 2)]
    fn finish(&mut self, tc: TestCase) {
        let Some(chat) = self.pick_chat(&tc) else {
            return;
        };
        self.answer(&chat, Reply::Text);
        self.net.host_settles();
        self.chats.retain(|other| other.run != chat.run);
        self.ended(&chat.run);
        self.check();
    }

    /// The person stops a chat: its claims end as at a normal end.
    #[rule(weight = 1)]
    fn cancel(&mut self, tc: TestCase) {
        let Some(chat) = self.pick_chat(&tc) else {
            return;
        };
        let device = &mut self.net.computer;
        let run = chat.run.clone();
        device.workspace.update(&mut device.cx, |_, cx| {
            cx.emit(WorkspaceEvent::Cancel { run })
        });
        self.net.host_settles();
        self.chats.retain(|other| other.run != chat.run);
        self.ended(&chat.run);
        self.check();
    }

    /// tau closes and starts again: the runs it cut off are over, and so
    /// are their claims.
    #[rule(weight = 1)]
    fn restart(&mut self, _tc: TestCase) {
        self.net.restart();
        let cut: Vec<Chat> = std::mem::take(&mut self.chats);
        for chat in &cut {
            // Cut off mid-turn: tau says so, with no steps it was told.
            self.steps.remove(&chat.run);
        }
        for chat in &cut {
            let held: Vec<String> = self
                .claims
                .iter()
                .filter(|(_, by)| **by == chat.run)
                .map(|(id, _)| id.clone())
                .collect();
            let kept = self.held();
            for id in held {
                self.claims.remove(&id);
                assert_eq!(
                    kept[&id]["status"],
                    json!("open"),
                    "{id} after tau started"
                );
            }
        }
        self.check();
    }

    /// A chat asks to hand an issue to a new chat; the person allows it
    /// from the computer and the phone at once, or denies it: one chat
    /// at most starts, holding the issue.
    #[rule(weight = 2)]
    fn dispatch(&mut self, tc: TestCase) {
        let Some(chat) = self.pick_chat(&tc) else {
            return;
        };
        let free: Vec<String> = self
            .issues
            .iter()
            .filter(|id| {
                !self.claims.contains_key(*id) && !self.dispatched.contains(id)
            })
            .cloned()
            .collect();
        if free.is_empty() || self.chats.len() >= 4 {
            return;
        }
        let id = tc.draw(gs::sampled_from(free));
        self.answer(
            &chat,
            Reply::Tool {
                name: "todo_dispatch".into(),
                args: json!({ "id": id }),
            },
        );
        // The call waits for the person, not the model: until its card
        // asks.
        let call = self.asking(&chat.run);
        let allowed = tc.draw(gs::booleans());
        let act = json!({
            "act": "answer", "run": chat.run.0.as_ref(), "call": call,
            "allowed": allowed, "reason": "not now",
        });
        let runs_before = self.net.computer.runs().len();
        for who in [Who::Computer, Who::Phone(0)] {
            let device = self.net.device(who);
            let action = act.clone();
            device.workspace.update(&mut device.cx, |_, cx| {
                cx.emit(WorkspaceEvent::PluginAct {
                    plugin: LUAU.into(),
                    action,
                    asker: None,
                })
            });
        }
        self.net.settle();
        let started = self.net.computer.runs().len() - runs_before;
        let mark = format!("Work on {id}");
        if allowed {
            assert_eq!(started, 1, "one chat starts on {id}");
            let run = self.run_of(&mark).expect("the chat on the issue");
            assert_eq!(
                self.held()[&id]["claim"].as_str(),
                Some(run.0.as_ref())
            );
            self.claims.insert(id.clone(), run.clone());
            self.chats.push(Chat { mark, run });
        } else {
            assert_eq!(started, 0, "a denied dispatch starts nothing");
        }
        self.dispatched.push(id);
        self.check();
    }
}

#[hegel::test(test_cases = 6, suppress_health_check = [HealthCheck::TooSlow])]
fn claims_end_with_their_runs(tc: TestCase) {
    hegel::stateful::machine(Todo::new()).steps(14).run(tc);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn claims_end_with_their_runs_nightly(tc: TestCase) {
    hegel::stateful::machine(Todo::new()).steps(40).run(tc);
}
