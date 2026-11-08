//! A Luau plugin starts and steers chats (ADR 0048, the third seam): an
//! action asks, and the host carries it out once the action ended, then
//! calls the action it names next with the chat; a run's tool asks the
//! person first, on a card the first answer takes, from whichever
//! device.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{agent::Agent, event::RunEvent};
use tau_luau_plugins::{NAME, Outcome, Record};
use tau_luau_plugins_host::{
    agent::{Dispatch, LuauPlugins},
    chats::Answer,
    registry::Registry,
};
use tau_testing::{block_on_io, scripted::ScriptedModel};
use tau_ui_plugin::{Conversations, Turn, registry::HostFuture};
use tau_vcs_host::{Identity, Project, ProjectRepo};

const HAND_OFF: &str = r#"
local tau = require("tau")
return tau.plugin {
  name = "greet",
  uses = { store = true },
  tools = {
    hand_off = {
      description = "Asks the person to start a chat on an issue.",
      call = function(args, ctx)
        return tau.allow {
          title = "Start a chat on " .. args.issue,
          text = "In a new chat.",
          action = "dispatch",
          args = { issue = args.issue },
        }
      end,
    },
  },
  actions = {
    dispatch = function(args, ctx)
      return tau.start_chat { text = "Work on " .. args.issue, ["then"] = "claimed", args = { issue = args.issue } }
    end,
    claimed = function(args, ctx)
      if args.error then return "not started: " .. args.error end
      ctx.store.put("claim/" .. args.issue, args.run)
      return "started " .. args.run .. " in " .. args.repo
    end,
    nudge = function(args, ctx)
      return tau.steer_chat { run = args.run, text = "Go on with " .. args.issue }
    end,
  },
}
"#;

/// The chats a plugin asked for: each start's repository and text, and
/// each steer's run and text. A start answers `r-new`, or fails when
/// told to.
#[derive(Default)]
struct Chats {
    started: Mutex<Vec<(String, String)>>,
    steered: Mutex<Vec<(String, String)>>,
    refuse: bool,
}

impl Conversations for Chats {
    fn make_repo(
        &self,
        _kind: &str,
        _name: &str,
    ) -> HostFuture<'_, anyhow::Result<()>> {
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

    fn start(
        &self,
        repo: &str,
        text: String,
    ) -> HostFuture<'_, anyhow::Result<String>> {
        self.started.lock().unwrap().push((repo.to_owned(), text));
        let refuse = self.refuse;
        Box::pin(async move {
            if refuse {
                anyhow::bail!("the code is conflicted")
            }
            Ok("r-new".to_owned())
        })
    }

    fn steer(
        &self,
        run: &str,
        text: String,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        self.steered.lock().unwrap().push((run.to_owned(), text));
        Box::pin(async { Ok(()) })
    }
}

fn registry(home: &std::path::Path) -> Registry {
    let repo = ProjectRepo::init(
        home.join("v1"),
        Identity::default(),
        &[("greet/plugin.luau".to_owned(), HAND_OFF.to_owned())],
    )
    .unwrap();
    let trunk = repo.trunk().unwrap();
    let registry = Registry::at(
        home.join("x"),
        home.join("allowed.json"),
        Identity::default(),
        || {},
    );
    block_on_io(registry.reload(&Project::from(repo), &trunk));
    block_on_io(registry.allow("greet")).unwrap();
    registry
}

fn claim(registry: &Registry, issue: &str) -> Option<Value> {
    let kv = block_on_io(registry.stores().of("greet", "home")).unwrap();
    block_on_io(kv.everything())
        .unwrap()
        .remove(&format!("claim/{issue}"))
}

/// What a person's action asks of the chats is carried out once it
/// ended, then the action it names next is called with the chat: the
/// claim it records names the chat that started. A chat that would not
/// start reaches it as an error, and nothing is claimed.
#[test]
fn an_action_starts_a_chat_then_records_it() {
    let home = tempfile::tempdir().unwrap();
    let registry = registry(home.path());
    let chats = Chats::default();
    let said = block_on_io(registry.repo_action(
        "greet",
        "home",
        "dispatch",
        json!({ "issue": "td-1" }),
        Some(&chats),
        None,
    ))
    .unwrap();
    assert_eq!(said, json!("started r-new in home"));
    assert_eq!(
        *chats.started.lock().unwrap(),
        [("home".to_owned(), "Work on td-1".to_owned())]
    );
    assert_eq!(claim(&registry, "td-1"), Some(json!("r-new")));

    let refusing = Chats {
        refuse: true,
        ..Chats::default()
    };
    let said = block_on_io(registry.repo_action(
        "greet",
        "home",
        "dispatch",
        json!({ "issue": "td-2" }),
        Some(&refusing),
        None,
    ))
    .unwrap();
    assert_eq!(said, json!("not started: the code is conflicted"));
    assert_eq!(claim(&registry, "td-2"), None);

    // Steering names no next action: what came of it is the answer.
    let said = block_on_io(registry.repo_action(
        "greet",
        "home",
        "nudge",
        json!({ "issue": "td-1", "run": "r-new" }),
        Some(&chats),
        None,
    ))
    .unwrap();
    assert_eq!(said, json!({ "run": "r-new" }));
    assert_eq!(
        *chats.steered.lock().unwrap(),
        [("r-new".to_owned(), "Go on with td-1".to_owned())]
    );

    // A host with no chats refuses, and says so.
    let refused = block_on_io(registry.repo_action(
        "greet",
        "home",
        "dispatch",
        json!({ "issue": "td-3" }),
        None,
        None,
    ));
    assert!(
        format!("{:#}", refused.unwrap_err()).contains("no chats"),
        "a host with no chats"
    );
}

/// A run that asks the person to start a chat, answered by `answers`
/// (each `None` a click that never comes) from as many devices: what the
/// model read of the call, the Allow card's records, and the chats
/// started.
struct Asked {
    result: String,
    records: Vec<Record>,
    started: Vec<(String, String)>,
    /// What each device's answer met.
    answered: Vec<bool>,
}

fn ask(answers: &[Answer], cancel: bool) -> Asked {
    let home = tempfile::tempdir().unwrap();
    let registry = registry(home.path());
    let chats = Arc::new(Chats::default());
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("hand_off", json!({ "issue": "td-1" })))
        .turn(|t| t.text("asked"));
    let store = block_on_io(tau_store_sqlite::memory()).unwrap();
    let plugins = LuauPlugins::new(
        block_on_io(registry.active()),
        json!({ "kind": "chat", "repo": "home", "model": "m" }),
        None,
    )
    .with_stores(registry.stores())
    .with_dispatch(Dispatch {
        allows: registry.allows(),
        forms: registry.forms(),
        registry: registry.clone(),
        chats: Some(chats.clone()),
    });
    let agent = Agent::new(llm.clone()).plugin(plugins);
    let allows = registry.allows();
    let (result, run, answered) = block_on_io(async {
        let mut run = agent.start("hand it off", &store);
        let (id, control) = (run.id(), run.control());
        let answering = async {
            let key = loop {
                if let Some(key) = allows.waiting().into_iter().next() {
                    break key;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            };
            assert_eq!(key.0, id.0.as_ref(), "the ask is the run's");
            if cancel {
                control.cancel();
            }
            // Every device at once: each takes it only if it still waits.
            answers
                .iter()
                .map(|answer| {
                    allows.answer(&key.0, &key.1, answer.clone()).is_ok()
                })
                .collect::<Vec<_>>()
        };
        let events = async {
            let events: Vec<RunEvent> = run.events().collect().await;
            events
        };
        let (answered, events) = tokio::join!(answering, events);
        let result = events
            .iter()
            .find_map(|event| match event {
                RunEvent::ToolEnd { output, .. } => Some(
                    output
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            tau_ai::message::InputBlock::Text(text) => {
                                Some(text.text.clone())
                            }
                            tau_ai::message::InputBlock::Image(_) => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .unwrap_or_default();
        (result, id, answered)
    });
    let records = block_on_io(store.records(&run.0, NAME))
        .unwrap()
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .filter(|record| {
            matches!(record, Record::Asked { .. } | Record::Answered { .. })
        })
        .collect();
    let started = chats.started.lock().unwrap().clone();
    assert!(
        registry.allows().waiting().is_empty(),
        "nothing waits once the call ended"
    );
    Asked {
        result,
        records,
        started,
        answered,
    }
}

fn outcome(records: &[Record]) -> Option<Outcome> {
    records.iter().rev().find_map(|record| match record {
        Record::Answered { outcome, .. } => Some(*outcome),
        _ => None,
    })
}

/// Allowed, from two devices at once, the dispatch starts exactly one
/// chat: the first answer takes the card, the second finds it taken. The
/// model reads what the action answered.
#[test]
fn an_allowed_dispatch_starts_one_chat_however_many_devices_allow() {
    let allow = Answer {
        allowed: true,
        reason: String::new(),
    };
    let asked = ask(&[allow.clone(), allow], false);
    assert_eq!(asked.answered, [true, false]);
    assert_eq!(
        asked.started,
        [("home".to_owned(), "Work on td-1".to_owned())]
    );
    assert_eq!(asked.result, "started r-new in home");
    assert!(
        matches!(
            &asked.records[0],
            Record::Asked { title, .. } if title == "Start a chat on td-1"
        ),
        "{:?}",
        asked.records
    );
    assert_eq!(outcome(&asked.records), Some(Outcome::Allowed));
}

/// Denied, it starts nothing, and the model reads why, once.
#[test]
fn a_denied_dispatch_starts_nothing_and_says_why() {
    let deny = Answer {
        allowed: false,
        reason: "not now".into(),
    };
    let allow = Answer {
        allowed: true,
        reason: String::new(),
    };
    let asked = ask(&[deny, allow], false);
    assert_eq!(asked.answered, [true, false]);
    assert!(asked.started.is_empty(), "{:?}", asked.started);
    assert_eq!(asked.result, "The person did not allow it: not now");
    assert_eq!(outcome(&asked.records), Some(Outcome::Denied));
}

/// A run that ends before the answer withdraws its card, and a late
/// Allow starts nothing.
#[test]
fn a_dispatch_whose_run_ended_is_withdrawn() {
    let allow = Answer {
        allowed: true,
        reason: String::new(),
    };
    let asked = ask(&[allow], true);
    assert!(asked.started.is_empty(), "{:?}", asked.started);
    assert_eq!(outcome(&asked.records), Some(Outcome::Withdrawn));
}
