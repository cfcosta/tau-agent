//! One tau, its computer's window and phones (decision 0013), against
//! what a person who used any of them must see on all of them.
//!
//! The computer runs the real host, with a model that answers only when
//! the test says so, so runs stay live as long as the test wants. The
//! phones reach it through the real feed and the real encoding, in
//! process: what the network would do is the test's to pick, message
//! by message, including dropping a phone and bringing it back.
//!
//! Whenever every message has arrived, every phone shows what the
//! computer shows ([`Workspace::synced`]), and the computer shows what
//! the person asked for, from whichever device they asked.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ops::{Deref, DerefMut},
    rc::Rc,
};

use hegel::{HealthCheck, TestCase, generators as gs};
use sync::{Keep, Net, Reply, Who, assert_synced, models, pick};
use tau_agent::tool::RunId;
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    models::{Effort, ModelChoice},
    route::Route,
    update::HostUpdate,
    view::{Item, Origin, RunView},
    workspace::Synced,
};

use crate::sync;

/// What the person asked for, in the order the host got it from any
/// device: what the computer must show once everything arrived.
#[derive(Debug, Default)]
struct Asked {
    /// Each message, its run (none for a new chat), and whether it may
    /// not show: the run was stopped after it, and a steer it had not
    /// read goes with it, or the host refused the new chat and said so.
    said: Vec<(Option<RunId>, String, bool)>,
    /// Chats closed and not written to since.
    closed: HashSet<RunId>,
    /// Chats stopped and not written to since.
    cancelled: HashSet<RunId>,
    /// Each chat's last goal set, through tau-goal's records.
    goals: HashMap<RunId, String>,
    /// Each model hidden or shown last.
    hidden: HashMap<String, bool>,
    /// The model coder runs on last picked.
    default: Option<ModelChoice>,
    /// Messages the host got more than once.
    twice: Vec<String>,
    /// Chats asked to land or be dropped, and not written to or refused
    /// since: each lands, waits in its main chat's queue, or is dropped.
    ending: HashSet<RunId>,
    /// The branch kept last.
    kept: Option<RunId>,
    /// Repositories hidden.
    hidden_repos: HashSet<String>,
    /// Chats ever asked to land or be dropped, and the messages that came
    /// for them after: the host refuses those once the chat ended.
    asked_to_end: HashSet<RunId>,
    /// Chats the host said landed or were dropped, asked for or not: a
    /// sub-agent nobody waits for lands as it returns (ADR 0026). A
    /// message the host gets for one after is late too: a phone's,
    /// typed while the chat still ran.
    ended: HashSet<RunId>,
    late: HashSet<String>,
    /// What the host said went wrong, for a failure to show.
    alerts: Vec<String>,
}

impl Asked {
    /// The host got `event`.
    fn arrived(&mut self, event: &WorkspaceEvent) {
        match event {
            WorkspaceEvent::NewRun { prompt, repo, .. } => {
                self.said_once(prompt);
                // A repository hidden before cannot take a new chat, nor
                // can none, once two devices hid one each; the host says
                // so.
                let refused =
                    repo.is_empty() || self.hidden_repos.contains(repo);
                self.said.push((None, prompt.clone(), refused))
            }
            WorkspaceEvent::Fork { prompt, .. } => {
                self.said_once(prompt);
                self.said.push((None, prompt.clone(), false))
            }
            WorkspaceEvent::Land { run }
            | WorkspaceEvent::DropChild { run } => {
                self.ending.insert(run.clone());
                self.asked_to_end.insert(run.clone());
            }
            WorkspaceEvent::KeepBranch { run } => self.kept = Some(run.clone()),
            WorkspaceEvent::HideRepo { repo } => {
                self.hidden_repos.insert(repo.clone());
            }
            WorkspaceEvent::Say { run, text, .. } => {
                self.said_once(text);
                if self.asked_to_end.contains(run) || self.ended.contains(run) {
                    self.late.insert(text.clone());
                }
                self.said.push((Some(run.clone()), text.clone(), false));
                self.ending.remove(run);
                // It may open the chat again, or start it again.
                self.closed.remove(run);
                self.cancelled.remove(run);
            }
            WorkspaceEvent::CloseRun { run } => {
                self.closed.insert(run.clone());
            }
            WorkspaceEvent::PluginRecord { run, plugin, body }
                if plugin == tau_luau_plugins::NAME =>
            {
                // tau-goal's state with a goal (ADR 0050).
                if let Ok(tau_luau_plugins::Record::State { plugin, state }) =
                    serde_json::from_value(body.clone())
                    && plugin == tau_luau_plugins::names::GOAL
                    && let Some(goal) = state["goal"]["condition"].as_str()
                {
                    self.goals.insert(run.clone(), goal.to_owned());
                }
            }
            WorkspaceEvent::HideModel { id, hidden } => {
                self.hidden.insert(id.clone(), *hidden);
            }
            WorkspaceEvent::SetDefaultModel { agent, choice }
                if agent == "coder" =>
            {
                self.default = Some(choice.clone());
            }
            WorkspaceEvent::Cancel { run } => {
                for (said, _, stopped) in &mut self.said {
                    *stopped |= said.as_ref() == Some(run);
                }
                self.cancelled.insert(run.clone());
            }
            _ => {}
        }
    }

    /// The host answered: a landing or a drop it refused, it said so.
    fn answered(&mut self, update: &HostUpdate) {
        match update {
            HostUpdate::Alert { title, message } => {
                // The host answers a new chat as it takes the request: a
                // refusal is of the last one, such as one from a main
                // chat left with conflicts (ADR 0024).
                if title == "Could not start the run"
                    || title == "Could not fork the run"
                {
                    let last = self
                        .said
                        .iter_mut()
                        .rev()
                        .find(|(run, ..)| run.is_none());
                    if let Some((_, _, refused)) = last {
                        *refused = true;
                    }
                }
                self.alerts.push(format!("{title}: {message}"))
            }
            // A turn tau starts, reporting sub-agents or resolving a
            // landing, is a new turn: the one the person stopped stays
            // stopped.
            HostUpdate::TauTurn { run, .. } => {
                self.cancelled.remove(run);
            }
            HostUpdate::Landed {
                run,
                landing: Err(_),
            }
            | HostUpdate::Dropped {
                run,
                result: Err(_),
            } => {
                self.ending.remove(run);
            }
            HostUpdate::Landed {
                run,
                landing: Ok(_),
            }
            | HostUpdate::Dropped {
                run,
                result: Ok(()),
            } => {
                self.ended.insert(run.clone());
            }
            _ => {}
        }
    }

    /// Every message is told apart: one the host got before came twice.
    fn said_once(&mut self, text: &str) {
        if self.said.iter().any(|(_, said, _)| said == text) {
            self.twice.push(text.to_owned());
        }
    }

    /// Fails unless `computer` shows what was asked, once.
    fn shown_by(&self, computer: &Synced) {
        assert!(self.twice.is_empty(), "taken twice: {:?}", self.twice);
        let users = |run: &RunView| -> Vec<String> {
            run.items
                .iter()
                .filter_map(|item| match item {
                    Item::User(text) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let shows = |run: Option<&RunId>, text: &String| {
            let in_transcript = computer
                .runs
                .iter()
                .filter(|view| run.is_none_or(|run| &view.id == run))
                .any(|view| users(view).contains(text));
            // A steer the run has not read yet shows as queued.
            let queued = run.and_then(|run| computer.queued.get(run));
            // A sub-agent that ended before reading it handed it to its
            // main chat, quoted in a message there (ADR 0026), or queued
            // for main's turn to read.
            let quoted = format!("> {text}");
            let forwarded = run
                .and_then(|run| {
                    computer.runs.iter().find(|view| &view.id == run)
                })
                .and_then(|view| match &view.origin {
                    Origin::SubAgent { parent } => Some(parent),
                    _ => None,
                })
                .and_then(|parent| {
                    computer.runs.iter().find(|view| &view.id == parent)
                })
                .is_some_and(|main| {
                    let read = main.items.iter().any(|item| match item {
                        Item::User(said) | Item::Tau(said) => {
                            said.contains(&quoted)
                        }
                        _ => false,
                    });
                    read || computer.queued.get(&main.id).is_some_and(|texts| {
                        texts.iter().any(|said| said.contains(&quoted))
                    })
                });
            in_transcript
                || forwarded
                || queued.is_some_and(|texts| texts.contains(text))
        };
        // A chat that landed or was dropped takes no more messages; the
        // host says so.
        let ended = |run: Option<&RunId>| {
            computer
                .runs
                .iter()
                .any(|view| Some(&view.id) == run && view.ending.is_some())
        };
        for (run, text, stopped) in &self.said {
            let refused = self.late.contains(text) && ended(run.as_ref());
            assert!(
                *stopped || refused || shows(run.as_ref(), text),
                "the computer does not show {text:?} to {run:?}; it said \
                 {:?}; it shows {:?}",
                self.alerts,
                computer
                    .runs
                    .iter()
                    .map(|view| (&view.id, &view.status, users(view)))
                    .collect::<Vec<_>>()
            );
        }
        for run in &self.closed {
            assert!(computer.closed.contains(run), "{run:?} is not closed");
        }
        for (run, goal) in &self.goals {
            let shown = computer
                .runs
                .iter()
                .find(|view| &view.id == run)
                .and_then(|view| view.plugin_states.get(tau_luau_plugins::NAME))
                .and_then(|state| {
                    state.json()["plugins"][tau_luau_plugins::names::GOAL]["state"]["goal"]
                        ["condition"]
                        .as_str()
                        .map(str::to_owned)
                });
            assert_eq!(shown.as_ref(), Some(goal), "{run:?}'s goal");
        }
        for run in &self.ending {
            let ended = computer
                .runs
                .iter()
                .any(|view| &view.id == run && view.ending.is_some());
            let waiting = computer.runs.iter().any(|view| {
                view.landing_queue
                    .iter()
                    .any(|waiting| *waiting.run == *run.0)
            });
            assert!(
                ended || waiting,
                "{run:?} neither ended nor waits to land"
            );
        }
        if let Some(run) = &self.kept {
            assert_eq!(
                computer.kept_branch.as_ref(),
                Some(run),
                "the kept branch"
            );
        }
        for repo in &self.hidden_repos {
            assert!(
                computer
                    .catalog
                    .repos
                    .iter()
                    .all(|listed| &listed.name != repo),
                "{repo} is still listed"
            );
        }
        let settings = &computer.catalog.models.settings;
        for (id, hidden) in &self.hidden {
            assert_eq!(settings.is_hidden(id), *hidden, "{id} hidden");
        }
        if let Some(choice) = &self.default {
            let coder = tau_ui_remote::models::Models {
                options: tau_chatgpt::plan_models(),
                settings: settings.clone(),
                ..Default::default()
            }
            .default_for("coder");
            assert_eq!(&coder, choice, "coder's model");
        }
        for run in &self.cancelled {
            let view = computer.runs.iter().find(|view| &view.id == run);
            assert!(
                view.is_some_and(|view| !view.status.is_live()),
                "{run:?} still runs after it was cancelled"
            );
        }
    }
}

struct Tau {
    net: Net,
    /// What the host got, from any device.
    asked: Rc<RefCell<Asked>>,
    /// The last prompt's number: each is told apart.
    prompts: usize,
}

impl Deref for Tau {
    type Target = Net;

    fn deref(&self) -> &Net {
        &self.net
    }
}

impl DerefMut for Tau {
    fn deref_mut(&mut self) -> &mut Net {
        &mut self.net
    }
}

impl Tau {
    fn new(phones: usize) -> Self {
        let mut net = Net::new(phones, Keep::Memory);
        // What the host gets: the computer's own requests, and phones'
        // as they arrive; and what it answers.
        let asked = Rc::new(RefCell::new(Asked::default()));
        let (arrived, answered) = (asked.clone(), asked.clone());
        let workspace = net.computer.workspace.clone();
        net.computer.cx.update(|_, cx| {
            cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
                arrived.borrow_mut().arrived(event)
            })
            .detach();
            cx.subscribe(&workspace, move |_, update: &HostUpdate, _| {
                answered.borrow_mut().answered(update)
            })
            .detach();
        });
        let mut tau = Self {
            net,
            asked,
            prompts: 0,
        };
        // The first repository's main chat has a turn, which new chats
        // can start from.
        let main = tau.computer.runs()[0].id.clone();
        tau.say_to(Who::Computer, &main, "the first turn".into());
        tau.host_settles();
        tau.gate.answer(0, Reply::Text);
        tau.settle();
        tau
    }

    /// Someone who can act: phones mostly, as what crosses the network
    /// is what can go wrong.
    fn who(&self, tc: &TestCase) -> Who {
        self.net.who(tc, 0.75)
    }

    fn prompt(&mut self) -> String {
        self.prompts += 1;
        format!("prompt {}", self.prompts)
    }
}

#[hegel::state_machine]
impl Tau {
    /// The person starts a chat.
    #[rule]
    fn new_chat(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let text = self.prompt();
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.navigate(Route::NewRun, cx);
            ws.submit_prompt(text.clone(), cx);
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person writes to a chat: a live one is steered, a finished
    /// one goes on.
    #[rule(weight = 3)]
    fn message(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let runs = self.device(who).runs();
        tc.assume(!runs.is_empty());
        let run = pick(&tc, &runs);
        let text = self.prompt();
        self.say_to(who, &run, text);
        self.after_action(who);
    }

    /// The person closes a chat.
    #[rule]
    fn close(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let runs = self.device(who).runs();
        tc.assume(!runs.is_empty());
        let run = pick(&tc, &runs);
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.close_run(&run, cx);
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person stops a live chat.
    #[rule]
    fn cancel(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let live: Vec<RunView> = self
            .device(who)
            .runs()
            .into_iter()
            .filter(|run| run.status.is_live())
            .collect();
        tc.assume(!live.is_empty());
        let run = pick(&tc, &live);
        let device = self.device(who);
        let event = WorkspaceEvent::Cancel { run: run.clone() };
        device
            .workspace
            .update(&mut device.cx, |_, cx| cx.emit(event));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person sets a chat's goal: tau-goal's UI stores a record.
    #[rule]
    fn set_goal(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let runs = self.device(who).runs();
        tc.assume(!runs.is_empty());
        let run = pick(&tc, &runs);
        let goal = self.prompt();
        let device = self.device(who);
        // As tau-goal's command leaves it: its state, kept by
        // tau-luau-plugins (ADR 0050).
        let record = tau_luau_plugins::Record::State {
            plugin: tau_luau_plugins::names::GOAL.into(),
            state: serde_json::json!({ "goal": {
                "condition": goal, "max_continuations": 1, "budget": 1.0,
                "status": "active", "continuations": 0, "spent": 0, "checks": [],
            } }),
        };
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.plugin_handle(tau_luau_plugins::NAME)
                .record(&run, record);
            ws.drain_plugin_requests(cx);
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person shows or hides a model in the picker.
    #[rule]
    fn hide_model(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let id = tc.draw(gs::sampled_from(models()));
        let device = self.device(who);
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.toggle_model_hidden(&id, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person picks the model new chats start on.
    #[rule]
    fn pick_default_model(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let model = tc.draw(gs::sampled_from(models()));
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            let choice = ModelChoice::new(model.clone(), Effort::Auto);
            ws.set_default_model("coder", choice, cx)
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person starts a chat from a main chat's finished turn.
    #[rule(weight = 2)]
    fn fork(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let device = self.device(who);
        let mains: Vec<(RunId, u32)> =
            device.workspace.read_with(&device.cx, |ws, _| {
                ws.runs()
                    .iter()
                    .filter(|run| ws.is_main(&run.id))
                    .map(|run| {
                        (run.id.clone(), Workspace::last_fork_turn_of(run))
                    })
                    .filter(|(_, turn)| *turn >= 1)
                    .collect()
            });
        tc.assume(!mains.is_empty());
        let n = tc.draw(gs::integers().min_value(0).max_value(mains.len() - 1));
        let (main, last) = mains[n].clone();
        let turn = tc.draw(gs::integers().min_value(1).max_value(last));
        let text = self.prompt();
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            assert!(ws.start_fork_at(&main, turn, cx));
            ws.submit_prompt(text, cx);
        });
        tc.event("a chat started from a turn");
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person lands a finished chat on its main chat, drops it, or
    /// keeps its branch.
    #[rule(weight = 3)]
    fn end_chat(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let device = self.device(who);
        let chats: Vec<RunView> = device
            .runs()
            .into_iter()
            .filter(|run| {
                matches!(run.origin, Origin::Fork { .. })
                    && !run.status.is_live()
                    && run.ending.is_none()
            })
            .collect();
        tc.assume(!chats.is_empty());
        let run = pick(&tc, &chats);
        let how = tc.draw(gs::sampled_from(vec!["land", "drop", "keep"]));
        tc.event(format!("a chat asked to {how}"));
        device.workspace.update(&mut device.cx, |ws, cx| match how {
            "land" => ws.land(&run, cx),
            "drop" => ws.drop_child(&run, cx),
            _ => ws.keep_branch(&run, cx),
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person stops listing a repository, while another is listed.
    #[rule]
    fn hide_repo(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let device = self.device(who);
        let repos: Vec<String> =
            device.workspace.read_with(&device.cx, |ws, _| {
                ws.catalog()
                    .repos
                    .iter()
                    .map(|repo| repo.name.clone())
                    .collect()
            });
        tc.assume(repos.len() > 1);
        let repo = tc.draw(gs::sampled_from(repos));
        tc.event("a repository hidden");
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.remove_repo(&repo, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// Someone writes to a finished main chat, and the model hands a
    /// task to a sub-agent: a chat of its own, under main, on every
    /// device.
    #[rule(weight = 2)]
    fn main_spawns(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let mains: Vec<RunView> =
            self.computer
                .workspace
                .read_with(&self.computer.cx, |ws, _| {
                    ws.runs()
                        .iter()
                        .filter(|run| {
                            ws.is_main(&run.id) && !run.status.is_live()
                        })
                        .cloned()
                        .collect()
                });
        tc.assume(!mains.is_empty());
        let main = pick(&tc, &mains);
        let waiting = self.gate.waiting();
        let text = self.prompt();
        self.say_to(who, &main, text);
        // A phone's message goes up first.
        if let Who::Phone(phone) = who {
            while self.deliver(phone, false) {}
        }
        self.host_settles();
        // Main went on, unless the host had it running by now: then the
        // message steered it, and there is nothing to hand off. (Too late
        // to reject: the message went.)
        if self.gate.waiting() != waiting + 1 {
            return;
        }
        tc.event("a main chat hands off a task");
        self.gate.answer(
            waiting,
            Reply::Delegate {
                task: "write the notes".into(),
            },
        );
        self.host_settles();
    }

    /// A chat starts from main, writes a file and finishes, and the
    /// person lands it from a device that shows it finished.
    #[rule(weight = 2)]
    fn chat_lands(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let main =
            self.computer
                .workspace
                .read_with(&self.computer.cx, |ws, _| {
                    ws.runs()
                        .iter()
                        .find(|run| {
                            ws.is_main(&run.id)
                                && Workspace::last_fork_turn_of(run) >= 1
                        })
                        .map(|run| run.id.clone())
                });
        let Some(main) = main else {
            tc.event("no main chat to land on");
            tc.reject()
        };
        let waiting = self.gate.waiting();
        let text = self.prompt();
        let computer = &mut self.computer;
        computer.workspace.update(&mut computer.cx, |ws, cx| {
            if ws.start_fork_at(&main, 1, cx) {
                ws.submit_prompt(text.clone(), cx);
            }
        });
        self.host_settles();
        // Its turn writes a file, then it is done, and done again when
        // tau-vcs holds its stop once to have it commit; something else
        // may have taken the turn first.
        for reply in [
            Reply::Write {
                path: "c.txt".into(),
                content: format!("{text}\n"),
            },
            Reply::Text,
            Reply::Text,
        ] {
            if self.gate.waiting() != waiting + 1 {
                tc.event("a chat to land was taken over");
                return;
            }
            self.gate.answer(waiting, reply);
            self.host_settles();
        }
        let chat = self.computer.runs().into_iter().find(|run| {
            matches!(run.origin, Origin::Fork { .. })
                && run.items.contains(&Item::User(text.clone()))
                && !run.status.is_live()
        });
        let Some(chat) = chat else {
            tc.event("a chat to land did not finish");
            return;
        };
        // The device sees it finished before it lands it.
        self.settle();
        tc.event("a finished chat lands");
        let device = self.device(who);
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.land(&chat.id, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The model answers one waiting request.
    #[rule(weight = 3)]
    fn model_answers(&mut self, tc: TestCase) {
        let waiting = self.gate.waiting();
        tc.assume(waiting > 0);
        let n = tc.draw(gs::integers().min_value(0).max_value(waiting - 1));
        let text = self.prompt();
        // Mostly done; else a file, which two chats may both write, or a
        // sub-agent's task.
        let reply =
            match tc.draw(gs::integers::<u8>().min_value(0).max_value(3)) {
                0 | 1 => Reply::Text,
                2 => Reply::Write {
                    path: tc
                        .draw(gs::sampled_from(vec!["a.txt", "b.txt"]))
                        .to_owned(),
                    content: format!("{text}\n"),
                },
                _ => Reply::Delegate {
                    task: "write the notes".into(),
                },
            };
        tc.event(match &reply {
            Reply::Text => "the model is done",
            Reply::Write { .. } => "the model writes a file",
            Reply::Delegate { .. } => "the model hands off a task",
            Reply::Tool { .. } => "the model calls a tool",
        });
        self.gate.answer(n, reply);
        self.host_settles();
    }

    /// One message crosses the network, one way or the other.
    #[rule(weight = 4)]
    fn one_arrives(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        let down: bool = tc.draw(gs::booleans());
        if !self.deliver(phone, down) {
            tc.reject();
        }
    }

    /// A phone's connection drops: what was on its way is lost.
    #[rule]
    fn phone_drops(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        let mut wire = self.wire.borrow_mut();
        let link = &mut wire.links[phone];
        // The network loses what was on its way, both ways, answers
        // included; the phone's outbox keeps what was not taken.
        let Some(conn) = link.conn.take() else {
            tc.reject()
        };
        link.down.clear();
        link.up.clear();
        link.waiting = false;
        wire.feed.leave(conn);
    }

    /// The computer takes a phone's request, and the connection drops
    /// before its answer arrives: the phone sends it again on the next.
    #[rule(weight = 3)]
    fn answer_lost(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        if !self.deliver(phone, false) {
            tc.reject();
        }
        tc.event("an answer was lost");
        let mut wire = self.wire.borrow_mut();
        let link = &mut wire.links[phone];
        let conn = link.conn.take().expect("it was connected");
        link.down.clear();
        link.up.clear();
        link.waiting = false;
        wire.feed.leave(conn);
    }

    /// A phone that dropped connects again, from the last message it
    /// had.
    #[rule]
    fn phone_comes_back(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        tc.assume(self.wire.borrow().links[phone].conn.is_none());
        self.wire.borrow_mut().connect(phone);
    }

    /// The computer answers a phone that needs everything.
    #[rule]
    fn snapshot_sent(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        tc.assume(self.wire.borrow().links[phone].waiting);
        self.send_snapshot(phone);
    }

    /// Everything arrives: every phone shows what the computer shows,
    /// and the computer shows what was asked of any of them.
    #[rule(weight = 2)]
    fn all_arrive(&mut self, tc: TestCase) {
        self.settle();
        let computer = self.computer.synced();
        if computer
            .runs
            .iter()
            .any(|run| matches!(run.origin, Origin::SubAgent { .. }))
        {
            tc.event("a sub-agent's chat is shown");
        }
        for (phone, device) in self.phones.iter_mut().enumerate() {
            let theirs = device.synced();
            assert_synced(&format!("phone {phone}"), &computer, &theirs);
        }
        self.asked.borrow().shown_by(&computer);
    }
}

// A real host per case, so generating is slow by nature.
#[hegel::test(test_cases = 30, suppress_health_check = [HealthCheck::TooSlow])]
fn every_device_shows_what_any_device_did(tc: TestCase) {
    let phones = tc.draw(gs::integers().min_value(1).max_value(2));
    let tau = Tau::new(phones);
    hegel::stateful::machine(tau).steps(30).run(tc);
}
