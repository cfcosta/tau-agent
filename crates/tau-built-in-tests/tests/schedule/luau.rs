//! tau-schedule's Luau loads as a plugin and passes its own tests, as tau
//! loads it; and through the real host (ADR 0050, gap D), a chat's tool
//! keeps a wake-up in the repository's store and the job wakes the chat
//! on it, in the host's local time, letting go of the store first.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tau_luau_plugins_host::{
    runtime::{Context, Hook, JobRef, Loaded, NoReach, StoreRef, load},
    store::Stores,
};
use tau_testing::block_on_io;
use tau_ui_plugin::{Conversations, Turn, Waking, registry::HostFuture};
use tokio_util::sync::CancellationToken;

fn loaded() -> Loaded {
    let files = tau_luau_plugins_host::built_in::files(
        tau_luau_plugins::names::SCHEDULE,
    )
    .unwrap();
    block_on_io(load(tau_luau_plugins::names::SCHEDULE, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    assert_eq!(loaded.declaration.name, tau_luau_plugins::names::SCHEDULE);
    assert!(loaded.declaration.uses.chats);
    assert_eq!(loaded.declaration.jobs[0].every, 1);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 10, "{tests:?}");
    let failed: Vec<_> = tests.iter().filter(|test| !test.passed).collect();
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Chats that keep what woke them, and whose turn, as a real one does,
/// opens the plugin's store in the chat's repository as it starts.
struct Chats {
    stores: Arc<Stores>,
    woken: Mutex<Vec<(String, String)>>,
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

    fn wake(
        &self,
        run: &str,
        text: String,
    ) -> HostFuture<'_, anyhow::Result<Waking>> {
        self.woken.lock().unwrap().push((run.to_owned(), text));
        Box::pin(async move {
            let kv = self
                .stores
                .of(tau_luau_plugins::names::SCHEDULE, "repo")
                .await
                .map_err(anyhow::Error::msg)?;
            drop(kv.session().await);
            Ok(Waking::Started)
        })
    }
}

/// `hook` with `input`: its value, after its store writes are kept.
fn call(loaded: &Loaded, hook: Hook, input: Value, context: &Context) -> Value {
    let outcome = block_on_io(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            loaded.call(
                &hook,
                input,
                context,
                Arc::new(NoReach),
                CancellationToken::new(),
            ),
        )
        .await
        .expect("the hook ends")
    });
    assert_eq!(outcome.error, None, "{hook:?}");
    outcome.value
}

#[test]
fn a_chat_is_woken_on_its_note_through_the_host() {
    let loaded = loaded();
    let stores = Arc::new(Stores::memory());
    let kv = block_on_io(stores.of(tau_luau_plugins::names::SCHEDULE, "repo"))
        .unwrap();
    let now = tau_luau_plugins_host::agent::now();
    let unix = now["unix"].as_i64().unwrap();
    let in_chat = Context {
        run: json!({ "id": "r1", "kind": "chat", "repo": "repo", "call": "call_1" }),
        now: now.clone(),
        settings: json!({}),
        state: json!({}),
        secrets: None,
        store: Some(StoreRef(kv.clone())),
        job: None,
    };
    let answer = call(
        &loaded,
        Hook::Tool("schedule".into()),
        json!({ "name": "recheck", "note": "Did #412 merge?", "in": "2h" }),
        &in_chat,
    );
    let local = |at: i64| tau_clock::local(&tau_clock::SystemClock, at);
    assert_eq!(
        answer,
        json!(format!(
            "Scheduled `recheck` for {}. This chat wakes then with your note.",
            local(unix + 7200)
        ))
    );
    // Two hours on: the wake-up came due.
    let mut later = tau_luau_plugins_host::agent::at(unix + 7200);
    later["local"] = json!(local(unix + 7200));
    let chats = Arc::new(Chats {
        stores: stores.clone(),
        woken: Mutex::default(),
    });
    let in_job = Context {
        run: json!({}),
        now: later,
        settings: json!({}),
        state: json!({}),
        secrets: None,
        store: None,
        job: Some(JobRef {
            stores: stores.clone(),
            conversations: Some(chats.clone()),
        }),
    };
    call(&loaded, Hook::Job("wake".into()), Value::Null, &in_job);
    assert_eq!(
        *chats.woken.lock().unwrap(),
        [(
            "r1".to_owned(),
            format!(
                "[scheduled · recheck · {}]\nDid #412 merge?",
                local(unix + 7200)
            )
        )]
    );
    let left = block_on_io(kv.everything()).unwrap();
    assert!(left.is_empty(), "{left:?}");
}

/// When the demo's fixture was made: 2026-10-12 12:00 UTC. The demo
/// moves each time in it by how far its own now is from this.
const DEMO_NOW: i64 = 1_791_806_400;

/// The demo's chat: it scheduled two looks ahead, and woke on a third.
fn demo(loaded: &Loaded) -> Value {
    let stores = Arc::new(Stores::memory());
    let kv = block_on_io(stores.of(tau_luau_plugins::names::SCHEDULE, "repo"))
        .unwrap();
    let mut now = tau_luau_plugins_host::agent::at(DEMO_NOW);
    now["local"] = json!("2026-10-12 12:00");
    now["offset"] = json!(0);
    let mut context = Context {
        run: json!({ "id": "scheduled-review", "kind": "chat", "repo": "repo", "call": "call_1" }),
        now,
        settings: json!({}),
        state: json!({}),
        secrets: None,
        store: Some(StoreRef(kv)),
        job: None,
    };
    let step = |hook: Hook, input: Value, context: &mut Context| {
        let outcome = block_on_io(loaded.call(
            &hook,
            input,
            context,
            Arc::new(NoReach),
            CancellationToken::new(),
        ));
        assert_eq!(outcome.error, None, "{hook:?}");
        context.state = outcome.state;
        outcome.value
    };
    for (name, after, note) in DEMO_WAKES {
        step(
            Hook::Tool("schedule".into()),
            json!({ "name": name, "note": note, "in": after }),
            &mut context,
        );
    }
    step(Hook::Start, json!({ "input": DEMO_WOKEN }), &mut context);
    let view = step(Hook::View, Value::Null, &mut context);
    json!({ "now": DEMO_NOW, "state": context.state, "view": view })
}

/// What the demo's chat scheduled: name, how far ahead, note.
const DEMO_WAKES: [(&str, &str, &str); 2] = [
    (
        "recheck-415",
        "2h",
        "If #415 has its second approval, land it; if CI is red, read the \
         failing job before touching anything.",
    ),
    (
        "release-notes",
        "26h",
        "Tomorrow's release: check the notes mention the plugins tree \
         change from #412.",
    ),
];

/// The message the demo's chat woke on.
const DEMO_WOKEN: &str = "[scheduled · look-at-415 · 2026-10-12 14:00]\n\
    Read #415's review. Answer what you can; if a change is asked for, \
    make it and push.";

/// The chat the demo shows is what the plugin leaves and draws.
/// `BLESS_FIXTURES=1` writes it anew from the plugin.
#[test]
fn the_demo_is_what_the_plugin_leaves_and_draws() {
    let loaded = loaded();
    let drawn = demo(&loaded);
    if std::env::var_os("BLESS_FIXTURES").is_some() {
        let path = crate::fixture_path("tau-schedule", "scheduled");
        let text = serde_json::to_string_pretty(&drawn).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: Value =
        serde_json::from_str(crate::fixture("tau-schedule", "scheduled"))
            .unwrap();
    assert_eq!(fixture, drawn, "run with BLESS_FIXTURES=1");
}

/// What the demo lists of the plugin is what it declares.
/// `BLESS_FIXTURES=1` writes it anew from the plugin.
#[test]
fn the_demo_lists_what_the_plugin_declares() {
    let declared = serde_json::to_value(&loaded().declaration).unwrap();
    if std::env::var_os("BLESS_FIXTURES").is_some() {
        let path = crate::fixture_path("tau-schedule", "declaration");
        let text = serde_json::to_string_pretty(&declared).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: Value =
        serde_json::from_str(crate::fixture("tau-schedule", "declaration"))
            .unwrap();
    assert_eq!(fixture, declared, "run with BLESS_FIXTURES=1");
}
