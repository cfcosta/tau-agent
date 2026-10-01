//! The store against a model: a map of notes, with every replaced
//! version kept. Reopening the directory reads back what the model holds.

mod common;

use std::collections::BTreeMap;

use common::{pool_id, pool_note};
use hegel::{TestCase, generators as gs};
use tau_memory::{
    note::{Link, LinkType, Note},
    store::{Notes, StoreError},
};

/// What the store should hold: each note, and every version a write
/// replaced, oldest first.
#[derive(Default)]
struct Model {
    notes: BTreeMap<String, Note>,
    history: BTreeMap<String, Vec<Note>>,
}

impl Model {
    /// Writes `note` over the one with its id, keeping that in history.
    fn replace(&mut self, note: Note) {
        let old = self.notes.insert(note.id.clone(), note);
        self.history
            .entry(old.as_ref().expect("replaces a note").id.clone())
            .or_default()
            .extend(old);
    }

    fn history(&self, id: &str) -> Vec<Note> {
        self.history.get(id).cloned().unwrap_or_default()
    }

    /// Backlinks computed the plain way: every link, from every note.
    fn backlinks(&self, id: &str) -> Vec<(String, Link)> {
        let mut found = Vec::new();
        for note in self.notes.values() {
            for link in note.all_links() {
                if link.to == id && link.kind != LinkType::About {
                    found.push((note.id.clone(), link));
                }
            }
        }
        found
    }
}

/// A scope's directory, the store over it, and its model.
struct Scope {
    dir: tempfile::TempDir,
    notes: Notes,
    model: Model,
    now: u64,
}

#[hegel::state_machine]
impl Scope {
    #[rule]
    fn create(&mut self, tc: TestCase) {
        self.now += 1;
        let note = tc.draw(pool_note());
        let taken = self.model.notes.contains_key(&note.id);
        match self.notes.create(note.clone()) {
            Err(StoreError::Exists(_)) => assert!(taken),
            Ok(()) => {
                assert!(!taken);
                self.model.notes.insert(note.id.clone(), note);
            }
            Err(other) => panic!("{other}"),
        }
    }

    #[rule]
    fn update(&mut self, tc: TestCase) {
        self.now += 1;
        let mut note = tc.draw(pool_note());
        let known = self.model.notes.contains_key(&note.id);
        note.updated = self.now;
        match self.notes.update(note.clone()) {
            Err(StoreError::Missing(_)) => assert!(!known),
            Ok(()) => {
                assert!(known);
                self.model.replace(note);
            }
            Err(other) => panic!("{other}"),
        }
    }

    #[rule]
    fn supersede(&mut self, tc: TestCase) {
        self.now += 1;
        let now = self.now;
        let old = tc.draw(pool_id());
        let mut new = tc.draw(pool_note());
        let known = self.model.notes.contains_key(&old);
        let free = new.id != old && !self.model.notes.contains_key(&new.id);
        new.links.retain(|link| link.to != old);
        match self.notes.supersede(&old, new.clone(), now) {
            Ok(()) => {
                assert!(known && free);
                new.links.push(Link {
                    to: old.clone(),
                    kind: LinkType::Supersedes,
                    why: None,
                });
                self.model.notes.insert(new.id.clone(), new);
                let mut previous = self.model.notes[&old].clone();
                previous.valid_to = Some(now);
                previous.updated = now;
                self.model.replace(previous);
            }
            Err(StoreError::Missing(_)) => assert!(!known),
            Err(StoreError::Exists(_)) => assert!(known && !free),
            Err(other) => panic!("{other}"),
        }
    }

    /// Stale marks follow the files a note came from or is about, and
    /// only reach notes written before the change began.
    #[rule]
    fn mark_stale(&mut self, tc: TestCase) {
        self.now += 1;
        let now = self.now;
        let paths: Vec<String> = tc
            .draw(gs::subsequences(vec!["src/a.rs", "src/b.rs"]))
            .into_iter()
            .map(str::to_owned)
            .collect();
        // A change that began at `written_by`: notes written since are
        // newer than it.
        let written_by =
            tc.draw(gs::integers::<u64>().min_value(1_000).max_value(now));
        let expected: Vec<String> = self
            .model
            .notes
            .values()
            .filter(|note| {
                note.valid_to.is_none()
                    && note.stale.is_none()
                    && note.updated <= written_by
                    && (note.source.files.iter().any(|f| paths.contains(f))
                        || note.links.iter().any(|link| {
                            link.kind == LinkType::About
                                && paths.contains(&link.to)
                        }))
            })
            .map(|note| note.id.clone())
            .collect();
        let marked = self
            .notes
            .mark_stale(&paths, "src changed", written_by, now)
            .unwrap();
        assert_eq!(marked, expected);
        tc.event(format!("marked {}", marked.len()));
        for id in marked {
            let mut note = self.model.notes[&id].clone();
            note.stale = Some("src changed".into());
            note.updated = now;
            self.model.replace(note);
        }
    }

    /// Any version the model kept comes back, and the one it replaces
    /// goes into history in turn.
    #[rule]
    fn revert(&mut self, tc: TestCase) {
        self.now += 1;
        let id = tc.draw(pool_id());
        let versions = self.model.history(&id);
        if versions.is_empty() {
            assert!(self.notes.revert(&id, versions.len()).is_err());
        } else {
            tc.event("reverted");
            let at =
                tc.draw(gs::integers::<usize>().max_value(versions.len() - 1));
            self.notes.revert(&id, at).unwrap();
            self.model.replace(versions[at].clone());
        }
        // Past the last version is never one.
        assert!(self.notes.revert(&id, versions.len() + 1).is_err());
    }

    #[rule]
    fn reopen(&mut self, _tc: TestCase) {
        self.notes = Notes::open(self.dir.path()).unwrap();
        assert!(
            self.notes.unreadable().is_empty(),
            "{:?}",
            self.notes.unreadable()
        );
    }

    /// The store holds the model's notes, and every version it kept.
    #[invariant(always_run)]
    fn the_store_holds_the_model(&self, _tc: TestCase) {
        let held: BTreeMap<String, Note> = self
            .notes
            .iter()
            .map(|note| (note.id.clone(), note.clone()))
            .collect();
        assert_eq!(held, self.model.notes);
        for id in IDS {
            assert_eq!(
                self.notes.history(id),
                self.model.history(id),
                "history of {id}"
            );
        }
    }

    #[invariant(always_run)]
    fn links_are_the_models(&self, _tc: TestCase) {
        let model = &self.model;
        for id in IDS {
            assert_eq!(
                self.notes.backlinks(id),
                model.backlinks(id),
                "backlinks of {id}"
            );
            // One hop reaches exactly the notes linked either way.
            let mut hop = self.notes.neighbours(id);
            hop.sort();
            let mut expected: Vec<String> = model
                .backlinks(id)
                .into_iter()
                .map(|(from, _)| from)
                .chain(model.notes.get(id).into_iter().flat_map(|note| {
                    note.all_links().into_iter().map(|link| link.to)
                }))
                .filter(|other| other != id && model.notes.contains_key(other))
                .collect();
            expected.sort();
            expected.dedup();
            assert_eq!(hop, expected, "neighbours of {id}");
        }
    }
}

/// The ids [`pool_id`] draws.
const IDS: [&str; 5] = ["a", "b", "c", "d", "e"];

#[hegel::test(test_cases = 60)]
fn the_store_matches_its_model(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let scope = Scope {
        notes: Notes::open(dir.path()).unwrap(),
        dir,
        model: Model::default(),
        now: 1_000,
    };
    hegel::stateful::machine(scope).steps(25).run(tc);
}

/// A file the store cannot read is reported with its path, and the rest
/// still load.
#[test]
fn a_broken_file_is_reported_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("fact")).unwrap();
    std::fs::write(dir.path().join("fact/broken.md"), "no front matter")
        .unwrap();
    let notes = Notes::open(dir.path()).unwrap();
    assert!(notes.is_empty());
    assert_eq!(notes.unreadable().len(), 1);
    assert!(notes.unreadable()[0].path.ends_with("fact/broken.md"));
}
