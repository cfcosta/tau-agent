//! The notify slot's `ctx.desktop.notice` (ADR 0050): it shows a notice
//! and hands it what opens the notice's chat, once; no other reach has it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::{
    Arc,
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use serde_json::json;
use tau_luau_plugins_host::{
    desktop::{DesktopReach, Opened},
    runtime::{Context, Files, Hook, NoReach, load},
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

const NOTIFIER: &str = r#"
local tau = require("tau")
return tau.plugin({
  fills = { "tau.notify" },
  notify = function(notice, ctx)
    if ctx.desktop == nil then
      error("no desktop", 0)
    end
    ctx.desktop.notice({ title = notice.title, body = notice.body })
    ctx.desktop.notice({ title = "again" })
  end,
})
"#;

/// What the test's show saw, and opened at once, as a click would.
static SHOWN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn show(title: String, body: String, opened: Opened) {
    SHOWN.lock().unwrap().push((title, body));
    opened();
}

/// Each notice shows; the first opens the chat, and only once.
#[test]
fn a_notice_shows_and_opens_its_chat_once() {
    block_on_io(async {
        let files = Files {
            source: NOTIFIER.into(),
            ..Files::named("notifier")
        };
        let loaded = load("notifier", files).await.unwrap();
        let opened = Arc::new(AtomicUsize::new(0));
        let counted = opened.clone();
        let reach = DesktopReach::new(
            Arc::new(NoReach),
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .showing(show);
        let outcome = loaded
            .call(
                &Hook::Notify,
                json!({ "title": "tau-agent is ready to land", "body": "retry-after" }),
                &Context::default(),
                Arc::new(reach),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(outcome.error, None);
        assert_eq!(
            *SHOWN.lock().unwrap(),
            [
                (
                    "tau-agent is ready to land".to_owned(),
                    "retry-after".to_owned()
                ),
                ("again".to_owned(), String::new()),
            ]
        );
        assert_eq!(opened.load(Ordering::SeqCst), 1);
    });
}

/// Without the slot's reach there is no `ctx.desktop`.
#[test]
fn only_the_notify_slot_reaches_the_desktop() {
    block_on_io(async {
        let files = Files {
            source: NOTIFIER.into(),
            ..Files::named("notifier")
        };
        let loaded = load("notifier", files).await.unwrap();
        let outcome = loaded
            .call(
                &Hook::Notify,
                json!({ "title": "t", "body": "b" }),
                &Context::default(),
                Arc::new(NoReach),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(outcome.error.as_deref(), Some("no desktop"));
    });
}
