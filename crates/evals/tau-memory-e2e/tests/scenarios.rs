//! The scenarios hold what the evaluation reads from them: each builds a
//! repository whose checks fail until the task is done and pass once it
//! is done by hand, the second task needs the fact, and the change makes
//! the old fact fail, touches exactly the files it lists, and leaves no
//! trace of the old fact for the agent to find.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use regex::Regex;
use tau_memory_e2e::scenario::{SCENARIOS, Scenario, Variant, run_script};

/// Every file in `dir` with its bytes, by path from `dir`.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(name, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// A repository with the first task done by hand, as a first run that
/// found the fact leaves it.
fn after_first(scenario: &Scenario, dir: &Path) {
    scenario.setup(dir).unwrap();
    run_script(dir, scenario.first.solution).unwrap();
}

#[test]
fn a_fresh_repository_fails_both_checks() {
    for scenario in SCENARIOS {
        let dir = tempfile::tempdir().unwrap();
        scenario.setup(dir.path()).unwrap();
        assert!(
            !scenario.first_done(dir.path()).unwrap(),
            "{}",
            scenario.name
        );
        assert!(
            !scenario.second_done(dir.path()).unwrap(),
            "{}",
            scenario.name
        );
    }
}

#[test]
fn the_first_task_done_by_hand_passes_its_check() {
    for scenario in SCENARIOS {
        let dir = tempfile::tempdir().unwrap();
        after_first(scenario, dir.path());
        assert!(
            scenario.first_done(dir.path()).unwrap(),
            "{}",
            scenario.name
        );
        // Doing the first task does not do the second.
        scenario.between(dir.path(), Variant::Stable).unwrap();
        assert!(
            !scenario.second_done(dir.path()).unwrap(),
            "{}",
            scenario.name
        );
    }
}

#[test]
fn the_second_task_done_by_hand_passes_in_either_variant() {
    for scenario in SCENARIOS {
        for variant in Variant::ALL {
            let dir = tempfile::tempdir().unwrap();
            after_first(scenario, dir.path());
            scenario.between(dir.path(), variant).unwrap();
            run_script(dir.path(), scenario.second_solution(variant)).unwrap();
            assert!(
                scenario.second_done(dir.path()).unwrap(),
                "{} {}",
                scenario.name,
                variant.name()
            );
        }
    }
}

/// Done with the old fact after the change, the second task fails: the
/// changed variant does test whether memory is re-checked.
#[test]
fn the_old_fact_fails_once_it_changed() {
    for scenario in SCENARIOS {
        let dir = tempfile::tempdir().unwrap();
        after_first(scenario, dir.path());
        scenario.between(dir.path(), Variant::Changed).unwrap();
        // The old solution may fail part way; what counts is the check.
        let _ = run_script(dir.path(), scenario.second.solution);
        assert!(
            !scenario.second_done(dir.path()).unwrap(),
            "{}",
            scenario.name
        );
    }
}

#[test]
fn the_change_touches_exactly_the_paths_it_lists() {
    for scenario in SCENARIOS {
        let dir = tempfile::tempdir().unwrap();
        after_first(scenario, dir.path());
        // Outputs are not part of the commit.
        scenario.between(dir.path(), Variant::Stable).unwrap();
        let before = snapshot(dir.path());
        scenario.between(dir.path(), Variant::Changed).unwrap();
        let after = snapshot(dir.path());
        let touched: BTreeSet<String> = before
            .keys()
            .chain(after.keys())
            .filter(|path| before.get(*path) != after.get(*path))
            .cloned()
            .collect();
        let listed: BTreeSet<String> = scenario
            .change
            .paths
            .iter()
            .map(|path| (*path).to_owned())
            .collect();
        assert_eq!(touched, listed, "{}", scenario.name);
    }
}

/// The stale pattern finds the old fact in the old solution, not in the
/// new one, and nothing in the changed repository still states it: an
/// agent that uses it got it from memory.
#[test]
fn the_stale_pattern_names_only_the_old_fact() {
    for scenario in SCENARIOS {
        let stale = Regex::new(scenario.change.stale).unwrap();
        assert!(
            stale.is_match(scenario.second.solution),
            "{}",
            scenario.name
        );
        assert!(
            !stale.is_match(scenario.change.solution),
            "{}",
            scenario.name
        );
        let dir = tempfile::tempdir().unwrap();
        after_first(scenario, dir.path());
        scenario.between(dir.path(), Variant::Changed).unwrap();
        for (path, bytes) in snapshot(dir.path()) {
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !stale.is_match(&text),
                "{}: {path} still states the old fact",
                scenario.name
            );
        }
    }
}

#[test]
fn scenario_names_are_distinct() {
    let names: BTreeSet<&str> = SCENARIOS.iter().map(|s| s.name).collect();
    assert_eq!(names.len(), SCENARIOS.len());
    assert!((4..=6).contains(&SCENARIOS.len()));
}
