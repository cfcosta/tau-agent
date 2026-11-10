//! tau-schedule through the real host (ADR 0050, gap D): a chat's tool
//! keeps a wake-up in the repository's store and the job wakes the chat
//! on it, in the host's local time, letting go of the store first. Its
//! Luau tests and fixtures are `tau-plugin test`'s (ADR 0053).

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
