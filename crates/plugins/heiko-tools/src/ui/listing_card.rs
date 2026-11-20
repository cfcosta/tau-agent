//! The `ls` step's body (ADR 0064): the names, the folders first, each
//! in a tone that says what it is: a folder, a file `@` changes (with
//! the letter of how), one `.gitignore` leaves out. The host lays them
//! out in columns and shows the first few until the step is opened.

use heiko_ui_plugin::{
    step::{Body, Name},
    tree::Tone,
};

use super::listing::{ChangeKind, DirListing, Listed};

/// The names, the folders then the files, and a last quiet one when
/// the listing was cut at its limit. None for an empty directory.
pub fn body(listing: &DirListing) -> Option<Body> {
    let mut names: Vec<Name> = listing
        .dirs()
        .chain(listing.files())
        .map(|listed| {
            let (text, tone) = name(listed);
            Name { text, tone }
        })
        .collect();
    if names.is_empty() {
        return None;
    }
    if listing.truncated {
        names.push(Name {
            text: "(cut at the limit)".into(),
            tone: Some(Tone::Neutral),
        });
    }
    Some(Body::Names {
        names,
        noun: "entry".into(),
    })
}

/// One entry's name and the tone it is set in: a folder's with its
/// slash, a symlink's with where it points, a changed file's with the
/// letter of its change.
fn name(listed: &Listed) -> (String, Option<Tone>) {
    let entry = &listed.entry;
    let mut text = entry.name.clone();
    if listed.is_dir() {
        text.push('/');
    }
    if let Some(target) = &entry.target {
        text.push_str(&format!(" → {target}"));
    }
    let tone = if entry.ignored {
        Some(Tone::Neutral)
    } else if let Some(kind) = listed.change {
        let (letter, tone) = change_look(kind);
        text.push(' ');
        text.push_str(letter);
        Some(tone)
    } else if listed.is_dir() {
        Some(Tone::Info)
    } else {
        None
    };
    (text, tone)
}

fn change_look(kind: ChangeKind) -> (&'static str, Tone) {
    match kind {
        ChangeKind::Added => ("A", Tone::Good),
        ChangeKind::Modified => ("M", Tone::Warn),
        ChangeKind::Removed => ("D", Tone::Bad),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn listing(files: usize, truncated: bool) -> DirListing {
        let mut entries = vec![json!({ "name": "src", "kind": "dir" })];
        entries.extend((0..files).map(|n| {
            json!({ "name": format!("file-{n}.rs"), "kind": "file", "size": 1 })
        }));
        DirListing::parse(
            &json!({ "entries": entries, "truncated": truncated }),
        )
        .expect("a listing")
    }

    fn names(body: Option<Body>) -> Vec<(String, Option<Tone>)> {
        match body {
            Some(Body::Names { names, noun }) => {
                assert_eq!(noun, "entry");
                names
                    .into_iter()
                    .map(|name| (name.text, name.tone))
                    .collect()
            }
            other => panic!("names: {other:?}"),
        }
    }

    /// The folders come first, with their slash and in the info tone;
    /// a listing cut at its limit says so last.
    #[test]
    fn folders_lead_and_a_cut_listing_says_so() {
        let listed = names(body(&listing(2, false)));
        assert_eq!(listed[0], ("src/".into(), Some(Tone::Info)));
        assert_eq!(listed[1], ("file-0.rs".into(), None));
        assert_eq!(listed.len(), 3);
        let cut = names(body(&listing(1, true)));
        assert_eq!(cut.last().unwrap().0, "(cut at the limit)");
        // A directory with one entry still has a body.
        assert!(body(&listing(0, false)).is_some());
    }
}
