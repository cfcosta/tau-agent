//! What landing each finished fork would do, worked out in the
//! background: when the fork finishes, and whenever its parent's head
//! may have moved (the parent ended a turn, a sibling landed, a fork
//! closed). The sidebar reads from it whether a fork is ready to land
//! or would conflict ([`tau_ui_remote::attention`]).

use std::{cell::RefCell, rc::Rc, time::Duration};

use tau_ui_remote::attention::Forecast;

use super::{landing::Reading, *};

/// How long a forecast waits for more changes before it starts: a
/// landing or a turn's end moves several runs at once.
pub(super) const SETTLE: Duration = Duration::from_millis(250);

impl Host {
    /// Waits `wait` for more changes before a forecast starts, in place
    /// of [`SETTLE`]: for a host whose runs end faster than a person's,
    /// as in tests.
    pub fn with_forecast_wait(mut self, wait: Duration) -> Self {
        self.forecast_wait = wait;
        self
    }
}

/// What a repository's runs look like, as far as forecasts go: each
/// run, whether it is going or closed, and its turn. A change in it may
/// have moved a parent's head or finished a fork.
type Shape = Vec<(RunId, bool, bool, u32)>;

impl Host {
    /// What landing `child` on its parent would do now: the preview's
    /// own reading ([`Self::land_dry`]), but one that writes nothing,
    /// neither catching the parent up with trunk nor making workspaces,
    /// so it can run while the person works. `None` when there is
    /// nothing to land.
    pub fn forecast_landing(
        &self,
        child: &RunId,
    ) -> anyhow::Result<Option<Forecast>> {
        let project = self.slot_of_run(child)?.project()?;
        if project.bookmark(&bookmark(child))?.is_none() {
            return Ok(None);
        }
        let landing = self.land_dry(child, Reading::Forecast)?;
        Ok(Some(Forecast::from(&landing)))
    }

    /// Whether `generation` is still the latest forecast asked for in
    /// `repo`.
    fn forecast_current(&self, repo: &str, generation: u64) -> bool {
        self.forecasts.lock().expect("not poisoned").get(repo)
            == Some(&generation)
    }
}

/// Keeps the workspace's landing forecasts current: each time a
/// repository's runs change shape, its finished forks get new ones.
pub(super) fn follow(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let shapes: Rc<RefCell<HashMap<String, Shape>>> = Rc::default();
    let host = host.clone();
    cx.observe(workspace, move |workspace, cx| {
        let changed: Vec<(String, Vec<RunId>)> = {
            let ws = workspace.read(cx);
            let mut shapes = shapes.borrow_mut();
            ws.catalog()
                .repos
                .iter()
                .filter_map(|repo| {
                    let shape: Shape = ws
                        .runs()
                        .iter()
                        .filter(|run| ws.repo_of(run) == repo.name)
                        .map(|run| {
                            (
                                run.id.clone(),
                                run.status.is_live(),
                                ws.is_closed(&run.id),
                                run.turn,
                            )
                        })
                        .collect();
                    if shapes.get(&repo.name) == Some(&shape) {
                        return None;
                    }
                    shapes.insert(repo.name.clone(), shape);
                    Some((repo.name.clone(), ws.finished_forks(&repo.name)))
                })
                .collect()
        };
        for (repo, forks) in changed {
            forecast_in_background(&host, repo, forks, workspace.clone(), cx);
        }
    })
    .detach();
}

/// Works out what landing each of `forks` in `repo` would do, off the
/// interface's thread, and shows it. A later call for the same
/// repository overtakes this one: it then shows nothing. A fork or a
/// parent still going keeps the forecast it had.
fn forecast_in_background(
    host: &Arc<Host>,
    repo: String,
    forks: Vec<RunId>,
    workspace: Entity<Workspace>,
    cx: &mut App,
) {
    let generation = {
        let mut forecasts = host.forecasts.lock().expect("not poisoned");
        let generation = forecasts.entry(repo.clone()).or_default();
        *generation += 1;
        *generation
    };
    if forks.is_empty() {
        return;
    }
    let counted = host.job();
    let job = {
        let (worker, repo) = (host.clone(), repo.clone());
        host.runtime.spawn_blocking(move || {
            let host = worker;
            std::thread::sleep(host.forecast_wait);
            let mut found = Vec::new();
            for fork in forks {
                if !host.forecast_current(&repo, generation) {
                    return Vec::new();
                }
                let Ok(parent) = host.parent_of(&fork) else {
                    continue;
                };
                host.settle(&fork);
                host.settle(&parent);
                if host.is_running(&fork) || host.is_running(&parent) {
                    continue;
                }
                match host.forecast_landing(&fork) {
                    Ok(forecast) => found.push((fork, forecast)),
                    Err(error) => eprintln!(
                        "tau-ui: cannot forecast landing {}: {error:#}",
                        fork.0
                    ),
                }
            }
            found
        })
    };
    let host = host.clone();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let _counted = counted;
        let Ok(found) = job.await else {
            return;
        };
        if !host.forecast_current(&repo, generation) {
            return;
        }
        let _ = workspace.update(cx, |ws, cx| {
            for (run, forecast) in found {
                ws.apply(HostUpdate::Forecast { run, forecast }, cx);
            }
        });
    })
    .detach();
}
