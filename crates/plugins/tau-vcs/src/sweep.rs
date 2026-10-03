//! Sweeping a project of what runs left behind: the workspaces and
//! bookmarks of runs that are gone, closed or thrown away, which
//! nothing cleans once the process that ran them is gone
//! (`docs/reference/vcs.md`, "Sweeping at start").
//!
//! [`plan`] is the rule, a pure function of the runs' standings and
//! what the project holds; [`Project::sweep`] carries a plan out.

use std::collections::BTreeSet;

use crate::{DEFAULT_WORKSPACE, Project, error::VcsError, run_workspace};

/// Where a run stands, as far as its workspaces and bookmark go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// It keeps them: running, or open, as a chat that can go on, or a
    /// sub-agent whose workspace was kept for recovery.
    Kept,
    /// Its work is on its parent now: they go, its commits stay.
    Landed,
    /// Its work is thrown away, as for a dropped chat or a sub-agent
    /// that failed or was cut off: they go, and so do its own commits.
    Discarded,
}

/// What a discarded run's commits are measured against: what its
/// parent has stays, and so does trunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keep {
    /// What this bookmark's commit has.
    Bookmark(String),
    /// What this workspace's working copy has: the main chat's, which
    /// commits on trunk, and whose own commits trunk may be past.
    Workspace(String),
}

/// One run that may own workspaces and a bookmark in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// The run's id: its bookmark is `tau/<run>`.
    pub run: String,
    pub standing: Standing,
    /// The workspaces it worked in, by name.
    pub workspaces: Vec<String>,
    /// For a discarded run: what stays when its commits go.
    pub keep: Keep,
}

/// The commits of a discarded run to abandon: what `heads` have that
/// `keep` lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abandon {
    pub run: String,
    /// Its bookmark, if the project has it.
    pub bookmark: Option<String>,
    /// Its workspaces the project still has: their working copies.
    pub workspaces: Vec<String>,
    pub keep: Keep,
}

/// What a sweep does, in this order: abandons, then forgets the
/// workspaces, then removes the bookmarks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    pub abandon: Vec<Abandon>,
    pub forget: Vec<String>,
    pub remove: Vec<String>,
}

impl Sweep {
    pub fn is_empty(&self) -> bool {
        self.abandon.is_empty()
            && self.forget.is_empty()
            && self.remove.is_empty()
    }
}

/// The prefix of a run's bookmark, `tau/<run>`.
pub const RUN_BOOKMARK_PREFIX: &str = "tau/";

/// What to sweep from a project that holds `workspaces` and
/// `bookmarks`, given the runs that may own them:
///
/// - a workspace goes unless a kept run worked in it; the default
///   workspace, the repository's own checkout, never goes;
/// - a run's bookmark (`tau/<run>`) goes unless that run is kept; other
///   bookmarks are not runs' and stay;
/// - a discarded run's own commits go first, from its bookmark and the
///   working copies of its workspaces that go.
///
/// A workspace or bookmark with no owner at all belongs to a run that
/// is gone, and goes.
pub fn plan(
    owners: &[Owner],
    workspaces: &[String],
    bookmarks: &[String],
) -> Sweep {
    let kept_workspaces: BTreeSet<&str> = owners
        .iter()
        .filter(|owner| owner.standing == Standing::Kept)
        .flat_map(|owner| owner.workspaces.iter().map(String::as_str))
        .collect();
    let kept_bookmarks: BTreeSet<String> = owners
        .iter()
        .filter(|owner| owner.standing == Standing::Kept)
        .map(|owner| bookmark_of(&owner.run))
        .collect();
    let forget: Vec<String> = workspaces
        .iter()
        .filter(|name| *name != DEFAULT_WORKSPACE)
        .filter(|name| !kept_workspaces.contains(name.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let remove: Vec<String> = bookmarks
        .iter()
        .filter(|name| name.starts_with(RUN_BOOKMARK_PREFIX))
        .filter(|name| !kept_bookmarks.contains(*name))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let abandon = owners
        .iter()
        .filter(|owner| owner.standing == Standing::Discarded)
        .filter_map(|owner| {
            let bookmark = bookmark_of(&owner.run);
            let bookmark = remove.contains(&bookmark).then_some(bookmark);
            let workspaces: Vec<String> = owner
                .workspaces
                .iter()
                .filter(|name| forget.contains(name))
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            (bookmark.is_some() || !workspaces.is_empty()).then(|| Abandon {
                run: owner.run.clone(),
                bookmark,
                workspaces,
                keep: owner.keep.clone(),
            })
        })
        .collect();
    Sweep {
        abandon,
        forget,
        remove,
    }
}

fn bookmark_of(run: &str) -> String {
    run_workspace::bookmark(&tau_agent::tool::RunId(run.into()))
}

impl Project {
    /// What [`plan`] says to sweep, given `owners`, from what the
    /// project holds now.
    pub fn plan_sweep(&self, owners: &[Owner]) -> Result<Sweep, VcsError> {
        let workspaces = self.workspaces()?;
        let bookmarks = self.bookmarks(RUN_BOOKMARK_PREFIX)?;
        Ok(plan(owners, &workspaces, &bookmarks))
    }

    /// Carries `sweep` out: abandons each discarded run's own commits,
    /// then forgets the workspaces (their directories go) and removes
    /// the bookmarks. Each step finds nothing to do when done before, so
    /// a sweep cut off is finished by the next. The operation log keeps
    /// what was abandoned.
    pub fn sweep(&self, sweep: &Sweep) -> Result<(), VcsError> {
        for abandon in &sweep.abandon {
            let keep = match &abandon.keep {
                Keep::Bookmark(name) => self.bookmark(name)?,
                Keep::Workspace(name) => self.workspace_head(name)?,
            };
            // Trunk always stays: a main chat that never ran stands on
            // the root commit.
            let trunk = self.trunk()?;
            let keeps: Vec<&str> = keep
                .iter()
                .map(String::as_str)
                .chain([trunk.as_str()])
                .collect();
            let mut heads = Vec::new();
            for name in &abandon.workspaces {
                heads.extend(self.workspace_head(name)?);
            }
            if let Some(name) = &abandon.bookmark {
                heads.extend(self.bookmark(name)?);
            }
            for head in heads {
                self.abandon_beyond(&keeps, &head)?;
            }
        }
        for name in &sweep.forget {
            self.forget_workspace(name)?;
        }
        for name in &sweep.remove {
            self.remove_bookmark(name)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hegel::{
        TestCase,
        generators::{self as gs, Generator as _},
    };

    use super::*;

    fn names(tc: &TestCase, pool: &[&str], max: usize) -> Vec<String> {
        let pool: Vec<String> =
            pool.iter().map(|name| (*name).to_owned()).collect();
        tc.draw(gs::vecs(gs::sampled_from(pool)).max_size(max))
    }

    fn owners(tc: &TestCase) -> Vec<Owner> {
        let runs = ["a", "b", "c", "d"];
        let count: usize = tc.draw(gs::integers().max_value(runs.len()));
        runs[..count]
            .iter()
            .map(|run| Owner {
                run: (*run).to_owned(),
                standing: tc.draw(
                    gs::sampled_from(vec![
                        Standing::Kept,
                        Standing::Landed,
                        Standing::Discarded,
                    ])
                    .print_as_debug(),
                ),
                workspaces: names(tc, &WORKSPACES, 2),
                keep: Keep::Workspace(DEFAULT_WORKSPACE.to_owned()),
            })
            .collect()
    }

    const WORKSPACES: [&str; 6] =
        ["default", "w1", "w2", "w3", "w1-sub-0", "w4"];
    const BOOKMARKS: [&str; 6] =
        ["tau/a", "tau/b", "tau/c", "tau/d", "tau/gone", "main"];

    /// What the project holds after `sweep`.
    fn after(
        sweep: &Sweep,
        workspaces: &[String],
        bookmarks: &[String],
    ) -> (Vec<String>, Vec<String>) {
        (
            workspaces
                .iter()
                .filter(|name| !sweep.forget.contains(name))
                .cloned()
                .collect(),
            bookmarks
                .iter()
                .filter(|name| !sweep.remove.contains(name))
                .cloned()
                .collect(),
        )
    }

    /// A sweep keeps everything a kept run has, takes everything else a
    /// run's (the default workspace and other bookmarks stay), abandons
    /// only discarded runs' commits, and only from what goes; and once
    /// done, sweeping again finds nothing.
    #[hegel::test(test_cases = 500)]
    fn a_sweep_keeps_what_kept_runs_have_and_nothing_else(tc: TestCase) {
        let owners = owners(&tc);
        let workspaces = names(&tc, &WORKSPACES, 6);
        let bookmarks = names(&tc, &BOOKMARKS, 6);
        let sweep = plan(&owners, &workspaces, &bookmarks);
        let (left_workspaces, left_bookmarks) =
            after(&sweep, &workspaces, &bookmarks);
        let kept: Vec<&Owner> = owners
            .iter()
            .filter(|owner| owner.standing == Standing::Kept)
            .collect();
        // What a kept run has stays.
        for owner in &kept {
            for name in &owner.workspaces {
                assert!(
                    !sweep.forget.contains(name),
                    "{name} of {}",
                    owner.run
                );
            }
            assert!(!sweep.remove.contains(&format!("tau/{}", owner.run)));
        }
        // What stays has a kept owner, or is not a run's.
        for name in &left_workspaces {
            assert!(
                name == DEFAULT_WORKSPACE
                    || kept.iter().any(|owner| owner.workspaces.contains(name)),
                "{name} stayed"
            );
        }
        for name in &left_bookmarks {
            assert!(
                !name.starts_with(RUN_BOOKMARK_PREFIX)
                    || kept
                        .iter()
                        .any(|owner| format!("tau/{}", owner.run) == *name),
                "{name} stayed"
            );
        }
        // Only what the project has goes.
        assert!(sweep.forget.iter().all(|name| workspaces.contains(name)));
        assert!(sweep.remove.iter().all(|name| bookmarks.contains(name)));
        // Commits go only for discarded runs, from what goes.
        for abandon in &sweep.abandon {
            let owner = owners
                .iter()
                .find(|owner| owner.run == abandon.run)
                .unwrap();
            assert_eq!(owner.standing, Standing::Discarded);
            assert!(
                abandon
                    .workspaces
                    .iter()
                    .all(|name| sweep.forget.contains(name))
            );
            assert!(
                abandon
                    .bookmark
                    .iter()
                    .all(|name| sweep.remove.contains(name))
            );
        }
        // A discarded run with something left to sweep has its commits go.
        for owner in owners
            .iter()
            .filter(|owner| owner.standing == Standing::Discarded)
        {
            let has = sweep.remove.contains(&format!("tau/{}", owner.run))
                || owner
                    .workspaces
                    .iter()
                    .any(|name| sweep.forget.contains(name));
            assert_eq!(
                has,
                sweep.abandon.iter().any(|abandon| abandon.run == owner.run)
            );
        }
        // Done once, a sweep has nothing left to do.
        assert!(plan(&owners, &left_workspaces, &left_bookmarks).is_empty());
    }

    #[test]
    fn the_default_workspace_and_other_bookmarks_stay() {
        let sweep = plan(
            &[],
            &["default".to_owned(), "stray".to_owned()],
            &["main".to_owned(), "tau/gone".to_owned()],
        );
        assert_eq!(
            sweep,
            Sweep {
                abandon: Vec::new(),
                forget: vec!["stray".to_owned()],
                remove: vec!["tau/gone".to_owned()],
            }
        );
    }
}
