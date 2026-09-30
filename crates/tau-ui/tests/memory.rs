//! Memory in the app: the Memory screen's view of a scope's notes, how
//! long ago a note was edited, and stale marks from turns' commits
//! bounded by when the notes were written.

use std::{collections::BTreeSet, path::Path};

use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use tau_memory::{
    Memory,
    index::Bm25,
    memory::{Draft, WriteError},
    note::{By, Link, LinkType, NoteType, Source},
};
use tau_ui::memory::{Memories, ago, catalog, now, stale_on_turn};
use tau_vcs::TurnSnapshot;

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

fn commit(paths: &[&str]) -> TurnSnapshot {
    TurnSnapshot {
        commit_id: "0".repeat(40),
        change_id: "k".repeat(32),
        head: "1".repeat(40),
        paths: paths.iter().map(|path| (*path).to_owned()).collect(),
    }
}

fn wait_a_moment() {
    std::thread::sleep(std::time::Duration::from_millis(3));
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

fn stale(memories: &Memories, dir: &Path, id: &str) -> bool {
    let scope = memories.scope(dir).unwrap();
    let memory = scope.lock().unwrap();
    memory.notes().get(id).unwrap().stale.is_some()
}

/// A commit marks notes written before the turn it ends, not the ones
/// written during it: the change may have come first. The next commit
/// that touches the file marks those too. A commit that changed nothing
/// marks nothing.
#[test]
fn commits_mark_only_notes_older_than_their_turn() {
    let data = tempfile::tempdir().unwrap();
    let (repo, user) = (data.path().join("repo"), data.path().join("user"));
    let memories = Memories::keywords();
    let plugin = memories.plugin(&repo, &user).unwrap();
    let scope = memories.scope(&repo).unwrap();
    scope
        .lock()
        .unwrap()
        .write(about("before the run", "src/a.rs"), now())
        .unwrap();
    wait_a_moment();

    let observe = stale_on_turn(plugin);
    wait_a_moment();
    observe(&commit(&[]));
    assert!(
        !stale(&memories, &repo, "before-the-run"),
        "nothing changed"
    );
    wait_a_moment();
    scope
        .lock()
        .unwrap()
        .write(about("during the turn", "src/a.rs"), now())
        .unwrap();
    wait_a_moment();

    observe(&commit(&["src/b.rs"]));
    assert!(!stale(&memories, &repo, "before-the-run"), "another file");
    wait_a_moment();
    // The bound is now past the note written during the turn.
    scope
        .lock()
        .unwrap()
        .write(about("during the next", "src/a.rs"), now())
        .unwrap();
    wait_a_moment();
    observe(&commit(&["src/a.rs"]));
    assert!(stale(&memories, &repo, "before-the-run"));
    assert!(stale(&memories, &repo, "during-the-turn"));
    assert!(!stale(&memories, &repo, "during-the-next"));
}

/// Scopes open once: every run in a repository shares its notes.
#[test]
fn a_scope_opens_once() {
    let data = tempfile::tempdir().unwrap();
    let memories = Memories::keywords();
    let a = memories.scope(data.path()).unwrap();
    let b = memories.scope(data.path()).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    a.lock()
        .unwrap()
        .write(about("shared", "src/a.rs"), now())
        .unwrap();
    assert_eq!(memories.catalog(data.path()).notes.len(), 1);
}
