//! The desktop's notification server, for the plugin that fills
//! `tau.notify` (ADR 0050): its `notify` hook gets
//! `ctx.desktop.notice`, which shows one notice and opens its chat when
//! the person clicks it.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_ai::message::Usage;
use tau_codemode_host::{ToolCall, ToolEntry, ToolReply};
use tau_jev::Jev;

use crate::runtime::Reach;

/// What the notice's call is named among the host's calls.
pub const NOTICE: &str = "desktop_notice";

/// What opens a notice's chat; taken by the first notice shown.
pub type Opened = Box<dyn FnOnce() + Send>;

/// What shows a notice: [`show`], or a test's.
pub type Show = fn(String, String, Opened);

/// `reach`, with `desktop_notice` too: for one notice's `notify` hook.
pub struct DesktopReach {
    pub reach: Arc<dyn Reach>,
    opened: Mutex<Option<Opened>>,
    show: Show,
}

impl DesktopReach {
    pub fn new(reach: Arc<dyn Reach>, opened: Opened) -> Self {
        Self {
            reach,
            opened: Mutex::new(Some(opened)),
            show,
        }
    }

    /// Shows notices with `show` instead of on the desktop.
    pub fn showing(mut self, show: Show) -> Self {
        self.show = show;
        self
    }
}

#[async_trait]
impl Reach for DesktopReach {
    fn tools(&self) -> Vec<ToolEntry> {
        let mut tools = self.reach.tools();
        tools.push(ToolEntry {
            name: NOTICE.into(),
            description: "Shows `title` and `body` on the desktop; a click \
                          opens the notice's chat."
                .into(),
            input_schema: json!({ "type": "object" }),
            output_schema: None,
            namespace: None,
            sequential: true,
        });
        tools
    }

    async fn call(&self, call: ToolCall) -> Result<ToolReply, String> {
        if call.name != NOTICE {
            return self.reach.call(call).await;
        }
        let text = |key: &str| match &call.args[key] {
            Value::String(text) => Ok(text.clone()),
            Value::Null if key == "body" => Ok(String::new()),
            _ => Err(format!("`{NOTICE}` takes a `{key}` string")),
        };
        let (title, body) = (text("title")?, text("body")?);
        // A second notice of the same hook shows, but opens nothing.
        let opened = self
            .opened
            .lock()
            .expect("not poisoned")
            .take()
            .unwrap_or_else(|| Box::new(|| {}));
        (self.show)(title, body, opened);
        Ok(ToolReply::success(Value::Null))
    }

    fn jev(&self) -> Option<Arc<dyn Jev>> {
        self.reach.jev()
    }

    fn charge(&self, usage: &Usage) {
        self.reach.charge(usage);
    }
}

/// Shows a notice on a thread of its own, which waits for a click.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn show(title: String, body: String, opened: Opened) {
    let spawned = std::thread::Builder::new().name("tau-notice".into()).spawn(
        move || {
            let shown = notify_rust::Notification::new()
                .appname("tau")
                .summary(&title)
                .body(&body)
                .action("default", "Open")
                .show();
            match shown {
                Ok(handle) => {
                    let mut opened = Some(opened);
                    handle.wait_for_action(|action| {
                        if action == "default"
                            && let Some(opened) = opened.take()
                        {
                            opened();
                        }
                    });
                }
                Err(error) => {
                    eprintln!("tau-notify: cannot show a notification: {error}")
                }
            }
        },
    );
    if let Err(error) = spawned {
        eprintln!("tau-notify: cannot show a notification: {error}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn show(_: String, _: String, _: Opened) {}
