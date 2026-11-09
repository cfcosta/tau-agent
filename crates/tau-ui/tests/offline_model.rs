//! The computer and two phones edit the same things while one or both
//! phones are away (decisions 0013, 0045): plugins' drafts and open
//! sections, a slot's pick, goals, the model new chats start on, models
//! hidden, chats closed, written to, stopped, and queued to land or
//! taken out of the queue. Phones come back in any order, sometimes
//! between one word of a draft and the next, a request comes twice, and
//! the computer closes and starts again on what it saved.
//!
//! What every case must show:
//! - **Convergence.** Once everything has arrived, every phone shows
//!   what the computer shows.
//! - **No lost intent, by today's rule.** Every edit made on any device
//!   reaches the computer once, and the value shown is the one of the
//!   edit that reached the computer last: the last arrival wins, not
//!   the last edit. A phone's edit made while away therefore overrides
//!   a later one made on the computer, and a draft is replaced whole, so
//!   words typed on two devices at once keep only one device's; the
//!   ignored tests at the end ask whether that should stay so.
//! - **Idempotence.** A request that comes again changes nothing.
//! - **Persistence.** After a restart the computer shows what it showed
//!   before: the UI state, chats closed, goals, models' settings.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/sync.rs"]
mod sync;

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ops::{Deref, DerefMut},
    rc::Rc,
};

use hegel::{HealthCheck, TestCase, generators as gs};
use serde_json::{Value, json};
use sync::{Keep, Net, Reply, Who, assert_synced, models, pick};
use tau_agent::tool::RunId;
use tau_remote::Up;
use tau_ui_remote::{
    WorkspaceEvent,
    models::{Effort, ModelChoice},
    route::Route,
    view::{Origin, RunView},
    workspace::Synced,
};

/// The plugin whose UI state the person edits: one with drafts and
/// sections, as tau-agents has.
const PLUGIN: &str = "tau-agents";
/// Its drafts, as tree inputs keep them.
const DRAFTS: [&str; 2] = ["draft.name", "draft.answer"];
/// Its sections, open or not.
const SECTIONS: [&str; 2] = ["open.secrets", "open.notices"];
/// The slot the person picks a filler for, and what they pick.
const SLOT: &str = "tau.forge";
const FILLERS: [&str; 2] = ["tau-github", "tau-forgejo"];

/// One thing the person can set, which holds one value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Reg {
    /// A key of a plugin's UI state.
    Ui(String, String),
    /// A chat's goal.
    Goal(RunId),
    /// The model coder's new chats start on.
    Default,
    /// Whether a model is hidden.
    Hidden(String),
    /// Whether a chat is open: closed, or written to again.
    Open(RunId),
    /// Whether a chat waits in its main chat's queue: asked to land, or
    /// taken out.
    Queued(RunId),
}

/// What the person set, and what reached the computer, in order.
#[derive(Debug, Default)]
struct Log {
    /// Each edit made on a device, as it was made.
    made: Vec<(Reg, Value)>,
    /// Each edit as the computer took it.
    arrived: Vec<(Reg, Value)>,
}

impl Log {
    /// The computer took `event`.
    fn arrived(&mut self, event: &WorkspaceEvent) {
        if let Some(edit) = edit_of(event) {
            self.arrived.push(edit);
        }
    }

    /// The value of the edit to `reg` that reached the computer last.
    fn last_arrived(&self) -> BTreeMap<Reg, Value> {
        self.arrived.iter().cloned().collect()
    }
}

/// The register `event` sets, and to what.
fn edit_of(event: &WorkspaceEvent) -> Option<(Reg, Value)> {
    Some(match event {
        WorkspaceEvent::Ui(event) => (
            Reg::Ui(event.plugin.clone(), event.key.clone()),
            event.value.clone().unwrap_or(Value::Null),
        ),
        WorkspaceEvent::PluginRecord { run, plugin, body }
            if plugin == tau_luau_plugins::NAME =>
        {
            let goal = goal_set(body)?;
            (Reg::Goal(run.clone()), json!(goal))
        }
        WorkspaceEvent::SetDefaultModel { agent, choice }
            if agent == "coder" =>
        {
            (Reg::Default, json!(choice.model))
        }
        WorkspaceEvent::HideModel { id, hidden } => {
            (Reg::Hidden(id.clone()), json!(hidden))
        }
        WorkspaceEvent::CloseRun { run } => {
            (Reg::Open(run.clone()), json!(false))
        }
        WorkspaceEvent::Say { run, .. } => {
            (Reg::Open(run.clone()), json!(true))
        }
        WorkspaceEvent::Land { run } => (Reg::Queued(run.clone()), json!(true)),
        WorkspaceEvent::Unqueue { run } => {
            (Reg::Queued(run.clone()), json!(false))
        }
        _ => return None,
    })
}

/// The goal a record of tau-luau-plugins' sets, when it is tau-goal's
/// state with one.
fn goal_set(body: &Value) -> Option<String> {
    let Ok(tau_luau_plugins::Record::State { plugin, state }) =
        serde_json::from_value(body.clone())
    else {
        return None;
    };
    (plugin == tau_luau_plugins::names::GOAL)
        .then(|| state["goal"]["condition"].as_str().map(str::to_owned))?
}

/// `run`'s goal, as `synced` shows it.
fn goal(synced: &Synced, run: &RunId) -> Option<String> {
    let view = synced.runs.iter().find(|view| &view.id == run)?;
    let luau = view.plugin_states.get(tau_luau_plugins::NAME)?.json();
    luau["plugins"][tau_luau_plugins::names::GOAL]["state"]["goal"]["condition"]
        .as_str()
        .map(str::to_owned)
}

/// What `synced` shows for `reg`, as an edit would set it.
fn shown(synced: &Synced, reg: &Reg) -> Value {
    let settings = &synced.catalog.models.settings;
    match reg {
        Reg::Ui(plugin, key) => synced
            .ui_state
            .get(plugin, key)
            .cloned()
            .unwrap_or(Value::Null),
        Reg::Goal(run) => goal(synced, run).map_or(Value::Null, Value::from),
        Reg::Default => json!(
            tau_ui_remote::models::Models {
                options: tau_chatgpt::plan_models(),
                settings: settings.clone(),
                ..Default::default()
            }
            .default_for("coder")
            .model
        ),
        Reg::Hidden(id) => json!(settings.is_hidden(id)),
        Reg::Open(run) => json!(!synced.closed.contains(run)),
        Reg::Queued(run) => json!(waits(synced, run)),
    }
}

/// Whether `run` waits in a main chat's landing queue.
fn waits(synced: &Synced, run: &RunId) -> bool {
    synced.runs.iter().any(|view| {
        view.landing_queue
            .iter()
            .any(|waiting| *waiting.run == *run.0)
    })
}

/// What a restart must keep, as the computer shows it.
fn kept(synced: &Synced) -> Value {
    let goals: BTreeMap<String, Option<String>> = synced
        .runs
        .iter()
        .map(|run| (run.id.0.to_string(), goal(synced, &run.id)))
        .collect();
    let closed: BTreeSet<String> =
        synced.closed.iter().map(|run| run.0.to_string()).collect();
    let repos: Vec<&String> =
        synced.catalog.repos.iter().map(|repo| &repo.name).collect();
    json!({
        "ui": synced.ui_state,
        "closed": closed,
        "goals": goals,
        "models": synced.catalog.models.settings,
        "repos": repos,
    })
}

struct Tau {
    net: Net,
    log: Rc<RefCell<Log>>,
    /// Chats the person closes, writes to and stops.
    chats: Vec<RunId>,
    /// The repository's main chat.
    main: RunId,
    /// Chats with changes, made to land through main's queue.
    landable: Vec<RunId>,
    /// The last number given, so every word, goal and file differs.
    next: usize,
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
    fn new() -> Self {
        let net = Net::new(2, Keep::Disk);
        let mut tau = Self {
            net,
            log: Rc::default(),
            chats: Vec::new(),
            main: RunId("".into()),
            landable: Vec::new(),
            next: 0,
        };
        // Main has a turn, which chats start from; two chats of the
        // person's have finished one each.
        tau.main = tau.computer.runs()[0].id.clone();
        let main = tau.main.clone();
        tau.say_to(Who::Computer, &main, "the first turn".into());
        tau.host_settles();
        tau.gate.answer(0, Reply::Text);
        tau.host_settles();
        for chat in ["chat a", "chat b"] {
            let computer = &mut tau.net.computer;
            computer.workspace.update(&mut computer.cx, |ws, cx| {
                ws.navigate(Route::NewRun, cx);
                ws.submit_prompt(chat.into(), cx);
            });
            tau.host_settles();
            tau.gate.answer(0, Reply::Text);
            tau.host_settles();
        }
        tau.chats = tau
            .computer
            .runs()
            .into_iter()
            .filter(|run| matches!(run.origin, Origin::Fork { .. }))
            .map(|run| run.id)
            .collect();
        assert_eq!(tau.chats.len(), 2, "the two chats started");
        tau.settle();
        tau.listen();
        tau
    }

    /// Logs what the computer takes from now on.
    fn listen(&mut self) {
        let log = self.log.clone();
        let workspace = self.net.computer.workspace.clone();
        self.net.computer.cx.update(|_, cx| {
            cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
                log.borrow_mut().arrived(event)
            })
            .detach();
        });
    }

    fn next(&mut self) -> usize {
        self.next += 1;
        self.next
    }

    fn who(&self, tc: &TestCase) -> Who {
        self.net.who(tc, 0.7)
    }

    /// `who` sets `key` of `plugin`'s UI state to `value`.
    fn set_ui(
        &mut self,
        who: Who,
        plugin: &'static str,
        key: &str,
        value: Option<Value>,
    ) {
        let reg = Reg::Ui(plugin.into(), key.into());
        self.made(reg, value.clone().unwrap_or(Value::Null));
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.set_plugin_ui(plugin, key, value, cx)
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    fn made(&mut self, reg: Reg, value: Value) {
        self.log.borrow_mut().made.push((reg, value));
    }

    /// What `who` shows for `key` of the plugin's UI state.
    fn ui_on(&mut self, who: Who, key: &str) -> Option<Value> {
        let device = self.device(who);
        device.workspace.read_with(&device.cx, |ws, _| {
            ws.synced().ui_state.get(PLUGIN, key).cloned()
        })
    }

    /// Every edit reached the computer once, and every device shows the
    /// one that reached it last.
    fn check(&mut self) {
        let computer = self.computer.synced();
        for (phone, device) in self.net.phones.iter_mut().enumerate() {
            let theirs = device.synced();
            assert_synced(&format!("phone {phone}"), &computer, &theirs);
        }
        let log = self.log.borrow();
        // A chat that lands is closed by the host, not the person: only
        // the person's chats count as theirs to open and close.
        let theirs = |(reg, _): &&(Reg, Value)| match reg {
            Reg::Open(run) => self.chats.contains(run),
            _ => true,
        };
        let key =
            |(reg, value): &(Reg, Value)| (reg.clone(), value.to_string());
        let mut made: Vec<_> =
            log.made.iter().filter(theirs).map(key).collect();
        let mut arrived: Vec<_> =
            log.arrived.iter().filter(theirs).map(key).collect();
        made.sort();
        arrived.sort();
        let (mut lost, mut extra) = (made.clone(), Vec::new());
        for edit in &arrived {
            match lost.iter().position(|made| made == edit) {
                Some(at) => {
                    lost.remove(at);
                }
                None => extra.push(edit.clone()),
            }
        }
        assert!(
            lost.is_empty() && extra.is_empty(),
            "what reached the computer is not what was made, once each: \
             never arrived {lost:?}; arrived unmade or twice {extra:?}"
        );
        for (reg, value) in log.last_arrived() {
            if !theirs(&&(reg.clone(), Value::Null)) {
                continue;
            }
            // A chat asked to land that main took landed, and is out of
            // the queue.
            let landed = computer.runs.iter().any(|view| {
                matches!(&reg, Reg::Queued(run) if *run == view.id)
                    && view.ending.is_some()
            });
            if landed {
                continue;
            }
            let history: Vec<_> =
                log.arrived.iter().filter(|(r, _)| *r == reg).collect();
            let view = match &reg {
                Reg::Open(run) | Reg::Queued(run) => {
                    computer.runs.iter().find(|view| view.id == *run).map(
                        |view| format!("{:?} {:?}", view.status, view.items),
                    )
                }
                _ => None,
            };
            assert_eq!(
                shown(&computer, &reg),
                value,
                "{reg:?} does not show the edit that arrived last; arrived \
                 {history:?}; the chat {view:?}"
            );
        }
    }
}

#[hegel::state_machine]
impl Tau {
    /// The person types a word into a draft, on whatever the device
    /// shows of it, or empties it.
    #[rule(weight = 4)]
    fn type_draft(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let key = tc.draw(gs::sampled_from(DRAFTS.to_vec()));
        let value = if tc.draw(gs::weighted_booleans(0.85)) {
            let shown = self.ui_on(who, key);
            let shown = shown.as_ref().and_then(Value::as_str).unwrap_or("");
            let word = format!("w{}", self.next());
            Some(json!(format!("{shown} {word}").trim_start()))
        } else {
            None
        };
        self.set_ui(who, PLUGIN, key, value);
    }

    /// The person opens or folds a section, as the device shows it.
    #[rule(weight = 2)]
    fn toggle_section(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let key = tc.draw(gs::sampled_from(SECTIONS.to_vec()));
        let open = self.ui_on(who, key).is_some();
        self.set_ui(who, PLUGIN, key, (!open).then_some(json!(true)));
    }

    /// The person picks which plugin fills a slot.
    #[rule]
    fn pick_slot(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let filler = tc.draw(gs::sampled_from(FILLERS.to_vec()));
        let reg = Reg::Ui(tau_ui_plugin::slots::PICKS.into(), SLOT.into());
        self.made(reg, json!(filler));
        let device = self.device(who);
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.pick_slot(SLOT, filler, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person sets a chat's goal: tau-goal's UI stores a record.
    #[rule]
    fn set_goal(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let mut runs = self.chats.clone();
        runs.push(self.main.clone());
        let run =
            runs[tc.draw(gs::integers().max_value(runs.len() - 1))].clone();
        let goal = format!("goal {}", self.next());
        self.made(Reg::Goal(run.clone()), json!(goal));
        let device = self.device(who);
        // As tau-goal's command leaves it: its state, kept by
        // tau-luau-plugins (ADR 0050).
        let record = tau_luau_plugins::Record::State {
            plugin: tau_luau_plugins::names::GOAL.into(),
            state: json!({ "goal": {
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

    /// The person picks the model new chats start on.
    #[rule]
    fn pick_default_model(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let model = tc.draw(gs::sampled_from(models()));
        self.made(Reg::Default, json!(model));
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            let choice = ModelChoice::new(model, Effort::Auto);
            ws.set_default_model("coder", choice, cx)
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person shows or hides a model, as the device shows it.
    #[rule]
    fn hide_model(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let id = tc.draw(gs::sampled_from(models()));
        let device = self.device(who);
        let hidden = device.workspace.update(&mut device.cx, |ws, cx| {
            ws.toggle_model_hidden(&id, cx);
            ws.catalog().models.settings.is_hidden(&id)
        });
        device.cx.run_until_parked();
        self.made(Reg::Hidden(id), json!(hidden));
        self.after_action(who);
    }

    /// The person closes a chat; a live one stops too.
    #[rule]
    fn close(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let run = self.chats[tc.draw(gs::integers().max_value(1))].clone();
        self.made(Reg::Open(run.clone()), json!(false));
        let device = self.device(who);
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.close_run(&run, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person writes to a chat, which opens it again.
    #[rule(weight = 2)]
    fn write(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let run = self.chats[tc.draw(gs::integers().max_value(1))].clone();
        let text = format!("message {}", self.next());
        self.made(Reg::Open(run.clone()), json!(true));
        self.say_to(who, &run, text);
        self.after_action(who);
    }

    /// The person stops a chat the device shows going.
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
        let event = WorkspaceEvent::Cancel { run };
        device
            .workspace
            .update(&mut device.cx, |_, cx| cx.emit(event));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The model answers a waiting request: a turn ends.
    #[rule(weight = 2)]
    fn model_answers(&mut self, tc: TestCase) {
        let waiting = self.gate.waiting();
        tc.assume(waiting > 0);
        let n = tc.draw(gs::integers().max_value(waiting - 1));
        self.gate.answer(n, Reply::Text);
        self.host_settles();
    }

    /// A chat from main writes a file and finishes; main gets a turn, so
    /// what asks to land waits in its queue.
    #[rule(weight = 2)]
    fn chat_to_land(&mut self, tc: TestCase) {
        // A few chats at a time, and main idle, so it can take a turn.
        let computer = self.computer.synced();
        let open = self
            .landable
            .iter()
            .filter(|run| {
                !computer
                    .runs
                    .iter()
                    .any(|view| view.id == **run && view.ending.is_some())
            })
            .count();
        tc.assume(open < 2 && self.gate.waiting() == 0);
        let text = format!("chat {}", self.next());
        let main = self.main.clone();
        let computer = &mut self.net.computer;
        let started = computer.workspace.update(&mut computer.cx, |ws, cx| {
            let started = ws.start_fork_at(&main, 1, cx);
            if started {
                ws.submit_prompt(text.clone(), cx);
            }
            started
        });
        tc.assume(started);
        self.host_settles();
        // Its turn writes a file, then it is done, and done again when
        // tau-vcs holds its stop once to have it commit.
        let file = format!("c{}.txt", self.next());
        for reply in [
            Reply::Write {
                path: file,
                content: format!("{text}\n"),
            },
            Reply::Text,
            Reply::Text,
        ] {
            if self.gate.waiting() != 1 {
                break;
            }
            self.gate.answer(0, reply);
            self.host_settles();
        }
        let chat = self.computer.runs().into_iter().find(|run| {
            matches!(run.origin, Origin::Fork { .. })
                && run
                    .items
                    .contains(&tau_ui_remote::view::Item::User(text.clone()))
                && !run.status.is_live()
        });
        let Some(chat) = chat else {
            tc.event("a chat to land did not finish");
            return;
        };
        self.landable.push(chat.id);
        // Main goes on with a turn of its own.
        let note = format!("main {}", self.next());
        self.say_to(Who::Computer, &main, note);
        self.host_settles();
    }

    /// The person asks to land a chat the device shows finished: it
    /// waits in the queue while main has a turn.
    #[rule(weight = 3)]
    fn land(&mut self, tc: TestCase) {
        tc.assume(!self.landable.is_empty());
        let who = self.who(&tc);
        let run = self.landable
            [tc.draw(gs::integers().max_value(self.landable.len() - 1))]
        .clone();
        let device = self.device(who);
        let synced = device.synced();
        let ended = synced
            .runs
            .iter()
            .any(|view| view.id == run && view.ending.is_some());
        tc.assume(!ended && !waits(&synced, &run));
        tc.event("a chat asked to land");
        self.made(Reg::Queued(run.clone()), json!(true));
        let device = self.device(who);
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.land(&run, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person takes a chat out of the queue, on a device that shows
    /// it waiting there.
    #[rule(weight = 3)]
    fn unqueue(&mut self, tc: TestCase) {
        let mut shown = Vec::new();
        let devices = [Who::Computer, Who::Phone(0), Who::Phone(1)];
        for who in devices {
            let synced = self.device(who).synced();
            for run in &self.landable {
                if waits(&synced, run) {
                    shown.push((who, run.clone()));
                }
            }
        }
        tc.assume(!shown.is_empty());
        let (who, run) =
            shown[tc.draw(gs::integers().max_value(shown.len() - 1))].clone();
        tc.event("a chat taken out of the queue");
        self.made(Reg::Queued(run.clone()), json!(false));
        let device = self.device(who);
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.unqueue(&run, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// One message crosses the network, one way or the other.
    #[rule(weight = 6)]
    fn one_arrives(&mut self, tc: TestCase) {
        let phone = tc.draw(gs::integers().max_value(1));
        let down: bool = tc.draw(gs::booleans());
        if !self.deliver(phone, down) {
            tc.reject();
        }
    }

    /// A phone goes away: what was on its way is lost; what the person
    /// does there waits in its outbox.
    #[rule(weight = 2)]
    fn phone_leaves(&mut self, tc: TestCase) {
        let phone = tc.draw(gs::integers().max_value(1));
        if !self.wire.borrow_mut().drop_phone(phone) {
            tc.reject();
        }
        tc.event("a phone goes away");
    }

    /// A phone comes back, from the last message it had, and sends what
    /// waits.
    #[rule(weight = 2)]
    fn phone_comes_back(&mut self, tc: TestCase) {
        let phone = tc.draw(gs::integers().max_value(1));
        tc.assume(self.wire.borrow().links[phone].conn.is_none());
        self.wire.borrow_mut().connect(phone);
    }

    /// The computer answers a phone that needs everything.
    #[rule]
    fn snapshot_sent(&mut self, tc: TestCase) {
        let phone = tc.draw(gs::integers().max_value(1));
        tc.assume(self.wire.borrow().links[phone].waiting);
        self.send_snapshot(phone);
    }

    /// The computer takes a phone's request, and the connection drops
    /// before its answer arrives: the phone sends it again on the next.
    #[rule]
    fn answer_lost(&mut self, tc: TestCase) {
        let phone = tc.draw(gs::integers().max_value(1));
        if !self.deliver(phone, false) {
            tc.reject();
        }
        tc.event("an answer was lost");
        self.wire.borrow_mut().drop_phone(phone);
    }

    /// A request the computer took comes again, as a network that
    /// repeats would bring it: it changes nothing.
    #[rule]
    fn comes_twice(&mut self, tc: TestCase) {
        let phone = tc.draw(gs::integers().max_value(1));
        let again = self.wire.borrow().links[phone].last_taken.clone();
        let Some((id, body)) = again else { tc.reject() };
        tc.assume(self.wire.borrow().links[phone].conn.is_some());
        tc.event("a request comes twice");
        let before = self.computer.synced();
        self.wire.borrow_mut().links[phone]
            .up
            .push_front(Up::Up { id, body });
        assert!(self.deliver(phone, false));
        let after = self.computer.synced();
        assert_synced(
            "the computer after a request came twice",
            &before,
            &after,
        );
    }

    /// tau closes and starts again on what it saved; phones go with it.
    #[rule]
    fn restart(&mut self, tc: TestCase) {
        tc.event("the computer starts again");
        self.host_settles();
        let shown = self.computer.synced();
        let before = kept(&shown);
        self.net.restart();
        self.listen();
        self.host_settles();
        let started = self.computer.synced();
        // What waited to land lands as tau starts, and closes: a turn of
        // main's that held it back ended with the restart.
        let landed: Vec<String> = started
            .runs
            .iter()
            .filter(|view| view.ending.is_some() && waits(&shown, &view.id))
            .map(|view| view.id.0.to_string())
            .collect();
        let mut after = kept(&started);
        if let Some(closed) = after["closed"].as_array_mut() {
            closed.retain(|run| {
                !landed.iter().any(|landed| run.as_str() == Some(landed))
            });
        }
        assert_eq!(after, before, "a restart changed what the computer shows");
    }

    /// Everything arrives: every device shows the same, and what reached
    /// the computer last.
    #[rule(weight = 3)]
    fn all_arrive(&mut self, _tc: TestCase) {
        self.settle();
        self.check();
    }
}

fn run(tc: TestCase, steps: i64) {
    hegel::stateful::machine(Tau::new()).steps(steps).run(tc);
}

// A real host per case, so generating is slow by nature.
#[hegel::test(test_cases = 16, suppress_health_check = [HealthCheck::TooSlow])]
fn devices_that_edit_while_away_end_up_agreeing(tc: TestCase) {
    run(tc, 30);
}

#[hegel::test(test_cases = 200, suppress_health_check = [HealthCheck::TooSlow])]
#[ignore = "nightly"]
fn devices_that_edit_while_away_end_up_agreeing_nightly(tc: TestCase) {
    run(tc, 60);
}

// What today's rule decides, which a person may not expect: each is a
// question for decision 0047, and passes once it is decided so.

/// One phone, connected, its draft empty; `key` of the plugin's UI
/// state set on `who` to `value`.
fn set_draft(net: &mut Net, who: Who, value: &str) {
    let device = net.device(who);
    device.workspace.update(&mut device.cx, |ws, cx| {
        ws.set_plugin_ui(PLUGIN, DRAFTS[0], Some(json!(value)), cx)
    });
    device.cx.run_until_parked();
    net.after_action(who);
}

fn draft(net: &mut Net) -> Option<Value> {
    net.computer
        .synced()
        .ui_state
        .get(PLUGIN, DRAFTS[0])
        .cloned()
}

#[test]
#[ignore = "should the last edit win, not the last to reach the computer? \
            a draft typed on a phone while away replaces one typed later \
            on the computer"]
fn the_later_edit_wins_over_one_made_away() {
    let mut net = Net::new(1, Keep::Memory);
    net.wire.borrow_mut().drop_phone(0);
    set_draft(&mut net, Who::Phone(0), "typed away on the phone");
    set_draft(&mut net, Who::Computer, "typed later on the computer");
    net.settle();
    assert_eq!(draft(&mut net), Some(json!("typed later on the computer")));
}

#[test]
#[ignore = "should a draft typed on two devices at once keep both \
            devices' words, as shared text does, rather than one \
            device's whole?"]
fn words_typed_on_two_devices_at_once_are_all_kept() {
    let mut net = Net::new(1, Keep::Memory);
    set_draft(&mut net, Who::Computer, "hello");
    net.settle();
    net.wire.borrow_mut().drop_phone(0);
    set_draft(&mut net, Who::Phone(0), "hello world");
    set_draft(&mut net, Who::Computer, "well, hello");
    net.settle();
    assert_eq!(draft(&mut net), Some(json!("well, hello world")));
}

#[test]
#[ignore = "should the phone keep what it did away on disk, so it \
            outlives the app being closed before it reaches the \
            computer?"]
fn what_a_phone_did_away_outlives_its_restart() {
    let mut net = Net::new(1, Keep::Memory);
    net.wire.borrow_mut().drop_phone(0);
    set_draft(&mut net, Who::Phone(0), "typed away on the phone");
    net.restart_phone(0, 1_000);
    net.settle();
    assert_eq!(draft(&mut net), Some(json!("typed away on the phone")));
}

#[test]
#[ignore = "should a phone's requests be numbered by something other \
            than its clock? one set back after a restart has the \
            computer drop every request as one it took"]
fn a_phone_whose_clock_went_back_is_still_heard() {
    let mut net = Net::new(1, Keep::Memory);
    // The phone numbers from its clock, as `remote` does.
    net.restart_phone(0, 1_000_000);
    set_draft(&mut net, Who::Phone(0), "before the clock went back");
    net.settle();
    net.restart_phone(0, 1_000);
    set_draft(&mut net, Who::Phone(0), "after the clock went back");
    net.settle();
    assert_eq!(draft(&mut net), Some(json!("after the clock went back")));
}

/// Words typed one after another on the computer: the host keeps, saves
/// and starts again with the last, as the window shows it.
#[test]
fn the_host_keeps_the_last_of_quick_edits() {
    let mut net = Net::new(1, Keep::Disk);
    let computer = &mut net.computer;
    computer.workspace.update(&mut computer.cx, |ws, cx| {
        for n in 0..200 {
            ws.set_plugin_ui(
                PLUGIN,
                DRAFTS[0],
                Some(json!(format!("w{n}"))),
                cx,
            );
        }
    });
    net.settle();
    let last = Some(json!("w199"));
    assert_eq!(draft(&mut net), last, "the window shows the last word");
    let kept = net.host.ui_state().get(PLUGIN, DRAFTS[0]).cloned();
    assert_eq!(kept, last, "the host keeps the last word");
    net.restart();
    assert_eq!(draft(&mut net), last, "the last word outlives a restart");
}

/// Models hidden and shown, and the model new chats start on, picked
/// one after another on the computer: every device ends with the last.
#[test]
fn the_host_keeps_the_last_of_quick_model_picks() {
    let mut net = Net::new(1, Keep::Memory);
    let ids = models();
    for round in 0..10 {
        let computer = &mut net.computer;
        computer.workspace.update(&mut computer.cx, |ws, cx| {
            for n in 0..=round + 40 {
                ws.toggle_model_hidden(&ids[0], cx);
                let model = ids[n % ids.len()].clone();
                ws.set_default_model(
                    "coder",
                    ModelChoice::new(model, Effort::Auto),
                    cx,
                );
            }
        });
        let shown = computer.synced().catalog.models.settings;
        net.settle();
        let settings = net.computer.synced().catalog.models.settings;
        assert_eq!(settings, shown, "round {round}: the last picks hold");
        let phone = net.phones[0].synced();
        assert_synced("phone 0", &net.computer.synced(), &phone);
    }
}

/// A phone that last saw a chat finished closes it while the computer
/// has it going again; a message after that opens it again, on every
/// device and after a restart, as any message to a closed chat does.
#[test]
fn a_message_opens_a_chat_closed_while_it_went_on() {
    let mut tau = Tau::new();
    let chat = tau.chats[0].clone();
    tau.wire.borrow_mut().drop_phone(0);
    tau.say_to(Who::Computer, &chat, "go on".into());
    tau.host_settles();
    // The phone still shows it finished: closing it does not stop it.
    let phone = &mut tau.net.phones[0];
    phone
        .workspace
        .update(&mut phone.cx, |ws, cx| ws.close_run(&chat, cx));
    phone.cx.run_until_parked();
    tau.settle();
    tau.say_to(Who::Computer, &chat, "and this".into());
    tau.settle();
    let open = |synced: &Synced| !synced.closed.contains(&chat);
    assert!(open(&tau.computer.synced()), "the message opened it");
    for phone in &mut tau.net.phones {
        assert!(open(&phone.synced()), "a phone shows it open");
    }
    // Its turn ends, and tau starts again.
    while tau.gate.waiting() > 0 {
        tau.gate.answer(0, Reply::Text);
        tau.host_settles();
    }
    tau.net.restart();
    assert!(open(&tau.computer.synced()), "open after a restart");
}
