//! tau-memory's UI: the notes page's view of a scope's notes, how
//! long ago a note was edited, and stale marks from turns' commits
//! bounded by when the notes were written.

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use tau_memory::ui::ago;
use tau_memory_host::{
    Memories,
    Memory,
    MemoryPlugin,
    Scopes,
    Search,
    index::Bm25,
    memory::{Draft, WriteError},
    note::{By, Link, LinkType, NoteType, Source},
    notebook as catalog,
    now,
    stale_on_turn,
};
use tau_ui_plugin::{TurnCommit, testing::FakeRun};

const TITLES: [&str; 5] = [
    "lanes drain in order",
    "the cache is keyed by model",
    "retries back off",
    "forks start from a turn",
    "rules are checked with jev",
];
const PATHS: [&str; 3] = ["src/a.rs", "src/b.rs", "docs/c.md"];

fn source(files: Vec<String>) -> Source {
    Source {
        by: By::Agent,
        run: None,
        turn: None,
        commit: None,
        files,
        imported: None,
    }
}

fn draft() -> impl PrintableGenerator<Draft> {
    // Draft is tau's own type, so its drawn values print through Debug.
    draft_unprinted().print_as_debug()
}

#[hegel::composite]
fn draft_unprinted(tc: &TestCase) -> Draft {
    let title = tc.draw(gs::sampled_from(TITLES.to_vec())).to_owned();
    let about: Vec<String> = tc
        .draw(gs::vecs(gs::sampled_from(PATHS.to_vec())).max_size(2))
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut links: Vec<Link> = about
        .iter()
        .map(|path| Link {
            to: path.clone(),
            kind: LinkType::About,
            why: None,
        })
        .collect();
    links.dedup_by(|a, b| a.to == b.to);
    if tc.draw(gs::booleans()) {
        links.push(Link {
            to: "retries-back-off".into(),
            kind: LinkType::Relates,
            why: tc.draw(gs::optional(gs::just("same queue".to_owned()))),
        });
    }
    let files = tc
        .draw(gs::vecs(gs::sampled_from(PATHS.to_vec())).max_size(2))
        .into_iter()
        .map(str::to_owned)
        .collect();
    Draft {
        kind: NoteType::Fact,
        title,
        description: "One line of what it says.".into(),
        body: tc
            .draw(gs::sampled_from(vec![
                "One paragraph.",
                "First.\n\nSecond,\nwrapped.",
            ]))
            .to_owned(),
        tags: Vec::new(),
        links,
        id: None,
        supersedes: None,
        source: source(files),
    }
}

/// The screen lists the current notes, newest first, each with its
/// paths (what it is about and came from), its links to other notes,
/// its paragraphs, and a stale mark first when it has one.
#[hegel::test(test_cases = 100)]
fn the_screen_shows_current_notes_as_they_are(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let mut memory = Memory::open(dir.path(), Box::new(Bm25::new())).unwrap();
    let mut clock = 1_000;
    for _ in 0..tc.draw(gs::integers::<usize>().min_value(1).max_value(8)) {
        clock += 1;
        let mut draft = tc.draw(draft());
        let existing: Vec<String> =
            memory.notes().iter().map(|note| note.id.clone()).collect();
        if !existing.is_empty() && tc.draw(gs::booleans()) {
            draft.supersedes = Some(tc.draw(gs::sampled_from(existing)));
        }
        // A title taken without a target is refused; that is fine here.
        match memory.write(draft, clock) {
            Ok(_) => tc.event("written"),
            Err(WriteError::Refused(_)) => tc.event("refused"),
            Err(other) => panic!("{other}"),
        }
        if tc.draw(gs::booleans()) {
            clock += 1;
            let path = tc.draw(gs::sampled_from(PATHS.to_vec())).to_owned();
            memory
                .mark_stale(&[path], "it changed", clock, clock)
                .unwrap();
        }
    }
    let shown = catalog(dir.path(), memory.notes(), clock);

    let mut current: Vec<_> = memory
        .notes()
        .iter()
        .filter(|note| !note.is_superseded())
        .collect();
    current.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.id.cmp(&b.id)));
    let ids: Vec<&str> =
        shown.notes.iter().map(|note| note.id.as_str()).collect();
    let want: Vec<&str> = current.iter().map(|note| note.id.as_str()).collect();
    assert_eq!(ids, want);

    for (entry, note) in shown.notes.iter().zip(&current) {
        let paths: BTreeSet<&str> = note
            .links
            .iter()
            .filter(|link| link.kind == LinkType::About)
            .map(|link| link.to.as_str())
            .chain(note.source.files.iter().map(String::as_str))
            .collect();
        assert_eq!(entry.paths, paths.into_iter().collect::<Vec<_>>());
        // Links to notes, listed or bare in the body, with why, or
        // their type when no reason was given; paths are not links.
        let links: Vec<(String, String)> = note
            .all_links()
            .into_iter()
            .filter(|link| link.kind != LinkType::About)
            .map(|link| {
                let why = link
                    .why
                    .unwrap_or_else(|| link.kind.as_str().replace('_', " "));
                (link.to, why)
            })
            .collect();
        let shown: Vec<(String, String)> = entry
            .links
            .iter()
            .map(|link| (link.to.clone(), link.why.clone()))
            .collect();
        assert_eq!(shown, links);
        assert!(shown.iter().all(|(to, _)| !PATHS.contains(&to.as_str())));
        assert_eq!(
            entry
                .body
                .first()
                .is_some_and(|p| p.starts_with("May be stale")),
            note.stale.is_some()
        );
        // Paragraphs are the body's, one line each.
        let paragraphs = note.body.split("\n\n").count();
        // The stale mark, then the description, then the body.
        let extra = usize::from(note.stale.is_some()) + 1;
        assert_eq!(entry.body.len(), extra + paragraphs);
        assert!(entry.body.iter().all(|p| !p.contains('\n')));
    }
}

/// `ago` names the largest whole unit the time since fits, and never
/// runs backwards as more time passes.
#[hegel::test(test_cases = 300)]
fn ago_names_the_largest_whole_unit(tc: TestCase) {
    let then = tc.draw(gs::integers::<u64>().max_value(1 << 40));
    let a = tc.draw(gs::integers::<u64>().max_value(1 << 36));
    let b = tc.draw(gs::integers::<u64>().max_value(1 << 36));
    let (short, long) = (a.min(b), a.max(b));
    // Each unit, in seconds, and the next unit's size: the count stays
    // below it, but for years.
    const UNITS: [(&str, u64, u64); 5] = [
        ("minute", 60, 60),
        ("hour", 3_600, 24),
        ("day", 86_400, 30),
        ("month", 2_592_000, 13),
        ("year", 31_536_000, u64::MAX),
    ];
    let rank = |elapsed_ms: u64| -> (usize, u64) {
        let text = ago(then + elapsed_ms, then);
        let elapsed = elapsed_ms / 1000;
        if text == "just now" {
            assert!(elapsed < 60, "{text} after {elapsed}s");
            return (0, 0);
        }
        let mut words = text.split(' ');
        let count: u64 = words.next().unwrap().parse().unwrap();
        let word = words.next().unwrap();
        assert_eq!(words.collect::<Vec<_>>(), ["ago"], "{text}");
        let unit = word.trim_end_matches('s');
        tc.event(unit);
        let at = UNITS.iter().position(|(u, _, _)| *u == unit).unwrap();
        let (_, seconds, next) = UNITS[at];
        assert!(
            count * seconds <= elapsed && elapsed < (count + 1) * seconds,
            "{text} after {elapsed}s"
        );
        assert!((1..next).contains(&count), "{text}");
        assert_eq!(word.ends_with('s'), count != 1, "{text}");
        (at + 1, count)
    };
    assert!(rank(short) <= rank(long));
    assert_eq!(ago(then, then + long), "just now", "a future time is now");
}

fn commit(paths: &[&str]) -> TurnCommit {
    TurnCommit {
        change_id: "k".repeat(32),
        paths: paths.iter().map(|path| (*path).to_owned()).collect(),
    }
}

fn about(title: &str, path: &str) -> Draft {
    Draft {
        kind: NoteType::Fact,
        title: title.into(),
        description: "What it says, in a line.".into(),
        body: "What it says.".into(),
        tags: Vec::new(),
        links: vec![Link {
            to: path.into(),
            kind: LinkType::About,
            why: None,
        }],
        id: None,
        supersedes: None,
        source: source(Vec::new()),
    }
}

/// One step of a run that writes notes and commits: the clock moves on
/// by `wait` milliseconds first, possibly none, so a note can share a
/// commit's millisecond.
#[derive(Debug, Clone)]
enum Step {
    Write { wait: u64, path: &'static str },
    Commit { wait: u64, paths: Vec<&'static str> },
}

/// A commit marks a note about a file it changed when the note was written
/// no later than the previous commit, or than the run's start for the
/// first: a note written during the turn may postdate the change. A note
/// written later is marked by the next commit that touches its file. A
/// commit that changed nothing marks nothing, and still moves the bound.
/// The clock is the scopes' own, moved by the test, so every boundary is
/// exact. Model: each note's path, write time and mark, and the bound.
#[hegel::test(test_cases = 100)]
fn commits_mark_only_notes_written_before_the_previous_commit(tc: TestCase) {
    const PATHS: [&str; 3] = ["src/a.rs", "src/b.rs", "docs/c.md"];
    let steps: Vec<Step> = tc.draw(
        gs::vecs(hegel::one_of!(
            hegel::tuples!(
                gs::integers::<u64>().max_value(2),
                gs::sampled_from(PATHS.to_vec())
            )
            .map(|(wait, path)| Step::Write { wait, path }),
            hegel::tuples!(
                gs::integers::<u64>().max_value(2),
                gs::subsequences(PATHS.to_vec())
            )
            .map(|(wait, paths)| Step::Commit { wait, paths }),
        ))
        .max_size(12)
        .print_as_debug(),
    );
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicU64::new(1_000));
    let read = clock.clone();
    let scopes = Scopes::new(
        Memory::open(dir.path(), Box::new(Bm25::new())).unwrap(),
        None,
        Arc::new(move || read.load(Ordering::SeqCst)),
    );
    // A note written before the run began, at its first millisecond.
    let first = about("note 0", "src/a.rs");
    scopes.repo.lock().unwrap().write(first, 1_000).unwrap();
    let observe = stale_on_turn(MemoryPlugin::new(scopes.clone()));

    // Each note: its id, its path, when it was written, whether marked.
    let mut notes = vec![("note-0".to_owned(), "src/a.rs", 1_000, false)];
    let mut bound = 1_000;
    for step in steps {
        match step {
            Step::Write { wait, path } => {
                let now = clock.fetch_add(wait, Ordering::SeqCst) + wait;
                let title = format!("note {}", notes.len());
                let written = scopes
                    .repo
                    .lock()
                    .unwrap()
                    .write(about(&title, path), now)
                    .unwrap();
                notes.push((written.id, path, now, false));
            }
            Step::Commit { wait, paths } => {
                let now = clock.fetch_add(wait, Ordering::SeqCst) + wait;
                observe(&commit(&paths));
                let written_by = std::mem::replace(&mut bound, now);
                for (_, path, at, marked) in &mut notes {
                    if paths.contains(path) && *at <= written_by {
                        tc.event("marked");
                        *marked = true;
                    }
                }
            }
        }
        let memory = scopes.repo.lock().unwrap();
        for (id, _, _, marked) in &notes {
            let stale = memory.notes().get(id).unwrap().stale.is_some();
            assert_eq!(stale, *marked, "{id}");
        }
    }
}

/// Scopes open once: every run in a repository shares its notes.
#[test]
fn a_scope_opens_once() {
    let data = tempfile::tempdir().unwrap();
    let memories = Memories::new(Search::Keywords);
    let a = memories.scope(data.path()).unwrap();
    let b = memories.scope(data.path()).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    a.lock()
        .unwrap()
        .write(about("shared", "src/a.rs"), now())
        .unwrap();
    assert_eq!(memories.notebook(data.path()).notes.len(), 1);
}

/// What memory's fold makes of its records: a note for each recall,
/// save and failure, in order, and its line from what it said as the run
/// started.
#[hegel::test(test_cases = 100)]
fn the_fold_notes_what_memory_did(tc: TestCase) {
    use serde_json::json;
    use tau_memory::ui::{Mark, State};
    let kinds: Vec<u8> =
        tc.draw(gs::vecs(gs::integers::<u8>().max_value(3)).max_size(8));
    let mut state = State::default();
    let mut anchors = FakeRun::default();
    let mut expected = Vec::new();
    let mut notes = None;
    for (n, kind) in kinds.iter().enumerate() {
        let body = match kind {
            0 => {
                notes = Some(n);
                json!({ "kind": "starting", "notes": n })
            }
            1 => {
                expected.push(Mark::Recalled(vec![("n-1".into(), "a".into())]));
                json!({ "kind": "recalled", "notes": [{ "id": "n-1", "title": "a" }] })
            }
            2 => {
                expected.push(Mark::Saved(n));
                json!({ "kind": "saved", "calls": vec![json!({ "tool": "memory_write" }); n] })
            }
            _ => {
                expected.push(Mark::Failed("offline".into()));
                json!({ "kind": "error", "message": "offline" })
            }
        };
        tau_ui_plugin::testing::fold(
            tau_memory::ui::MemoryUi,
            &mut state,
            &body,
            &mut anchors,
        );
    }
    let marks: Vec<Mark> = anchors
        .anchors
        .iter()
        .map(|key| state.marks[key].clone())
        .collect();
    assert_eq!(marks, expected);
    assert_eq!(state.notes, notes);
    assert_eq!(state.status().is_some(), notes.is_some());
}
