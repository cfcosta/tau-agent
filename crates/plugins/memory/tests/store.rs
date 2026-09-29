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

#[derive(Debug, Clone, Copy)]
enum Op {
    Create,
    Update,
    Supersede,
    MarkStale,
    Revert,
    Reopen,
}

/// What the store should hold: each note, and how many versions of it
/// the history keeps.
#[derive(Default)]
struct Model {
    notes: BTreeMap<String, Note>,
    versions: BTreeMap<String, usize>,
}

impl Model {
    fn replace(&mut self, note: Note) {
        *self.versions.entry(note.id.clone()).or_default() += 1;
        self.notes.insert(note.id.clone(), note);
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

#[hegel::test(test_cases = 60)]
fn the_store_matches_its_model(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let mut notes = Notes::open(dir.path()).unwrap();
    let mut model = Model::default();
    let mut now = 1_000;
    let steps = tc.draw(gs::integers::<usize>().min_value(1).max_value(25));
    for _ in 0..steps {
        now += 1;
        let op = tc.draw(gs::sampled_from(vec![
            Op::Create,
            Op::Update,
            Op::Supersede,
            Op::MarkStale,
            Op::Revert,
            Op::Reopen,
        ]));
        match op {
            Op::Create => {
                let note = tc.draw(pool_note());
                let taken = model.notes.contains_key(&note.id);
                match notes.create(note.clone()) {
                    Err(StoreError::Exists(_)) => assert!(taken),
                    Ok(()) => {
                        assert!(!taken);
                        model.notes.insert(note.id.clone(), note);
                    }
                    Err(other) => panic!("{other}"),
                }
            }
            Op::Update => {
                let mut note = tc.draw(pool_note());
                let known = model.notes.contains_key(&note.id);
                note.updated = now;
                match notes.update(note.clone()) {
                    Err(StoreError::Missing(_)) => assert!(!known),
                    Ok(()) => {
                        assert!(known);
                        model.replace(note);
                    }
                    Err(other) => panic!("{other}"),
                }
            }
            Op::Supersede => {
                let old = tc.draw(pool_id());
                let mut new = tc.draw(pool_note());
                let known = model.notes.contains_key(&old);
                let free = new.id != old && !model.notes.contains_key(&new.id);
                new.links.retain(|link| link.to != old);
                match notes.supersede(&old, new.clone(), now) {
                    Ok(()) => {
                        assert!(known && free);
                        new.links.push(Link {
                            to: old.clone(),
                            kind: LinkType::Supersedes,
                            why: None,
                        });
                        model.notes.insert(new.id.clone(), new);
                        let mut previous = model.notes[&old].clone();
                        previous.valid_to = Some(now);
                        previous.updated = now;
                        model.replace(previous);
                    }
                    Err(StoreError::Missing(_)) => assert!(!known),
                    Err(StoreError::Exists(_)) => assert!(known && !free),
                    Err(other) => panic!("{other}"),
                }
            }
            Op::MarkStale => {
                // Stale marks follow the files a note came from.
                let file =
                    tc.draw(gs::sampled_from(vec!["src/a.rs", "src/b.rs"]));
                // A change that began at `written_by`: notes written
                // since are newer than it.
                let written_by = tc.draw(
                    gs::integers::<u64>().min_value(1_000).max_value(now),
                );
                let expected: Vec<String> = model
                    .notes
                    .values()
                    .filter(|note| {
                        note.valid_to.is_none()
                            && note.stale.is_none()
                            && note.updated <= written_by
                            && (note.source.files.iter().any(|f| f == file)
                                || note.links.iter().any(|link| {
                                    link.kind == LinkType::About
                                        && link.to == file
                                }))
                    })
                    .map(|note| note.id.clone())
                    .collect();
                let marked = notes
                    .mark_stale(
                        &[file.to_owned()],
                        "src changed",
                        written_by,
                        now,
                    )
                    .unwrap();
                assert_eq!(marked, expected);
                for id in marked {
                    let mut note = model.notes[&id].clone();
                    note.stale = Some("src changed".into());
                    note.updated = now;
                    model.replace(note);
                }
            }
            Op::Revert => {
                let id = tc.draw(pool_id());
                let versions = notes.history(&id);
                assert_eq!(
                    versions.len(),
                    model.versions.get(&id).copied().unwrap_or(0)
                );
                if versions.is_empty() {
                    assert!(notes.revert(&id, 0).is_err());
                } else {
                    let at = tc.draw(
                        gs::integers::<usize>().max_value(versions.len() - 1),
                    );
                    notes.revert(&id, at).unwrap();
                    model.replace(versions[at].clone());
                }
            }
            Op::Reopen => {
                notes = Notes::open(dir.path()).unwrap();
                assert!(
                    notes.unreadable().is_empty(),
                    "{:?}",
                    notes.unreadable()
                );
            }
        }
        let held: BTreeMap<String, Note> = notes
            .iter()
            .map(|note| (note.id.clone(), note.clone()))
            .collect();
        assert_eq!(held, model.notes);
        for id in ["a", "b", "c", "d", "e"] {
            assert_eq!(
                notes.backlinks(id),
                model.backlinks(id),
                "backlinks of {id}"
            );
            // One hop reaches exactly the notes linked either way.
            let mut hop = notes.neighbours(id);
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
