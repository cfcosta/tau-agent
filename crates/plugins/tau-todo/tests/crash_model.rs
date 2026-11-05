//! A crash anywhere (ADR 0048, "2. The store"): the plugin's tools on its
//! store in a file, every write through a fuse that lets a drawn number
//! through and fails the rest, as if tau died there. The store, opened
//! again from the file, is the reference model as of the calls that
//! answered: never a mix, no half-made issue, no link to one that is
//! not there; and opening it again changes nothing.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod support;

use std::sync::Arc;

use hegel::{TestCase, generators as gs};
use support::{
    Plugin,
    kept,
    store::{Op, Step, Store, draw_step},
};
use tau_luau_plugins_host::store::{Fuse, Stores};
use tau_testing::block_on_io;

/// What reopening the store reads, after `steps` run with `fuse`: the
/// model as of the calls that answered, which it checks along the way.
fn close_after(plugin: &Plugin, steps: &[Step], writes: i64) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let fuse = Fuse::after(writes);
    let stores = Stores::at(dir.path().to_owned()).with_fuse(fuse.clone());
    let store = block_on_io(stores.of(tau_todo::NAME, "home")).unwrap();
    let mut model = Store::default();
    for step in steps {
        let (hook, input) = step.call();
        let answer =
            block_on_io(plugin.call(&store, &step.run, step.now, hook, input));
        let mut tried = model.clone();
        let allowed = tried.apply(step, &answer).is_ok();
        match (&answer, fuse.blown()) {
            // A call that went in is the model's.
            (Ok(_), _) => {
                assert!(allowed, "{step:?} answered, the model refuses it");
                model = tried;
            }
            // Refused before tau closed: the model refuses it too.
            (Err(_), false) => {
                assert!(!allowed, "{step:?} refused: {answer:?}")
            }
            // Cut off as tau closed: none of it is kept.
            (Err(_), true) => {}
        }
    }
    drop(store);
    drop(stores);
    // tau starts again on the file, twice.
    for _ in 0..2 {
        let stores = Stores::at(dir.path().to_owned());
        let store = block_on_io(stores.of(tau_todo::NAME, "home")).unwrap();
        let (issues, retired) = block_on_io(kept(&store));
        assert_eq!(issues, model.issues, "what tau found as it started");
        let model_retired: Vec<String> =
            model.retired.iter().cloned().collect();
        assert_eq!(retired, model_retired);
        // No link to an issue that is not there.
        for issue in issues.values() {
            for link in issue["links"].as_array().into_iter().flatten() {
                let on = link["id"].as_str().unwrap();
                assert!(
                    issues.contains_key(on),
                    "{} links to {on}, gone",
                    issue["id"]
                );
            }
        }
    }
    fuse.blown()
}

fn steps(tc: &TestCase, n: usize) -> Vec<Step> {
    let mut model = Store::default();
    let mut now: i64 = 1_793_700_000;
    let plugin = Plugin::new();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        block_on_io(
            Stores::at(dir.path().to_owned()).of(tau_todo::NAME, "home"),
        )
        .unwrap(),
    );
    let mut out = Vec::new();
    // Drawn against the model as it goes, so ids name issues there are.
    for _ in 0..n {
        let step = draw_step(tc, &model, &mut now);
        let (hook, input) = step.call();
        let answer =
            block_on_io(plugin.call(&store, &step.run, step.now, hook, input));
        let _ = model.apply(&step, &answer);
        out.push(step);
    }
    out
}

#[hegel::test(test_cases = 40)]
fn a_crash_anywhere_loses_nothing_it_answered(tc: TestCase) {
    let steps = steps(&tc, 15);
    let writes: i64 = tc.draw(gs::integers().min_value(0).max_value(60));
    close_after(&Plugin::new(), &steps, writes);
}

#[hegel::test(test_cases = 400)]
#[ignore = "nightly"]
fn a_crash_anywhere_loses_nothing_it_answered_nightly(tc: TestCase) {
    let steps = steps(&tc, 40);
    let writes: i64 = tc.draw(gs::integers().min_value(0).max_value(200));
    close_after(&Plugin::new(), &steps, writes);
}

/// Every write of every tool in turn is where tau closes, so no place
/// to close depends on a draw: one issue made, linked, claimed, checked,
/// closed, deleted.
#[test]
fn every_write_is_a_place_to_close() {
    let plugin = Plugin::new();
    let at = |now: i64, run: &str, op: Op| Step {
        run: run.into(),
        now,
        op,
    };
    // Ids come from the plugin: learn them from a run with no fuse.
    let dir = tempfile::tempdir().unwrap();
    let store = block_on_io(
        Stores::at(dir.path().to_owned()).of(tau_todo::NAME, "home"),
    )
    .unwrap();
    let create = |now: i64, title: &str, kind: &str| {
        let step = at(
            now,
            "r1",
            Op::Create {
                title: title.into(),
                kind: kind.into(),
                priority: 1,
                criteria: 1,
                parent: None,
            },
        );
        let (hook, input) = step.call();
        let answer =
            block_on_io(plugin.call(&store, "r1", now, hook, input)).unwrap();
        (step, answer["issue"]["id"].as_str().unwrap().to_owned())
    };
    let (first, a) = create(10, "the epic", "epic");
    let (second, b) = create(20, "a task", "task");
    let steps = vec![
        first,
        second,
        at(
            30,
            "r1",
            Op::Link {
                id: b.clone(),
                kind: "parent".into(),
                on: a.clone(),
                remove: false,
            },
        ),
        at(40, "r2", Op::Claim { id: b.clone() }),
        at(
            50,
            "r2",
            Op::Check {
                id: b.clone(),
                at: 1,
            },
        ),
        at(
            60,
            "r2",
            Op::Comment {
                id: b.clone(),
                text: "half way".into(),
            },
        ),
        at(70, "r2", Op::RunEnd),
        at(
            80,
            "r1",
            Op::Close {
                id: b.clone(),
                reason: "done".into(),
            },
        ),
        at(
            90,
            "r1",
            Op::Close {
                id: a.clone(),
                reason: "done".into(),
            },
        ),
        at(100, "r1", Op::Delete { id: b }),
    ];
    let mut writes = 0;
    while close_after(&plugin, &steps, writes) {
        writes += 1;
        assert!(writes < 1000, "the steps never ran whole");
    }
    assert!(writes > steps.len() as i64, "every step wrote");
}
