//! Desktop notifications, while tau's window is not focused: a chat
//! asks the person something, a chat is ready to land, a turn left
//! conflicts on main, a run failed.
//!
//! [`Notifier`] decides when: one notice each time a chat's state turns
//! into one of those, never per frame, and none for what was already so
//! when tau first saw the chat. [`Sink`] is where notices go: the
//! desktop's notification server over D-Bus ([`Desktop`]), or a test's
//! list. Clicking a notice opens its chat and brings tau's window up.
//! `"notifications": false` in `interface.json` turns them off.

use std::collections::HashMap;

use gpui::{
    AnyWindowHandle,
    App,
    AppContext as _,
    BorrowAppContext as _,
    Entity,
    Global,
};
use tau_agent::tool::RunId;
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
pub fn sightings(ws: &Workspace, cx: &mut App) -> Vec<Sighting> {
    ws.runs()
        .iter()
        .filter(|run| !ws.is_closed(&run.id))
        .map(|run| Sighting {
            run: run.id.clone(),
            title: run.title.clone(),
            repo: ws.repo_of(run).to_owned(),
            target: tau_ui_remote::ui::landing::target(ws, run)
                .unwrap_or_else(|| "main".to_owned()),
            attention: ws.attention(run, cx),
        })
        .collect()
}

/// Notices sent to the desktop's notification server, each on a thread
/// of its own that waits for a click. A clicked notice's chat goes to
/// `clicked`.
pub struct Desktop {
    clicked: mpsc::UnboundedSender<RunId>,
}

impl Sink for Desktop {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn show(&mut self, notice: Notice) {
        let clicked = self.clicked.clone();
        let spawned = std::thread::Builder::new()
            .name("tau-notice".into())
            .spawn(move || {
                let shown = notify_rust::Notification::new()
                    .appname("tau")
                    .summary(&notice.title)
                    .body(&notice.body)
                    .action("default", "Open")
                    .show();
                match shown {
                    Ok(handle) => handle.wait_for_action(|action| {
                        if action == "default" {
                            let _ = clicked.send(notice.run.clone());
                        }
                    }),
                    Err(error) => {
                        eprintln!("tau-ui: cannot show a notification: {error}")
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("tau-ui: cannot show a notification: {error}");
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn show(&mut self, notice: Notice) {
        let _ = (&self.clicked, notice);
    }
}

/// The app's notifications: the notifier, where its notices go, and
/// the window whose focus keeps them quiet.
struct Notifications {
    notifier: Notifier,
    sink: Box<dyn Sink>,
    window: AnyWindowHandle,
    /// `interface.json`, read as each notice goes: `"notifications":
    /// false` turns them off.
    settings: std::path::PathBuf,
}

impl Global for Notifications {}

impl Notifications {
    fn focused(&self, cx: &App) -> bool {
        cx.active_window() == Some(self.window)
    }

    /// Whether the person left notifications on.
    fn on(&self) -> bool {
        tau_ui_remote::motion::saved_notifications(&self.settings)
    }
}

/// Notifies from `workspace`'s chats, shown in `window`, while that
/// window is not focused; a clicked notice opens its chat there.
pub fn follow(
    workspace: &Entity<Workspace>,
    window: AnyWindowHandle,
    settings: std::path::PathBuf,
    cx: &mut App,
) {
    let (clicked, mut clicks) = mpsc::unbounded_channel();
    cx.set_global(Notifications {
        notifier: Notifier::default(),
        sink: Box::new(Desktop { clicked }),
        window,
        settings,
    });
    cx.observe(workspace, |workspace, cx| {
        let sightings = workspace.update(cx, |ws, cx| sightings(ws, cx));
        cx.update_global::<Notifications, _>(|notifications, cx| {
            let quiet = notifications.focused(cx) || !notifications.on();
            let Notifications { notifier, sink, .. } = notifications;
            notifier.observe(&sightings, quiet, sink.as_mut());
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
        let quiet = notifications.focused(cx) || !notifications.on();
        let Notifications { notifier, sink, .. } = notifications;
        notifier.conflicts_left(main, repo, files.len(), quiet, sink.as_mut());
    });
}
