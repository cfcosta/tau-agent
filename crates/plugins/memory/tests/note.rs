//! Notes read back as they were written, and links come from the front
//! matter and the body alike.

use hegel::{TestCase, generators as gs};
use tau_memory::note::{
    By,
    Link,
    LinkType,
    Note,
    NoteType,
    Source,
    is_id,
    slug,
    wiki_links,
};

#[hegel::composite]
fn id(tc: TestCase) -> String {
    tc.draw(gs::from_regex("[a-z0-9][a-z0-9-]{0,20}"))
}

/// One line of any text, not blank.
#[hegel::composite]
fn line(tc: TestCase) -> String {
    let text: String =
        tc.draw(gs::text().exclude_characters("\n\r").max_size(40));
    format!("x{text}")
}

#[hegel::composite]
fn note(tc: TestCase) -> Note {
    let links = (0..tc.draw(gs::integers::<usize>().max_value(3)))
        .map(|_| Link {
            to: tc.draw(id()),
            kind: tc.draw(gs::sampled_from(LinkType::ALL.to_vec())),
            why: tc.draw(gs::optional(line())),
        })
        .collect();
    let created: u64 = tc.draw(gs::integers::<u64>().max_value(1 << 50));
    Note {
        id: tc.draw(id()),
        title: tc.draw(line()),
        description: tc.draw(line()),
        kind: tc.draw(gs::sampled_from(NoteType::ALL.to_vec())),
        tags: tc.draw(gs::vecs(line()).max_size(3)),
        created,
        updated: created,
        valid_from: created,
        valid_to: tc
            .draw(gs::optional(gs::integers::<u64>().max_value(1 << 50))),
        stale: tc.draw(gs::optional(line())),
        source: Source {
            by: tc.draw(gs::sampled_from(vec![
                By::User,
                By::Agent,
                By::Inferred,
            ])),
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

#[hegel::test(test_cases = 300)]
fn a_note_reads_back_as_written(tc: TestCase) {
    let note = tc.draw(note());
    note.validate().unwrap();
    let text = note.render();
    assert_eq!(Note::parse(&text).unwrap(), note);
    // And its file does not change on a second trip.
    assert_eq!(Note::parse(&text).unwrap().render(), text);
}

#[hegel::test]
fn a_bare_link_in_the_body_relates(tc: TestCase) {
    let mut note = tc.draw(note());
    let target = tc.draw(id());
    note.body =
        format!("See [[{target}]] and [[{target}]], not [[Not An Id]].");
    let links = note.all_links();
    let bare: Vec<&Link> = links.iter().filter(|l| l.to == target).collect();
    // Once, and as listed when the front matter names it already.
    assert_eq!(bare.len(), 1);
    match note.links.iter().find(|l| l.to == target) {
        Some(listed) => assert_eq!(bare[0], listed),
        None => assert_eq!(bare[0].kind, LinkType::Relates),
    }
    assert!(links.iter().all(|l| l.to != "Not An Id"));
}

#[hegel::test]
fn a_slug_is_an_id(tc: TestCase) {
    let title: String = tc.draw(gs::text().max_size(200));
    let id = slug(&title);
    assert!(is_id(&id), "{id:?} from {title:?}");
}

#[test]
fn wiki_links_skip_what_is_not_an_id() {
    assert_eq!(wiki_links("[[a]] [[B]] [[a]] [[c-2]] [[x"), ["a", "c-2"]);
}

#[test]
fn a_multi_line_title_is_refused() {
    let mut note = Note::parse(
        "+++\nid = \"a\"\ntitle = \"t\"\ndescription = \"d\"\ntype = \"fact\"\n\
         created = 1\nupdated = 1\nvalid_from = 1\n\n[source]\nby = \"user\"\n+++\nbody",
    )
    .unwrap();
    assert!(note.validate().is_ok());
    note.title = "two\nlines".into();
    assert!(note.validate().is_err());
}
