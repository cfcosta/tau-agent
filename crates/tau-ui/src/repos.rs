//! Repositories in the workspace: which one is selected and which are
//! open in the sidebar, their runs, and adding and removing them. Each
//! repository has its own runs, memory and constitution.

use std::sync::LazyLock;

use gpui::{Context, Window};
use tau_agent::tool::RunId;

use crate::{
    catalog::Repo,
    route::Route,
    view::{Origin, RunView},
    workspace::{Workspace, WorkspaceEvent},
};

/// How many runs an open repository lists before "Show older runs".
pub const RUNS_SHOWN: usize = 5;

/// What a repository shows in the sidebar.
pub struct RepoRows<'a> {
    pub repo: &'a Repo,
    pub open: bool,
    /// The runs listed: the main chat first, then others newest first;
    /// forks and sub-agents go under them.
    pub runs: Vec<&'a RunView>,
    /// How many of the main chat's chats are listed under it, newest
    /// first; `None` when there is no main chat or all are.
    pub main_children: Option<usize>,
    /// The runs are a filter's matches, listed without what is under
    /// them.
    pub flat: bool,
    /// Older runs not listed.
    pub older: usize,
    /// All its runs.
    pub total: usize,
    /// Its runs still going.
    pub live: usize,
}

static NO_REPO: LazyLock<Repo> = LazyLock::new(Repo::default);

impl Workspace {
    /// The repository `run` works on: its own, or the first listed when
    /// the host did not say.
    pub fn repo_of<'a>(&'a self, run: &'a RunView) -> &'a str {
        if run.repo.is_empty() {
            self.catalog.repos.first().map_or("", |repo| &repo.name)
        } else {
            &run.repo
        }
    }

    /// The repository new runs start in: the one last selected, else the
    /// first listed.
    pub fn selected_repo(&self) -> Option<&str> {
        self.repo
            .as_deref()
            .filter(|name| self.catalog.repo(name).is_some())
            .or_else(|| self.catalog.repos.first().map(|repo| &*repo.name))
    }

    /// A repository by name, or an empty one when it is not listed.
    pub fn repo_named(&self, name: &str) -> &Repo {
        self.catalog.repo(name).unwrap_or(&NO_REPO)
    }

    /// Whether `run` is a repository's main chat, which every other chat
    /// in the repository forks from and which cannot be closed.
    pub fn is_main(&self, run: &RunId) -> bool {
        self.catalog
            .repos
            .iter()
            .any(|repo| repo.main.as_ref() == Some(run))
    }

    /// `run`'s open forks and sub-agents that have a view of their own,
    /// newest first.
    pub fn open_children<'a>(
        &'a self,
        run: &'a RunView,
    ) -> impl Iterator<Item = &'a RunView> {
        self.runs.iter().filter(move |view| {
            view.origin.parent() == Some(&run.id)
                && !self.closed.contains(&view.id)
        })
    }

    pub fn is_repo_open(&self, name: &str) -> bool {
        self.open_repos.contains(name)
    }

    /// Runs in `repo` that are not a fork or sub-agent of another: its
    /// main chat first, then the others newest first.
    pub fn root_runs<'a>(
        &'a self,
        repo: &'a str,
    ) -> impl Iterator<Item = &'a RunView> {
        let main = self.repo_named(repo).main.clone();
        let main_run = main.as_ref().and_then(|id| self.run(id));
        main_run
            .into_iter()
            .chain(self.runs.iter().filter(move |run| {
                run.origin == Origin::Root
                    && Some(&run.id) != main.as_ref()
                    && self.repo_of(run) == repo
                    && !self.closed.contains(&run.id)
            }))
    }

    /// The sidebar's tree. A filter keeps repositories whose name
    /// matches, and opens the others on their runs that match.
    pub fn repo_rows(&self, filter: &str) -> Vec<RepoRows<'_>> {
        let filter = filter.trim().to_lowercase();
        self.catalog
            .repos
            .iter()
            .filter_map(|repo| {
                let all: Vec<&RunView> = self.root_runs(&repo.name).collect();
                let live = self
                    .runs
                    .iter()
                    .filter(|run| {
                        run.status.is_live() && self.repo_of(run) == repo.name
                    })
                    .count();
                let everything = self.all_runs.contains(&repo.name);
                // With a main chat, the chats under it are what gets long.
                let main = repo.main.as_ref().and_then(|id| self.run(id));
                let chats =
                    main.map_or(0, |main| self.open_children(main).count());
                let total = all.len() + chats;
                let named = repo.name.to_lowercase().contains(&filter);
                if filter.is_empty() || named {
                    let open = self.is_repo_open(&repo.name);
                    if main.is_some() {
                        let shown = if everything {
                            chats
                        } else {
                            chats.min(RUNS_SHOWN)
                        };
                        return Some(RepoRows {
                            repo,
                            open,
                            runs: all,
                            main_children: (shown < chats).then_some(shown),
                            flat: false,
                            older: chats - shown,
                            total,
                            live,
                        });
                    }
                    let shown = if everything {
                        total
                    } else {
                        total.min(RUNS_SHOWN)
                    };
                    return Some(RepoRows {
                        repo,
                        open,
                        runs: all[..shown].to_vec(),
                        main_children: None,
                        flat: false,
                        older: total - shown,
                        total,
                        live,
                    });
                }
                // Any conversation in the repository whose title matches,
                // listed on its own.
                let runs: Vec<&RunView> = self
                    .runs
                    .iter()
                    .filter(|run| {
                        self.repo_of(run) == repo.name
                            && !self.closed.contains(&run.id)
                            && run.title.to_lowercase().contains(&filter)
                    })
                    .collect();
                (!runs.is_empty()).then_some(RepoRows {
                    repo,
                    open: true,
                    runs,
                    main_children: None,
                    flat: true,
                    older: 0,
                    total,
                    live,
                })
            })
            .collect()
    }

    /// Opens the repositories the sidebar had open last time, or the
    /// selected one on a first start, and forgets any no longer listed.
    pub(crate) fn restore_repos(&mut self) {
        let listed = |name: &String| self.catalog.repo(name).is_some();
        let mut open: std::collections::HashSet<String> = self
            .open_repos
            .iter()
            .filter(|name| listed(name))
            .cloned()
            .collect();
        if open.is_empty() {
            open = self
                .catalog
                .open_repos
                .iter()
                .filter(|name| listed(name))
                .cloned()
                .collect();
        }
        if self.repo.as_ref().is_none_or(|name| !listed(name)) {
            self.repo = self
                .current()
                .map(|run| self.repo_of(run).to_owned())
                .filter(|name| listed(name))
                .or_else(|| {
                    self.catalog.repos.first().map(|repo| repo.name.clone())
                });
        }
        if open.is_empty() {
            open.extend(self.repo.clone());
        }
        self.open_repos = open;
    }

    pub(crate) fn emit_open_repos(&self, cx: &mut Context<Self>) {
        // In the sidebar's order, so what is saved reads well.
        let open = self
            .catalog
            .repos
            .iter()
            .filter(|repo| self.open_repos.contains(&repo.name))
            .map(|repo| repo.name.clone())
            .collect();
        cx.emit(WorkspaceEvent::OpenRepos(open));
    }

    /// Opens or closes a repository in the sidebar, and selects it.
    pub fn toggle_repo_open(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.open_repos.remove(name) {
            self.open_repos.insert(name.to_owned());
        }
        self.repo = Some(name.to_owned());
        self.repo_menu = None;
        self.emit_open_repos(cx);
        cx.notify();
    }

    /// Lists all of a repository's runs, not only the newest.
    pub fn show_older_runs(&mut self, name: &str, cx: &mut Context<Self>) {
        self.all_runs.insert(name.to_owned());
        cx.notify();
    }

    /// Starts writing a new run in `name`.
    pub fn new_run_in(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.repo = Some(name.to_owned());
        self.repo_menu = None;
        self.start_new_run(window, cx);
    }

    /// Opens `repo`'s notes.
    pub fn open_memory(&mut self, repo: &str, cx: &mut Context<Self>) {
        self.navigate(
            Route::Plugin {
                plugin: tau_memory::plugin::NAME.into(),
                page: "notes".into(),
                params: [("repo".to_owned(), repo.to_owned())].into(),
            },
            cx,
        );
    }

    pub fn hover_repo(
        &mut self,
        name: &str,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let now = if hovered {
            Some(name.to_owned())
        } else if self.hovered_repo.as_deref() == Some(name) {
            None
        } else {
            return;
        };
        if self.hovered_repo != now {
            self.hovered_repo = now;
            cx.notify();
        }
    }

    pub fn toggle_repo_menu(&mut self, name: &str, cx: &mut Context<Self>) {
        self.repo_menu = match self.repo_menu.as_deref() {
            Some(open) if open == name => None,
            _ => Some(name.to_owned()),
        };
        cx.notify();
    }

    pub fn close_repo_menu(&mut self, cx: &mut Context<Self>) {
        if self.repo_menu.take().is_some() {
            cx.notify();
        }
    }

    /// Asks the host to bring in the repository's new commits.
    pub fn update_repo(&mut self, name: &str, cx: &mut Context<Self>) {
        self.repo_menu = None;
        cx.emit(WorkspaceEvent::UpdateRepo {
            repo: name.to_owned(),
        });
        cx.notify();
    }

    /// Opens the repository's clone in the file manager.
    pub fn show_in_files(&mut self, name: &str, cx: &mut Context<Self>) {
        let path = self.repo_named(name).path.clone();
        self.repo_menu = None;
        if !path.is_empty() {
            cx.reveal_path(&expand_home(&path));
        }
        cx.notify();
    }

    /// Stops listing a repository. Its runs stay in the store and in
    /// History; the host keeps its project.
    pub fn remove_repo(&mut self, name: &str, cx: &mut Context<Self>) {
        self.catalog.repos.retain(|repo| repo.name != name);
        self.open_repos.remove(name);
        self.repo_menu = None;
        if self.repo.as_deref() == Some(name) {
            self.repo = None;
            self.restore_repos();
        }
        if self.route.repo() == Some(name) {
            self.back_stack.clear();
            self.route = Route::Home;
        }
        cx.emit(WorkspaceEvent::HideRepo {
            repo: name.to_owned(),
        });
        self.emit_open_repos(cx);
        cx.notify();
    }

    /// A repository the host added: listed last, open and selected. One
    /// already listed under the name is replaced.
    pub fn add_repo(&mut self, repo: Repo, cx: &mut Context<Self>) {
        let name = repo.name.clone();
        match self.catalog.repo_mut(&name) {
            Some(listed) => *listed = repo,
            None => self.catalog.repos.push(repo),
        }
        self.open_repos.insert(name.clone());
        self.repo = Some(name);
        self.emit_open_repos(cx);
        cx.notify();
    }
}

/// `~/x` as a path under the home directory.
pub(crate) fn expand_home(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => std::path::PathBuf::from(home).join(rest),
        _ => std::path::PathBuf::from(path),
    }
}
