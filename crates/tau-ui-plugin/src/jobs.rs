//! Jobs the host keeps for plugins (ADR 0044): work a plugin starts and
//! the host runs past the call that started it, any window or page, with
//! a fraction and a line of progress the interface shows, until it ends
//! or is cancelled. A job that a restart cut off is the plugin's to
//! start again, from a checkpoint in its own directory
//! ([`crate::HostHalf::resume`]).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{
    host::{Push, RepoCtx},
    view::Link,
};

/// How often progress asks the interface to draw the catalog again, at
/// most: a job may report on every item it reads.
pub const REFRESH: Duration = Duration::from_millis(500);

/// How long a cancelled job has to stop by itself, writing its
/// checkpoint, before the host drops it.
pub const GRACE: Duration = Duration::from_secs(3);

/// A job going on, as the catalog lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    pub plugin: String,
    /// The plugin's name for it, one job of a name at a time.
    pub name: String,
    /// What it does, for the person: `Importing notes`.
    pub label: String,
    /// How far it is, from 0 to 1.
    pub fraction: f32,
    /// What it is doing now, in one line.
    pub line: String,
    /// The page that shows its work, when the job names one
    /// ([`JobCx::page`]): clicking the job in the status bar opens it.
    /// `None` opens the plugin's own page, if it has one.
    #[serde(default)]
    pub page: Option<Link>,
}

/// The repositories the host lists now. Clones share it.
#[derive(Clone)]
pub struct LiveRepos(Arc<dyn Fn() -> Vec<RepoCtx> + Send + Sync>);

impl LiveRepos {
    pub fn new(
        repos: impl Fn() -> Vec<RepoCtx> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(repos))
    }

    /// The repositories as they are now.
    pub fn now(&self) -> Vec<RepoCtx> {
        (self.0)()
    }
}

impl std::fmt::Debug for LiveRepos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveRepos")
    }
}

/// What a job reaches while it runs.
#[derive(Clone)]
pub struct JobCx {
    plugin: Arc<str>,
    name: Arc<str>,
    id: u64,
    cancel: CancellationToken,
    dir: PathBuf,
    jobs: Jobs,
}

impl JobCx {
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Says how far the job is, from 0 to 1, and what it does now. The
    /// interface shows it soon after ([`REFRESH`]).
    pub fn progress(&self, fraction: f32, line: impl Into<String>) {
        let fraction = if fraction.is_nan() {
            0.
        } else {
            fraction.clamp(0., 1.)
        };
        let line = line.into();
        let changed = {
            let mut running = self.jobs.0.running.lock().expect("not poisoned");
            match running.get_mut(&self.key()) {
                Some(job) if job.id == self.id => {
                    let changed =
                        job.info.fraction != fraction || job.info.line != line;
                    job.info.fraction = fraction;
                    job.info.line = line;
                    changed
                }
                _ => false,
            }
        };
        if changed {
            self.jobs.refresh_soon();
        }
    }

    /// Names the page that shows the job's work, a link of the plugin's
    /// own unless it names another's: clicking the job in the status bar
    /// opens it. A job may name it as it starts, or once it has
    /// something to show.
    pub fn page(&self, link: Link) {
        let changed = {
            let mut running = self.jobs.0.running.lock().expect("not poisoned");
            match running.get_mut(&self.key()) {
                Some(job) if job.id == self.id => {
                    let link = Some(link.from(&self.plugin));
                    let changed = job.info.page != link;
                    job.info.page = link;
                    changed
                }
                _ => false,
            }
        };
        if changed {
            self.jobs.refresh_soon();
        }
    }

    /// Whether the host asked the job to stop. It then has [`GRACE`] to
    /// write its checkpoint and return.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Waits until the host asks the job to stop.
    pub async fn cancelled(&self) {
        self.cancel.cancelled().await
    }

    /// The job's cancellation, for what it awaits: the person's model
    /// takes it ([`crate::PluginModel::ask`]).
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Where the plugin keeps its files, its checkpoints among them
    /// ([`crate::HostCx::plugin_dir`]).
    pub fn plugin_dir(&self) -> &Path {
        &self.dir
    }

    /// The repositories the host lists now, those added since the job
    /// started among them.
    pub fn repos(&self) -> Vec<RepoCtx> {
        self.jobs.0.repos.now()
    }

    fn key(&self) -> (String, String) {
        (self.plugin.to_string(), self.name.to_string())
    }
}

impl std::fmt::Debug for JobCx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobCx")
            .field("plugin", &self.plugin)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// The jobs the host keeps for plugins. A host service
/// ([`crate::HostCx::jobs`]); clones share them.
#[derive(Clone)]
pub struct Jobs(Arc<Inner>);

struct Inner {
    runtime: tokio::runtime::Handle,
    /// tau's directory for data shared by every repository, which each
    /// plugin's own is under.
    dir: PathBuf,
    repos: LiveRepos,
    push: Arc<dyn Fn(Push) + Send + Sync>,
    running: Mutex<BTreeMap<(String, String), Running>>,
    /// Makes each job's id its own, so one that ends never takes a later
    /// job of its name with it.
    next: std::sync::atomic::AtomicU64,
    refresh: Mutex<Refresh>,
}

struct Running {
    id: u64,
    info: JobInfo,
    cancel: CancellationToken,
}

/// When progress last asked for the catalog, and whether a request
/// waits to go.
#[derive(Default)]
struct Refresh {
    last: Option<Instant>,
    waiting: bool,
}

impl Jobs {
    /// Jobs that run on `runtime`, each plugin's under its directory in
    /// `dir`, reading the repositories from `repos`, telling the
    /// interface through `push`.
    pub fn new(
        runtime: tokio::runtime::Handle,
        dir: PathBuf,
        repos: LiveRepos,
        push: Arc<dyn Fn(Push) + Send + Sync>,
    ) -> Self {
        Self(Arc::new(Inner {
            runtime,
            dir,
            repos,
            push,
            running: Mutex::default(),
            next: Default::default(),
            refresh: Mutex::default(),
        }))
    }

    /// Starts `plugin`'s job `name`, which does what `label` says: `job`
    /// runs on the host's runtime until it returns or, once cancelled,
    /// for [`GRACE`] more. It fails when a job of that name is going.
    /// One that fails is said to the person as an alert.
    pub fn start<F, Fut>(
        &self,
        plugin: &str,
        name: &str,
        label: impl Into<String>,
        job: F,
    ) -> anyhow::Result<()>
    where
        F: FnOnce(JobCx) -> Fut,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        // Its clock is the runtime's, wherever it is called from.
        let _runtime = self.0.runtime.enter();
        let key = (plugin.to_owned(), name.to_owned());
        let id = self
            .0
            .next
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cancel = CancellationToken::new();
        let label = label.into();
        {
            let mut running = self.0.running.lock().expect("not poisoned");
            if running.contains_key(&key) {
                anyhow::bail!("{plugin} is running {name} already");
            }
            running.insert(
                key.clone(),
                Running {
                    id,
                    info: JobInfo {
                        plugin: plugin.to_owned(),
                        name: name.to_owned(),
                        label: label.clone(),
                        fraction: 0.,
                        line: String::new(),
                        page: None,
                    },
                    cancel: cancel.clone(),
                },
            );
        }
        let cx = JobCx {
            plugin: plugin.into(),
            name: name.into(),
            id,
            cancel: cancel.clone(),
            dir: self.0.dir.join("plugins").join(plugin),
            jobs: self.clone(),
        };
        let work = job(cx);
        let jobs = self.clone();
        self.0.runtime.spawn(async move {
            let stopped = async {
                cancel.cancelled().await;
                tokio::time::sleep(GRACE).await;
            };
            let ended = tokio::select! {
                ended = work => ended,
                () = stopped => Ok(()),
            };
            jobs.ended(&key, id);
            if let Err(error) = ended {
                (jobs.0.push)(Push::Alert {
                    title: format!("{label} failed"),
                    message: format!("{error:#}"),
                });
            }
        });
        self.refresh_now();
        Ok(())
    }

    /// Asks `plugin`'s job `name` to stop; false when none is going.
    pub fn cancel(&self, plugin: &str, name: &str) -> bool {
        let running = self.0.running.lock().expect("not poisoned");
        match running.get(&(plugin.to_owned(), name.to_owned())) {
            Some(job) => {
                job.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Whether `plugin`'s job `name` is going.
    pub fn is_running(&self, plugin: &str, name: &str) -> bool {
        self.0
            .running
            .lock()
            .expect("not poisoned")
            .contains_key(&(plugin.to_owned(), name.to_owned()))
    }

    /// The jobs going on, by plugin, then name.
    pub fn running(&self) -> Vec<JobInfo> {
        self.0
            .running
            .lock()
            .expect("not poisoned")
            .values()
            .map(|job| job.info.clone())
            .collect()
    }

    fn ended(&self, key: &(String, String), id: u64) {
        {
            let mut running = self.0.running.lock().expect("not poisoned");
            if running.get(key).is_some_and(|job| job.id == id) {
                running.remove(key);
            }
        }
        self.refresh_now();
    }

    /// Asks for the catalog now: a job started or ended.
    fn refresh_now(&self) {
        let _runtime = self.0.runtime.enter();
        self.0.refresh.lock().expect("not poisoned").last =
            Some(Instant::now());
        (self.0.push)(Push::Catalog);
    }

    /// Asks for the catalog at most once per [`REFRESH`]: now when the
    /// last ask is that old, else once it is, for what changed meanwhile.
    fn refresh_soon(&self) {
        let _runtime = self.0.runtime.enter();
        let wait = {
            let mut refresh = self.0.refresh.lock().expect("not poisoned");
            if refresh.waiting {
                return;
            }
            let now = Instant::now();
            match refresh.last {
                Some(last) if now < last + REFRESH => {
                    refresh.waiting = true;
                    last + REFRESH - now
                }
                _ => {
                    refresh.last = Some(now);
                    drop(refresh);
                    (self.0.push)(Push::Catalog);
                    return;
                }
            }
        };
        let jobs = self.clone();
        self.0.runtime.spawn(async move {
            tokio::time::sleep(wait).await;
            {
                let mut refresh = jobs.0.refresh.lock().expect("not poisoned");
                refresh.waiting = false;
                refresh.last = Some(Instant::now());
            }
            (jobs.0.push)(Push::Catalog);
        });
    }
}

impl std::fmt::Debug for Jobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let running = self.0.running.lock().map_or(0, |running| running.len());
        write!(f, "Jobs({running} running)")
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use super::*;

    /// A repository named `name`.
    fn repo(name: &str) -> RepoCtx {
        RepoCtx {
            id: crate::RepoId(name.into()),
            name: name.into(),
            remote: None,
            folder: None,
            checkout: PathBuf::from("/c").join(name),
            dir: PathBuf::from("/d").join(name),
            workspaces: PathBuf::from("/w").join(name),
        }
    }

    type Listed = Arc<Mutex<Vec<RepoCtx>>>;
    type Pushed = Arc<Mutex<Vec<Push>>>;

    /// Jobs on a runtime whose clock moves only when everything waits,
    /// over a list of repositories the test changes, with what they push.
    fn jobs() -> (tokio::runtime::Runtime, Jobs, Listed, Pushed) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        let (listed, pushed): (Listed, Pushed) = Default::default();
        let (live, heard) = (listed.clone(), pushed.clone());
        let jobs = Jobs::new(
            runtime.handle().clone(),
            PathBuf::from("/data/tau"),
            LiveRepos::new(move || live.lock().unwrap().clone()),
            Arc::new(move |push| heard.lock().unwrap().push(push)),
        );
        (runtime, jobs, listed, pushed)
    }

    fn catalogs(pushed: &Pushed) -> usize {
        pushed
            .lock()
            .unwrap()
            .iter()
            .filter(|push| **push == Push::Catalog)
            .count()
    }

    /// A job is listed with what it last said and the page it named, of
    /// its own plugin, one of a name at a time, in its plugin's directory,
    /// and leaves the list when it ends; a failure is said to the person.
    #[test]
    fn a_job_is_listed_while_it_runs() {
        let (runtime, jobs, _, pushed) = jobs();
        let (go, gone) = tokio::sync::oneshot::channel::<()>();
        jobs.start("p", "import", "Importing", |job| {
            job.page(Link::page("review").param("repo", "tau"));
            async move {
                assert_eq!(job.plugin_dir(), Path::new("/data/tau/plugins/p"));
                job.progress(0.25, "read 1 of 4");
                job.progress(f32::NAN, "nothing yet");
                job.progress(7., "read 4 of 4");
                let _ = gone.await;
                anyhow::bail!("no more")
            }
        })
        .unwrap();
        runtime.block_on(tokio::task::yield_now());
        assert_eq!(
            jobs.running(),
            [JobInfo {
                plugin: "p".into(),
                name: "import".into(),
                label: "Importing".into(),
                fraction: 1.,
                line: "read 4 of 4".into(),
                page: Some(Link::to("p", "review").param("repo", "tau")),
            }]
        );
        assert!(jobs.is_running("p", "import"));
        let again =
            jobs.start("p", "import", "Importing", |_| async { Ok(()) });
        assert!(again.is_err(), "one job of a name at a time");
        go.send(()).unwrap();
        runtime.block_on(async {
            tokio::time::sleep(Duration::from_millis(1)).await
        });
        assert!(jobs.running().is_empty());
        assert!(pushed.lock().unwrap().iter().any(|push| matches!(
            push,
            Push::Alert { title, message }
                if title == "Importing failed" && message == "no more"
        )));
    }

    /// Progress asks for the catalog at most once per [`REFRESH`], and
    /// once more after for what came meanwhile.
    #[test]
    fn progress_refreshes_the_catalog_at_most_so_often() {
        let (runtime, jobs, _, pushed) = jobs();
        jobs.start("p", "count", "Counting", |job| async move {
            for n in 0..100u8 {
                job.progress(f32::from(n) / 100., format!("{n}"));
                tokio::time::sleep(REFRESH / 10).await;
            }
            job.cancelled().await;
            Ok(())
        })
        .unwrap();
        // The start, then progress over ten refresh periods.
        runtime.block_on(async {
            tokio::time::sleep(REFRESH * 10 + REFRESH / 2).await
        });
        let refreshed = catalogs(&pushed);
        assert!((10..=12).contains(&refreshed), "{refreshed}");
        assert_eq!(jobs.running()[0].line, "99");
    }

    /// A job reads the repositories as they are when it asks.
    #[test]
    fn a_job_reads_the_repositories_as_they_are() {
        let (runtime, jobs, listed, _) = jobs();
        listed.lock().unwrap().push(repo("first"));
        let (seen, saw) = tokio::sync::mpsc::unbounded_channel();
        jobs.start("p", "watch", "Watching", move |job| async move {
            loop {
                let names: Vec<String> =
                    job.repos().into_iter().map(|repo| repo.name).collect();
                let _ = seen.send(names);
                tokio::select! {
                    () = job.cancelled() => return Ok(()),
                    () = tokio::time::sleep(Duration::from_millis(10)) => {}
                }
            }
        })
        .unwrap();
        let mut saw = saw;
        assert_eq!(runtime.block_on(saw.recv()).unwrap(), ["first"]);
        listed.lock().unwrap().push(repo("later"));
        let names = runtime.block_on(async {
            loop {
                let names = saw.recv().await.unwrap();
                if names.len() == 2 {
                    return names;
                }
            }
        });
        assert_eq!(names, ["first", "later"]);
    }

    /// A cancelled job that stops by itself ends at once; one that does
    /// not is dropped after [`GRACE`].
    #[test]
    fn cancel_stops_a_job() {
        let (runtime, jobs, _, _) = jobs();
        jobs.start("p", "polite", "Polite", |job| async move {
            job.cancelled().await;
            assert!(job.is_cancelled());
            Ok(())
        })
        .unwrap();
        jobs.start("p", "deaf", "Deaf", |_| std::future::pending())
            .unwrap();
        runtime.block_on(tokio::task::yield_now());
        assert!(jobs.cancel("p", "polite") && jobs.cancel("p", "deaf"));
        assert!(!jobs.cancel("p", "none"));
        runtime.block_on(async {
            tokio::time::sleep(Duration::from_millis(1)).await
        });
        let names: Vec<String> =
            jobs.running().into_iter().map(|job| job.name).collect();
        assert_eq!(names, ["deaf"]);
        runtime.block_on(async { tokio::time::sleep(GRACE).await });
        assert!(jobs.running().is_empty());
    }
}
