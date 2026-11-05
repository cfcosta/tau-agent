//! The window as phones mirror it (ADR 0046), the kernel's half: what
//! crosses to phones, what a phone may ask, and the window acting on it
//! as on its own. Whatever carries it to phones reaches the window only
//! through the [`App`] service this offers (tau-phones).

use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use gpui::Entity;
use serde_json::Value;
use tau_ui_plugin::{
    app::{App, Feed, Reply, Sink, Up},
    wire::ViewKey,
};
use tokio::sync::mpsc;

use crate::{
    phones::PhoneUp,
    update::HostUpdate,
    workspace::{Workspace, WorkspaceEvent},
};

/// [`App`] on this computer: what plugins are given. What it is asked
/// waits for the window ([`Mirrored::attach`]).
pub struct Mirror {
    sink: Arc<Mutex<Option<Sink>>>,
    asks: mpsc::UnboundedSender<Ask>,
}

/// What the window is asked, on its own thread.
enum Ask {
    Mirrored(bool),
    Snapshot(Reply),
    Event(WorkspaceEvent),
    Views { device: String, keys: Vec<ViewKey> },
    DropViews { device: String, keys: Vec<ViewKey> },
    Left(String),
}

/// The window's half of a [`Mirror`]: what it was asked, and where what
/// it applies goes.
pub struct Mirrored {
    sink: Arc<Mutex<Option<Sink>>>,
    asked: mpsc::UnboundedReceiver<Ask>,
}

impl Mirror {
    pub fn new() -> (Arc<Self>, Mirrored) {
        let sink = Arc::default();
        let (asks, asked) = mpsc::unbounded_channel();
        let mirror = Self {
            sink: Arc::clone(&sink),
            asks,
        };
        (Arc::new(mirror), Mirrored { sink, asked })
    }

    fn ask(&self, ask: Ask) {
        // A window that closed asks nothing more.
        let _ = self.asks.send(ask);
    }
}

impl App for Mirror {
    fn mirror(&self, sink: Option<Sink>) {
        let on = sink.is_some();
        *self.sink.lock().expect("not poisoned") = sink;
        self.ask(Ask::Mirrored(on));
    }

    fn snapshot(&self, reply: Reply) {
        self.ask(Ask::Snapshot(reply));
    }

    fn up(&self, device: &str, body: Value) -> Up {
        let device = device.to_owned();
        match from_phone(body) {
            // The plugins' views a phone draws, built here for it, and
            // those it stopped drawing.
            Some(PhoneUp::Event(WorkspaceEvent::AskViews(keys))) => {
                self.ask(Ask::Views { device, keys })
            }
            Some(PhoneUp::Event(WorkspaceEvent::DropViews(keys))) => {
                self.ask(Ask::DropViews { device, keys })
            }
            Some(PhoneUp::Event(event)) => self.ask(Ask::Event(event)),
            Some(PhoneUp::Name(name)) => return Up::Renamed(name),
            None => {}
        }
        Up::Taken
    }

    fn left(&self, device: &str) {
        self.ask(Ask::Left(device.to_owned()));
    }
}

impl Mirrored {
    /// Answers what the mirror is asked from `workspace`, and sends what
    /// it applies to the mirror's sink while it has one.
    pub fn attach(self, workspace: &Entity<Workspace>, cx: &mut gpui::App) {
        let Self { sink, mut asked } = self;
        let send = {
            let sink = Arc::clone(&sink);
            move |feed: Feed| {
                let sink = sink.lock().expect("not poisoned").clone();
                if let Some(sink) = sink {
                    sink(feed);
                }
            }
        };
        // Phones' views wait to be built again after what the host
        // applied: one rebuild for a burst of updates.
        let refreshing = Rc::new(Cell::new(false));

        // What the host applies goes to every phone; an answer to a
        // request goes to the phones there now, unnumbered and not kept.
        // After it, the views phones asked for are built again.
        let echo = send.clone();
        cx.subscribe(workspace, move |workspace, update: &HostUpdate, cx| {
            if let Some(body) = to_phones(update) {
                echo(Feed {
                    body,
                    transient: transient(update),
                });
            }
            if !matches!(update, HostUpdate::Views { .. }) {
                refresh_views(&refreshing, &echo, &workspace, cx);
            }
        })
        .detach();

        // A phone's click, command or key: carried out here, and what is
        // for its window goes back to it.
        cx.subscribe(workspace, |workspace, event: &WorkspaceEvent, cx| {
            if let WorkspaceEvent::PluginCall {
                id,
                plugin,
                origin,
                layout,
                call,
            } = event
            {
                workspace.update(cx, |ws, cx| {
                    ws.answer_call(*id, plugin, origin, *layout, call, cx)
                });
            }
        })
        .detach();

        let workspace = workspace.downgrade();
        cx.spawn(async move |cx| {
            while let Some(ask) = asked.recv().await {
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                cx.update(|cx| answer(ask, &send, &workspace, cx));
            }
        })
        .detach();
    }
}

/// A mirror of `workspace`, for a window with no host of its own.
pub fn app(workspace: &Entity<Workspace>, cx: &mut gpui::App) -> Arc<dyn App> {
    let (mirror, mirrored) = Mirror::new();
    mirrored.attach(workspace, cx);
    mirror
}

fn answer(
    ask: Ask,
    send: &impl Fn(Feed),
    workspace: &Entity<Workspace>,
    cx: &mut gpui::App,
) {
    match ask {
        Ask::Mirrored(on) => workspace.update(cx, |ws, _| {
            ws.set_mirrored(on);
            if !on {
                ws.forget_views();
            }
        }),
        Ask::Snapshot(reply) => reply(snapshot(workspace.read(cx))),
        Ask::Event(event) => workspace.update(cx, |_, cx| cx.emit(event)),
        Ask::Views { device, keys } => {
            let answers =
                workspace.update(cx, |ws, _| ws.ask_views(&device, keys));
            if let Some(body) = to_phones(&answers) {
                send(Feed {
                    body,
                    transient: true,
                });
            }
        }
        Ask::DropViews { device, keys } => {
            workspace.update(cx, |ws, _| ws.drop_views(&device, &keys))
        }
        Ask::Left(device) => {
            workspace.update(cx, |ws, _| ws.forget_phone(&device))
        }
    }
}

/// Builds the views phones asked for again once what the host applies
/// now is in, and sends those that changed.
fn refresh_views(
    refreshing: &Rc<Cell<bool>>,
    send: &(impl Fn(Feed) + Clone + 'static),
    workspace: &Entity<Workspace>,
    cx: &mut gpui::App,
) {
    if refreshing.replace(true) {
        return;
    }
    let (refreshing, send, workspace) =
        (refreshing.clone(), send.clone(), workspace.downgrade());
    cx.defer(move |cx| {
        refreshing.set(false);
        let changed = workspace
            .update(cx, |ws, _| ws.refresh_views())
            .ok()
            .flatten();
        if let Some(body) = changed.as_ref().and_then(to_phones) {
            send(Feed {
                body,
                transient: true,
            });
        }
    });
}

/// What phones are sent of `update`: all but onboarding, which stays
/// on the computer, and what plugins only the computer may reach keep
/// (`HostUpdate::for_phones`).
pub fn to_phones(update: &HostUpdate) -> Option<Value> {
    if matches!(update, HostUpdate::Setup(_)) {
        return None;
    }
    serde_json::to_value(update.clone().for_phones())
        .inspect_err(|error| {
            eprintln!("tau-ui: cannot send an update: {error}")
        })
        .ok()
}

/// Whether `update` answers a request, which a phone that missed it
/// asks again: an image's bytes, a plugin's reply. Not kept for phones
/// that come back, so the feed holds no image.
pub fn transient(update: &HostUpdate) -> bool {
    matches!(
        update,
        HostUpdate::Picture { .. }
            | HostUpdate::PluginReply { .. }
            | HostUpdate::PluginFetched { .. }
            | HostUpdate::Views { .. }
            | HostUpdate::Called { .. }
    )
}

/// What a phone that needs everything is sent: what `workspace` shows.
pub fn snapshot(workspace: &Workspace) -> Vec<Value> {
    to_phones(&workspace.snapshot()).into_iter().collect()
}

/// What a phone sent, if it is something a phone may ask.
pub fn from_phone(body: Value) -> Option<PhoneUp> {
    match serde_json::from_value::<PhoneUp>(body) {
        Ok(PhoneUp::Event(event)) if !event.from_phone() => None,
        // A plugin that only the computer may ask (`computer_only`).
        Ok(PhoneUp::Event(
            WorkspaceEvent::PluginAct { plugin, .. }
            | WorkspaceEvent::PluginCall { plugin, .. },
        )) if tau_ui_remote::plugins::registry()
            .get(&plugin)
            .is_some_and(|erased| erased.declaration().reach.computer_only) =>
        {
            None
        }
        Ok(up) => Some(up),
        Err(error) => {
            eprintln!("tau-ui: a phone sent what tau cannot read: {error}");
            None
        }
    }
}
