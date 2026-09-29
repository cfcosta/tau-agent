//! Notes read back as they were written, and links come from the front
//! matter and the body alike.

mod common;

use common::{id, note};
use hegel::{TestCase, generators as gs};
use tau_memory::note::{Link, LinkType, Note, is_id, slug, wiki_links};

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
