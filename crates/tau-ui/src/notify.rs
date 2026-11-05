//! Desktop notifications, while tau's window is not focused: a chat
//! asks the person something, a chat is ready to land, a turn left
//! conflicts on main, a run failed, or something of a plugin's waits on
//! the person: an agent's note, question or spent day (ADR 0037).
//!
//! [`Notifier`] decides when: one notice each time a chat's state turns
//! into one of those, never per frame, and none for what was already so
//! when tau first saw the chat. [`Sink`] is where notices go: the plugin
//! that fills the `tau.notify` slot (ADR 0045; tau-notify's desktop
//! notifications, which its settings switch off), or a test's list.
//! Opening a notice opens its chat and brings tau's window up.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use gpui::{
    AnyWindowHandle,
    App,
    AppContext as _,
    BorrowAppContext as _,
    Entity,
    Global,
};
use tau_agent::tool::RunId;
use tau_ui_plugin::{
    Waiting,
    notify::{self, NOTIFY},
    slots::Fills,
    waiting::Watch,
};
use tau_ui_remote::attention::{Attention, count};
use tokio::sync::mpsc;

use crate::{Workspace, route::Route};

/// What a notice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A chat waits on the person's answer.
    Asks,
    /// A finished fork would land cleanly.
    ReadyToLand,
    /// A turn left conflicts on a repository's main chat.
    ConflictsOnMain,
    /// A run stopped with an error.
    Failed,
    /// Something of a plugin's waits on the person (`WAITING`): an
    /// agent's note, question or spent day.
    Waiting,
}

impl Kind {
    /// The notice a chat in `attention` calls for, if any.
    pub fn of(attention: &Attention) -> Option<Self> {
        match attention {
            Attention::Asks { .. } => Some(Self::Asks),
            Attention::ReadyToLand { .. } => Some(Self::ReadyToLand),
            Attention::Failed => Some(Self::Failed),
            // Conflicts on main are said through the host's hook, once
            // per turn that leaves them ([`conflicts_on_main`]).
            Attention::Working { .. }
            | Attention::ConflictsOnMain { .. }
            | Attention::Queued(_)
            | Attention::WouldConflict { .. }
            | Attention::Interrupted
            | Attention::Landed
            | Attention::Dropped
            | Attention::Idle => None,
        }
    }
}

/// One notification: the chat it opens, and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub run: RunId,
    pub kind: Kind,
    pub title: String,
    pub body: String,
}

/// A chat as the notifier sees it: its state and the words a notice
/// about it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub run: RunId,
    pub title: String,
    pub repo: String,
    /// The run it lands on, as the sidebar calls it: `main`.
    pub target: String,
    pub attention: Attention,
}

impl Sighting {
    /// The notice for this sighting, if its state calls for one.
    fn notice(&self) -> Option<Notice> {
        let kind = Kind::of(&self.attention)?;
        let (title, body) = match &self.attention {
            Attention::Asks { question } => {
                (format!("{} asks you", self.title), question.clone())
            }
            Attention::ReadyToLand { changes } => (
                format!("{} is ready to land", self.title),
                format!(
                    "{} on {} · {}",
                    count(*changes, "change"),
                    self.target,
                    self.repo
                ),
            ),
            _ => (
                format!("{} failed", self.title),
                format!("The run stopped with an error · {}", self.repo),
            ),
        };
        Some(Notice {
            run: self.run.clone(),
            kind,
            title,
            body,
        })
    }
}

/// Where notices go.
pub trait Sink {
    fn show(&mut self, notice: Notice);
}

/// Decides when to notify: once each time a chat's state turns into one
/// that calls for a notice, while the window is not focused.
#[derive(Debug, Default)]
pub struct Notifier {
    /// The notice each chat's state called for when last seen.
    seen: HashMap<RunId, Option<Kind>>,
    /// What waited on the person when last seen.
    waiting: Watch,
}

impl Notifier {
    /// Takes in the chats as they are now. A chat whose state turned
    /// into one that calls for a notice gets one, unless the window is
    /// `focused`: the person sees it there. A chat seen for the first
    /// time gets none: it was so before. A chat no longer there is
    /// forgotten.
    pub fn observe(
        &mut self,
        sightings: &[Sighting],
        focused: bool,
        sink: &mut dyn Sink,
    ) {
        let mut seen = HashMap::with_capacity(sightings.len());
        for sighting in sightings {
            let kind = Kind::of(&sighting.attention);
            let before = self.seen.get(&sighting.run);
            let turned = before.is_some_and(|before| *before != kind);
            if turned
                && !focused
                && let Some(notice) = sighting.notice()
            {
                sink.show(notice);
            }
            seen.insert(sighting.run.clone(), kind);
        }
        self.seen = seen;
    }

    /// Takes in what waits on the person as it is now, by plugin: each
    /// thing that started waiting since gets a notice that opens its
    /// run, unless the window is `focused`.
    pub fn waiting(
        &mut self,
        waiting: &[(&str, Waiting)],
        focused: bool,
        sink: &mut dyn Sink,
    ) {
        for alert in self.waiting.observe(waiting) {
            let Some(run) = alert.run.filter(|_| !focused) else {
                continue;
            };
            sink.show(Notice {
                run,
                kind: Kind::Waiting,
                title: alert.title,
                body: alert.body,
            });
        }
    }

    /// A turn of `main`, `repo`'s main chat, left conflicts in `files`
    /// files: said each time, unless the window is `focused`.
    pub fn conflicts_left(
        &mut self,
        main: &RunId,
        repo: &str,
        files: usize,
        focused: bool,
        sink: &mut dyn Sink,
    ) {
        if focused {
            return;
        }
        sink.show(Notice {
            run: main.clone(),
            kind: Kind::ConflictsOnMain,
            title: "Conflicts are still on main".into(),
            body: format!(
                "tau's turn left conflicts in {} · {repo}",
                count(files, "file")
            ),
        });
    }
}

/// The chats of `ws` as the notifier sees them: every open one.
pub fn sightings(ws: &Workspace) -> Vec<Sighting> {
    ws.runs()
        .iter()
        .filter(|run| !ws.is_closed(&run.id))
        .map(|run| Sighting {
            run: run.id.clone(),
            title: run.title.clone(),
            repo: ws.repo_of(run).to_owned(),
            target: tau_ui_remote::ui::landing::target(ws, run)
                .unwrap_or_else(|| "main".to_owned()),
            attention: ws.attention(run),
        })
        .collect()
}

/// Notices given to the plugin that fills `tau.notify` as each goes, so
/// the person's pick counts at once. An opened notice's chat goes to
/// `clicked`.
pub struct Slotted {
    slots: Arc<Mutex<Fills>>,
    clicked: mpsc::UnboundedSender<RunId>,
}

impl Slotted {
    pub fn new(
        slots: Arc<Mutex<Fills>>,
        clicked: mpsc::UnboundedSender<RunId>,
    ) -> Self {
        Self { slots, clicked }
    }
}

impl Sink for Slotted {
    fn show(&mut self, notice: Notice) {
        let filler = self.slots.lock().expect("not poisoned").get(&NOTIFY);
        let Some(filler) = filler else {
            return;
        };
        let (clicked, run) = (self.clicked.clone(), notice.run);
        filler.show(
            notify::Notice {
                title: notice.title,
                body: notice.body,
            },
            Box::new(move || {
                let _ = clicked.send(run);
            }),
        );
    }
}

/// The app's notifications: the notifier, where its notices go, and
/// the window whose focus keeps them quiet.
struct Notifications {
    notifier: Notifier,
    sink: Box<dyn Sink>,
    window: AnyWindowHandle,
}

impl Global for Notifications {}

impl Notifications {
    fn focused(&self, cx: &App) -> bool {
        cx.active_window() == Some(self.window)
    }
}

/// Notifies from `workspace`'s chats, shown in `window`, while that
/// window is not focused, through what fills `slots`' `tau.notify`; an
/// opened notice opens its chat there.
pub fn follow(
    workspace: &Entity<Workspace>,
    window: AnyWindowHandle,
    slots: Arc<Mutex<Fills>>,
    cx: &mut App,
) {
    let (clicked, mut clicks) = mpsc::unbounded_channel();
    cx.set_global(Notifications {
        notifier: Notifier::default(),
        sink: Box::new(Slotted::new(slots, clicked)),
        window,
    });
    cx.observe(workspace, |workspace, cx| {
        let sightings = workspace.update(cx, |ws, _| sightings(ws));
        let waiting = workspace.read(cx).waiting();
        cx.update_global::<Notifications, _>(|notifications, cx| {
            let quiet = notifications.focused(cx);
            let Notifications { notifier, sink, .. } = notifications;
            notifier.observe(&sightings, quiet, sink.as_mut());
            notifier.waiting(&waiting, quiet, sink.as_mut());
        });
    })
    .detach();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        while let Some(run) = clicks.recv().await {
            let _ = cx
                .update_window(window, |_, window, _| window.activate_window());
            let _ =
                workspace.update(cx, |ws, cx| ws.navigate(Route::Run(run), cx));
        }
    })
    .detach();
}

/// Says that a turn of `main`, `repo`'s main chat, left conflicts in
/// `files`, unless tau's window is focused or nothing follows the
/// workspace ([`follow`]). The host calls it through its conflicts hook
/// (`Host::on_conflicts_on_main`).
pub fn conflicts_on_main(
    main: &RunId,
    repo: &str,
    files: &[String],
    cx: &mut App,
) {
    if !cx.has_global::<Notifications>() {
        return;
    }
    cx.update_global::<Notifications, _>(|notifications, cx| {
        let quiet = notifications.focused(cx);
        let Notifications { notifier, sink, .. } = notifications;
        notifier.conflicts_left(main, repo, files.len(), quiet, sink.as_mut());
    });
}
