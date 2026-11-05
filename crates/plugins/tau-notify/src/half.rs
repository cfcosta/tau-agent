//! tau-notify's host half: the desktop's notification server fills the
//! `tau.notify` slot.

use std::sync::Arc;

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{
    Group,
    HostCx,
    HostHalf,
    PluginHost,
    PluginInfo,
    RunCtx,
    notify::{NOTIFY, Notice, Notify},
    slots::Fills,
};

use crate::{NAME, NotifyUi, Settings};

/// The host half for tau's registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct NotifyHost;

/// What the host half keeps: the host's context, to read the person's
/// settings as each notice goes.
pub struct Host {
    cx: HostCx,
}

impl PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Self { cx: cx.clone() })
    }
}

impl HostHalf for NotifyHost {
    type Plugin = NotifyUi;
    type Host = Host;

    async fn agent_plugins(
        &self,
        _: &Host,
        _: &RunCtx,
        _: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        _: &Host,
        _: &HostCx,
        settings: &Settings,
    ) -> PluginInfo {
        PluginInfo {
            description: if settings.on {
                "Notices on the desktop while tau is in the background"
            } else {
                "Notices on the desktop: off"
            }
            .into(),
            group: Group::App,
            ..Default::default()
        }
    }

    fn fill(&self, host: &Host, fills: &mut Fills) {
        fills.fill(
            &NOTIFY,
            Arc::new(Desktop {
                cx: host.cx.clone(),
            }) as Arc<dyn Notify>,
        );
    }
}

/// Notices sent to the desktop's notification server, each on a thread
/// of its own that waits for a click.
struct Desktop {
    cx: HostCx,
}

impl Notify for Desktop {
    fn show(&self, notice: Notice, opened: Box<dyn FnOnce() + Send>) {
        if !self.cx.settings::<Settings>(NAME).on {
            return;
        }
        show(notice, opened);
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn show(notice: Notice, opened: Box<dyn FnOnce() + Send>) {
    let spawned = std::thread::Builder::new().name("tau-notice".into()).spawn(
        move || {
            let shown = notify_rust::Notification::new()
                .appname("tau")
                .summary(&notice.title)
                .body(&notice.body)
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
fn show(_: Notice, _: Box<dyn FnOnce() + Send>) {}
