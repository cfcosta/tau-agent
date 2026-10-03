//! When tau notifies (`tau_ui::notify`): once each time a chat's state
//! turns into one that calls for a notice (it asks, it is ready to
//! land, it failed), never twice for the same stretch, never while the
//! window is focused, and never for what was so when tau first saw the
//! chat. The notifier runs against a plain record of what each chat
//! was, with a sink that keeps what it is shown.

use std::collections::HashMap;

use hegel::{
    TestCase,
    generators::{self as gs, Generator as _},
};
use tau_agent::tool::RunId;
use tau_ui::notify::{Kind, Notice, Notifier, Sighting, Sink};
use tau_ui_remote::attention::Attention;

/// Keeps what it is shown.
#[derive(Default)]
struct Shown(Vec<Notice>);

impl Sink for Shown {
    fn show(&mut self, notice: Notice) {
        self.0.push(notice);
    }
}

const RUNS: [&str; 3] = ["a", "b", "c"];

fn attention(tc: &TestCase) -> Attention {
    tc.draw(
        gs::sampled_from(vec![
            Attention::Working { turn: 3 },
            Attention::Asks {
                question: "Keep the workspaces?".into(),
            },
            Attention::ReadyToLand { changes: 2 },
            Attention::WouldConflict {
                files: vec!["a.rs".into()],
            },
            Attention::Interrupted,
            Attention::Failed,
            Attention::Landed,
            Attention::Idle,
        ])
        .print_as_debug(),
    )
}

/// The notice kind a state calls for, written out apart from the
/// notifier's own.
fn calls_for(attention: &Attention) -> Option<Kind> {
    match attention {
        Attention::Asks { .. } => Some(Kind::Asks),
        Attention::ReadyToLand { .. } => Some(Kind::ReadyToLand),
        Attention::Failed => Some(Kind::Failed),
        _ => None,
    }
}

struct Notifying {
    notifier: Notifier,
    shown: Shown,
    /// What each chat in view called for when last seen.
    model: HashMap<String, Option<Kind>>,
    /// Every chat's sightings in order, each with whether it was
    /// notified: (run, kind, notified).
    log: Vec<(String, Option<Kind>, bool)>,
}

#[hegel::state_machine]
impl Notifying {
    /// The workspace changed: some chats are in view, each in a state,
    /// with the window focused or not.
    #[rule]
    fn see(&mut self, tc: TestCase) {
        let focused = tc.draw(gs::booleans());
        let in_view: Vec<&str> = RUNS
            .into_iter()
            .filter(|_| tc.draw(gs::booleans()))
            .collect();
        let sightings: Vec<Sighting> = in_view
            .iter()
            .map(|run| Sighting {
                run: RunId((*run).into()),
                title: format!("Chat {run}"),
                repo: "tau-agent".into(),
                target: "main".into(),
                attention: attention(&tc),
            })
            .collect();
        let before = self.shown.0.len();
        self.notifier.observe(&sightings, focused, &mut self.shown);
        let shown = self.shown.0[before..].to_vec();

        let mut expected = Vec::new();
        let mut model = HashMap::new();
        for sighting in &sightings {
            let run = sighting.run.0.to_string();
            let kind = calls_for(&sighting.attention);
            let turned =
                self.model.get(&run).is_some_and(|before| *before != kind);
            let notified = turned && !focused && kind.is_some();
            if notified {
                expected.push((sighting.run.clone(), kind.unwrap()));
            }
            self.log.push((run.clone(), kind, notified));
            model.insert(run, kind);
        }
        // A chat out of view is forgotten: seen again, it is new.
        self.model = model;
        let got: Vec<(RunId, Kind)> = shown
            .iter()
            .map(|notice| (notice.run.clone(), notice.kind))
            .collect();
        assert_eq!(got, expected);
        for notice in &shown {
            let sighting = sightings
                .iter()
                .find(|sighting| sighting.run == notice.run)
                .unwrap();
            assert!(notice.title.starts_with(&sighting.title), "{notice:?}");
        }
    }

    /// A turn left conflicts on main: said once, unless focused.
    #[rule]
    fn conflicts(&mut self, tc: TestCase) {
        let focused = tc.draw(gs::booleans());
        let files = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
        let before = self.shown.0.len();
        self.notifier.conflicts_left(
            &RunId("main".into()),
            "tau-agent",
            files,
            focused,
            &mut self.shown,
        );
        let shown = &self.shown.0[before..];
        if focused {
            assert!(shown.is_empty());
        } else {
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].kind, Kind::ConflictsOnMain);
            assert!(shown[0].body.contains(&files.to_string()));
        }
    }

    /// No chat is notified twice for one stretch of the same state: two
    /// notices of a chat have a sighting of another state between them.
    #[invariant(always_run)]
    fn one_notice_per_stretch(&self, _tc: TestCase) {
        let mut last: HashMap<&str, (Option<Kind>, bool)> = HashMap::new();
        for (run, kind, notified) in &self.log {
            let entry = last.entry(run.as_str()).or_insert((*kind, false));
            if entry.0 != *kind {
                *entry = (*kind, false);
            }
            assert!(!(*notified && entry.1), "{run} notified twice");
            entry.1 |= *notified;
        }
    }
}

/// The notifier says each change once, as a plain record of each
/// chat's last state says it should.
#[hegel::test(test_cases = 100)]
fn notices_fire_once_per_change(tc: TestCase) {
    let notifying = Notifying {
        notifier: Notifier::default(),
        shown: Shown::default(),
        model: HashMap::new(),
        log: Vec::new(),
    };
    hegel::stateful::machine(notifying).steps(30).run(tc);
}

/// A notice's words, as the design writes them.
#[test]
fn notices_read_as_the_design_words_them() {
    let sighting = |attention| Sighting {
        run: RunId("r".into()),
        title: "Load AGENTS.md".into(),
        repo: "tau-agent".into(),
        target: "main".into(),
        attention,
    };
    let mut notifier = Notifier::default();
    let mut shown = Shown::default();
    let states = [
        Attention::Working { turn: 1 },
        Attention::Asks {
            question: "Delete the 3 workspaces with no run, or keep them?"
                .into(),
        },
        Attention::Working { turn: 2 },
        Attention::ReadyToLand { changes: 2 },
        Attention::Failed,
    ];
    for state in states {
        notifier.observe(&[sighting(state)], false, &mut shown);
    }
    notifier.conflicts_left(
        &RunId("main".into()),
        "tau-agent",
        2,
        false,
        &mut shown,
    );
    let words: Vec<(String, String)> = shown
        .0
        .into_iter()
        .map(|notice| (notice.title, notice.body))
        .collect();
    let pair = |title: &str, body: &str| (title.to_owned(), body.to_owned());
    assert_eq!(
        words,
        [
            pair(
                "Load AGENTS.md asks you",
                "Delete the 3 workspaces with no run, or keep them?"
            ),
            pair(
                "Load AGENTS.md is ready to land",
                "2 changes on main · tau-agent"
            ),
            pair(
                "Load AGENTS.md failed",
                "The run stopped with an error · tau-agent"
            ),
            pair(
                "Conflicts are still on main",
                "tau's turn left conflicts in 2 files · tau-agent"
            ),
        ]
    );
}
