//! tau-todo's board and band on every device (ADR 0048, "5. The board
//! and the band on every device"): the computer and two phones, through
//! the real feed and encoding, moving cards between columns, the same
//! card on two devices at once, starting a run from the board, folding
//! the band, and tau starting again.
//!
//! - Every move is the action it stands for: to Done closes with the
//!   reason given, to Blocked records what it waits on, to In progress
//!   starts a chat holding the issue, back to Ready opens it again.
//! - The same card moved on two devices ends in one column: the one the
//!   computer took first; the other move is refused, not applied.
//! - Once everything arrives, every phone shows what the computer does,
//!   the band's fold among it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/sync.rs"]
mod sync;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use hegel::{HealthCheck, TestCase, generators as gs};
use serde_json::{Value, json};
use sync::{Keep, Net, Who, assert_synced};
use tau_luau_plugins_host::store::{Fuse, Kv};
use tau_testing::block_on_io;
use tau_ui_remote::WorkspaceEvent;

const REPO: &str = "repo";
const LUAU: &str = "tau-luau-plugins";

struct Board {
    net: Net,
    store: Arc<Kv>,
    /// Each issue's column, as the model has it.
    columns: BTreeMap<String, &'static str>,
    /// The band's fold, as the person left it.
    folded: Option<bool>,
}

fn issue(id: &str) -> Value {
    json!({
        "id": id, "title": format!("work on {id}"), "description": "",
        "criteria": [], "type": "task", "priority": 2, "status": "open",
        "labels": [], "comments": [], "links": [],
        "created": 1_793_700_000, "updated": 1_793_700_000,
    })
}

impl Board {
    fn new() -> Self {
        tau::install();
        let net = Net::new(2, Keep::Disk);
        let path: PathBuf = net
            .dir()
            .join("plugins")
            .join(LUAU)
            .join("stores")
            .join(tau_todo::NAME)
            .join(format!("{REPO}.db"));
        let store =
            Arc::new(block_on_io(Kv::open(&path, Fuse::never())).unwrap());
        let ids: Vec<String> = (1..=3).map(|n| format!("td-000{n}")).collect();
        block_on_io(async {
            let mut session = store.session().await;
            for id in &ids {
                session.put(&format!("issue/{id}"), issue(id));
            }
            session.commit().await.unwrap();
        });
        Self {
            net,
            store,
            columns: ids.into_iter().map(|id| (id, "ready")).collect(),
            folded: None,
        }
    }

    /// Each issue's column, as the store has it.
    fn kept(&self) -> BTreeMap<String, &'static str> {
        block_on_io(self.store.everything())
            .unwrap()
            .into_iter()
            .filter_map(|(key, issue)| {
                let id = key.strip_prefix("issue/")?.to_owned();
                let column = match issue["status"].as_str()? {
                    "closed" => "done",
                    "in_progress" => "in_progress",
                    "blocked" => "blocked",
                    _ => "ready",
                };
                Some((id, column))
            })
            .collect()
    }

    /// `who` carries out the board's `action` with `args`, as its page's
    /// button or drag does.
    fn act(&mut self, who: Who, action: &str, args: Value) {
        let act = json!({
            "act": "repo_action", "plugin": tau_todo::NAME, "repo": REPO,
            "action": action, "args": args,
        });
        let device = self.net.device(who);
        device.workspace.update(&mut device.cx, |_, cx| {
            cx.emit(WorkspaceEvent::PluginAct {
                plugin: LUAU.into(),
                action: act,
                asker: None,
            })
        });
    }

    fn in_column(&self, tc: &TestCase, column: &str) -> Option<String> {
        let ids: Vec<String> = self
            .columns
            .iter()
            .filter(|(_, at)| **at == column)
            .map(|(id, _)| id.clone())
            .collect();
        (!ids.is_empty()).then(|| tc.draw(gs::sampled_from(ids)))
    }

    fn check(&mut self) {
        self.net.settle();
        assert_eq!(self.kept(), self.columns, "the board's columns");
        // In progress is a chat's: one the computer shows holds it.
        let runs: Vec<String> = self
            .net
            .computer
            .runs()
            .iter()
            .map(|run| run.id.0.to_string())
            .collect();
        for (key, issue) in block_on_io(self.store.everything()).unwrap() {
            if key.starts_with("issue/")
                && issue["status"] == json!("in_progress")
            {
                let claim = issue["claim"].as_str().unwrap_or_default();
                assert!(
                    runs.iter().any(|run| run == claim),
                    "{key} is held by {claim:?}, no chat"
                );
            }
        }
        let ours = self.net.computer.synced();
        for phone in 0..self.net.phones.len() {
            let theirs = self.net.phones[phone].synced();
            assert_synced(&format!("phone {phone}"), &ours, &theirs);
        }
    }
}

#[hegel::state_machine]
impl Board {
    /// A card dragged to Done, and the reason typed in the modal.
    #[rule(weight = 3)]
    fn drag_to_done(&mut self, tc: TestCase) {
        let from = tc.draw(gs::sampled_from(vec!["ready", "blocked"]));
        let Some(id) = self.in_column(&tc, from) else {
            return;
        };
        let who = self.net.who(&tc, 0.5);
        self.act(
            who,
            "move",
            json!({ "card": id, "from": from, "to": "done" }),
        );
        self.net.settle();
        self.act(who, "close", json!({ "text": "done on the board" }));
        self.columns.insert(id, "done");
        self.check();
    }

    /// A card dragged to Blocked, with what it waits on.
    #[rule(weight = 2)]
    fn drag_to_blocked(&mut self, tc: TestCase) {
        let Some(id) = self.in_column(&tc, "ready") else {
            return;
        };
        let who = self.net.who(&tc, 0.5);
        self.act(
            who,
            "move",
            json!({ "card": id, "from": "ready", "to": "blocked" }),
        );
        self.net.settle();
        self.act(who, "block", json!({ "text": "the vendor" }));
        self.columns.insert(id, "blocked");
        self.check();
    }

    /// A card dragged back to Ready: open again.
    #[rule(weight = 2)]
    fn drag_to_ready(&mut self, tc: TestCase) {
        let from = tc.draw(gs::sampled_from(vec!["done", "blocked"]));
        let Some(id) = self.in_column(&tc, from) else {
            return;
        };
        let who = self.net.who(&tc, 0.5);
        self.act(
            who,
            "move",
            json!({ "card": id, "from": from, "to": "ready" }),
        );
        self.columns.insert(id, "ready");
        self.check();
    }

    /// The same card dragged on the computer and a phone at once, to two
    /// columns: the computer's move is taken first, and the phone's,
    /// from a column it left, is refused.
    #[rule(weight = 2)]
    fn same_card_twice(&mut self, tc: TestCase) {
        let Some(id) = self.in_column(&tc, "ready") else {
            return;
        };
        let phone = tc.draw(gs::integers::<usize>().max_value(1));
        self.act(
            Who::Computer,
            "move",
            json!({ "card": id, "from": "ready", "to": "done" }),
        );
        self.act(
            Who::Phone(phone),
            "move",
            json!({ "card": id, "from": "ready", "to": "blocked" }),
        );
        self.net.settle();
        self.act(Who::Computer, "close", json!({ "text": "the computer's" }));
        self.columns.insert(id, "done");
        self.check();
    }

    /// "Start a run" on a card: a chat starts holding it.
    #[rule(weight = 1)]
    fn start_run(&mut self, tc: TestCase) {
        let Some(id) = self.in_column(&tc, "ready") else {
            return;
        };
        let who = self.net.who(&tc, 0.5);
        // From the card's button, or dragged to In progress.
        if tc.draw(gs::booleans()) {
            self.act(who, "start", json!({ "issue": id }));
        } else {
            self.act(
                who,
                "move",
                json!({ "card": id, "from": "ready", "to": "in_progress" }),
            );
        }
        self.columns.insert(id, "in_progress");
        self.check();
    }

    /// The band over a run's composer folded or opened, on any device.
    #[rule(weight = 1)]
    fn fold_band(&mut self, tc: TestCase) {
        let open = tc.draw(gs::booleans());
        let who = self.net.who(&tc, 0.5);
        let device = self.net.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.set_plugin_ui(
                LUAU,
                tau_ui_plugin::tree::open_key("tau-todo/todo-band"),
                Some(json!(open)),
                cx,
            )
        });
        self.folded = Some(!open);
        self.check();
    }

    /// tau closes and starts again: the chats it cut off let go of what
    /// they held.
    #[rule(weight = 1)]
    fn restart(&mut self, _tc: TestCase) {
        self.net.restart();
        for column in self.columns.values_mut() {
            if *column == "in_progress" {
                *column = "ready";
            }
        }
        self.check();
    }
}

#[hegel::test(test_cases = 5, suppress_health_check = [HealthCheck::TooSlow])]
fn every_device_shows_the_board_the_computer_does(tc: TestCase) {
    hegel::stateful::machine(Board::new()).steps(12).run(tc);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn every_device_shows_the_board_the_computer_does_nightly(tc: TestCase) {
    hegel::stateful::machine(Board::new()).steps(40).run(tc);
}
