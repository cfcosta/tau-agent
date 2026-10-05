//! Generators shared by the tests: notes valid by construction.

#![allow(dead_code)]

use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use tau_memory_host::note::{By, Link, LinkType, Note, NoteType, Source};

#[hegel::composite]
pub fn id(tc: &TestCase) -> String {
    tc.draw(gs::from_regex("[a-z0-9][a-z0-9-]{0,20}"))
}

/// One line of any text, not blank.
#[hegel::composite]
pub fn line(tc: &TestCase) -> String {
    let text: String =
        tc.draw(gs::text().exclude_characters("\n\r").max_size(40));
    format!("x{text}")
}

pub fn note() -> impl PrintableGenerator<Note> {
    // Note is tau's own type, so its drawn values print through Debug.
    note_unprinted().print_as_debug()
}

#[hegel::composite]
fn note_unprinted(tc: &TestCase) -> Note {
    let links = (0..tc.draw(gs::integers::<usize>().max_value(3)))
        .map(|_| Link {
            to: tc.draw(id()),
            kind: tc.draw(
                gs::sampled_from(LinkType::ALL.to_vec()).print_as_debug(),
            ),
            why: tc.draw(gs::optional(line())),
        })
        .collect();
    // Each time on its own: a round trip that swapped two would show.
    let time = || gs::integers::<u64>().max_value(1 << 50);
    Note {
        id: tc.draw(id()),
        title: tc.draw(line()),
        description: tc.draw(line()),
        kind: tc
            .draw(gs::sampled_from(NoteType::ALL.to_vec()).print_as_debug()),
        tags: tc.draw(gs::vecs(line()).max_size(3)),
        created: tc.draw(time()),
        updated: tc.draw(time()),
        valid_from: tc.draw(time()),
        valid_to: tc
            .draw(gs::optional(gs::integers::<u64>().max_value(1 << 50))),
        stale: tc.draw(gs::optional(line())),
        source: Source {
            by: tc.draw(
                gs::sampled_from(vec![By::User, By::Agent, By::Inferred])
                    .print_as_debug(),
            ),
            run: tc.draw(gs::optional(line())),
            turn: tc.draw(gs::optional(gs::integers::<u32>())),
            commit: tc.draw(gs::optional(line())),
            files: tc.draw(gs::vecs(line()).max_size(3)),
        },
        links,
        // Any body, fence lines included: only the first closes the
        // front matter.
        body: tc.draw(hegel::one_of!(
            gs::text().max_size(200),
            gs::just("+++\n+++\nstill body\n".to_owned()),
        )),
    }
}

/// Ids from a small pool, so notes collide and link to one another.
#[hegel::composite]
pub fn pool_id(tc: &TestCase) -> String {
    tc.draw(gs::sampled_from(vec!["a", "b", "c", "d", "e"]))
        .to_owned()
}

/// A current note from the pool, linking within it, with some bare
/// `[[id]]` links in its body.
pub fn pool_note() -> impl PrintableGenerator<Note> {
    // Note is tau's own type, so its drawn values print through Debug.
    pool_note_unprinted().print_as_debug()
}

#[hegel::composite]
fn pool_note_unprinted(tc: &TestCase) -> Note {
    let mut note = tc.draw(note());
    note.id = tc.draw(pool_id());
    note.valid_to = None;
    note.stale = None;
    // Written before the tests' clock starts, at 1_000, or just after:
    // old enough, often, for a change to make it stale.
    note.created = tc.draw(gs::integers::<u64>().max_value(1_100));
    note.updated = tc.draw(gs::integers::<u64>().max_value(1_100));
    note.links = (0..tc.draw(gs::integers::<usize>().max_value(3)))
        .map(|_| Link {
            to: tc.draw(pool_id()),
            kind: tc.draw(
                gs::sampled_from(vec![
                    LinkType::Relates,
                    LinkType::Refines,
                    LinkType::Contradicts,
                    LinkType::DerivedFrom,
                ])
                .print_as_debug(),
            ),
            why: None,
        })
        .collect();
    // Some notes come from, or are about, the files the tests change.
    note.source.files = ["src/a.rs", "src/b.rs"]
        .into_iter()
        .filter(|_| tc.draw(gs::booleans()))
        .map(str::to_owned)
        .collect();
    if tc.draw(gs::booleans()) {
        note.links.push(Link {
            to: tc
                .draw(gs::sampled_from(vec!["src/a.rs", "src/b.rs"]))
                .to_owned(),
            kind: LinkType::About,
            why: None,
        });
    }
    let bare = tc.draw(gs::vecs(pool_id()).max_size(2));
    note.body = bare.iter().map(|id| format!("see [[{id}]]\n")).collect();
    note
}
