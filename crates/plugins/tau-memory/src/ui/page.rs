//! The notes: a search, a link for each kind of note, and the notes in
//! a table. A note opens in place of the list, with a small map of its
//! links and the links themselves under it. Both are view trees (ADR
//! 0045), in the frame the interface gives a page.

use serde_json::Value;
use tau_ui_plugin::{
    Link,
    ViewCx,
    points::{AtAnchor, AtRepo},
    tree::{GraphNode, Node, Tone, draft_key},
};

use super::{
    Mark,
    MemoryUi,
    NoteView,
    Notebook,
    SEARCH,
    USER,
    icon,
    notes_link,
};
use crate::{
    import::Imports,
    note::NoteType,
    record::{NAME, USER as USER_ID},
};

/// The notebook a page is about: a repository's, or with [`USER`] as its
/// scope, the user's.
fn notebook(view: &ViewCx<'_, MemoryUi>, repo: &str) -> Notebook {
    if repo == USER {
        view.data.you.clone()
    } else {
        view.repo(repo).cloned().unwrap_or_default()
    }
}

/// A note's page.
fn note_link(repo: &str, id: &str) -> Link {
    notes_link(repo).param("note", id.to_owned())
}

/// The list, showing only notes of `kind`; every note without one.
fn kind_link(repo: &str, kind: Option<NoteType>) -> Link {
    match kind {
        Some(kind) => notes_link(repo).param("kind", kind.as_str()),
        None => notes_link(repo),
    }
}

/// A kind's name on its link, for many notes.
fn plural(kind: NoteType) -> &'static str {
    match kind {
        NoteType::Fact => "Facts",
        NoteType::Convention => "Conventions",
        NoteType::Decision => "Decisions",
        NoteType::Gotcha => "Gotchas",
        NoteType::Procedure => "Procedures",
        NoteType::Case => "Cases",
        NoteType::Preference => "Preferences",
        NoteType::Index => "Indexes",
    }
}

/// The tone a kind's name is written in.
pub(super) fn kind_tone(kind: NoteType) -> Tone {
    match kind {
        NoteType::Fact | NoteType::Decision | NoteType::Preference => {
            Tone::Info
        }
        NoteType::Gotcha => Tone::Warn,
        NoteType::Procedure => Tone::Good,
        NoteType::Convention | NoteType::Case | NoteType::Index => {
            Tone::Neutral
        }
    }
}

/// The page's title: the open note's, or whose notes they are.
pub fn title(view: &mut ViewCx<'_, MemoryUi>) -> String {
    let repo = view.param("repo").unwrap_or_default().to_owned();
    let book = notebook(view, &repo);
    match view.param("note").and_then(|id| book.note(id)) {
        Some(note) => note.title.clone(),
        None if repo == USER => "Your notes".to_owned(),
        None => "Memory".to_owned(),
    }
}

/// The page: the notes of the `repo` parameter, those of the `kind`
/// parameter alone when it is set, or the `note` parameter's note open.
pub fn render(view: &mut ViewCx<'_, MemoryUi>) -> Node {
    match view.param("repo").map(str::to_owned) {
        None => Node::Empty {
            text: "Pick a repository to see its notes.".into(),
        },
        Some(repo) => {
            let book = notebook(view, &repo);
            match view.param("note").and_then(|id| book.note(id)) {
                Some(note) => note_tree(&repo, &book, note),
                None => {
                    let kind = view.param("kind").and_then(NoteType::parse);
                    let query = view
                        .ui_value(&draft_key(SEARCH))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    list_tree(&repo, &book, &view.data.imports, query, kind)
                }
            }
        }
    }
}

/// Whether `note` matches `query`, already lowercase.
fn matches(note: &NoteView, query: &str) -> bool {
    query.is_empty()
        || note.title.to_lowercase().contains(query)
        || note.body.iter().any(|p| p.to_lowercase().contains(query))
}

/// The list: what Claude Code and Codex know of the repository, when
/// they know it; the search, the kinds and the way to import; then a
/// row for each note that matches both `query` and `kind`.
pub fn list_tree(
    repo: &str,
    book: &Notebook,
    imports: &Imports,
    query: &str,
    kind: Option<NoteType>,
) -> Node {
    let query = query.to_lowercase();
    let mut children = Vec::new();
    if repo == USER {
        children.push(Node::Title {
            text: "Your notes".into(),
            subtitle: Some(
                "What runs learned about you and how you work. Every \
                 repository's runs see these."
                    .into(),
            ),
        });
    } else if let Some(banner) = super::banner::banner(repo, imports) {
        children.push(banner);
    }
    let kinds = std::iter::once((None, "All", book.notes.len())).chain(
        NoteType::ALL.into_iter().filter_map(|each| {
            let count =
                book.notes.iter().filter(|note| note.kind == each).count();
            (count > 0).then_some((Some(each), plural(each), count))
        }),
    );
    let mut bar = vec![
        icon("search", Tone::Neutral),
        Node::Input {
            id: SEARCH.into(),
            placeholder: Some("Search notes".into()),
            multiline: false,
            submit: None,
            keep: true,
            secret: false,
            focus: None,
        },
    ];
    bar.extend(kinds.map(|(each, name, count)| {
        let label = format!("{name} {count}");
        if each == kind {
            Node::Pill {
                text: label,
                tone: Tone::Info,
            }
        } else {
            Node::Link {
                label,
                to: kind_link(repo, each),
            }
        }
    }));
    bar.extend(super::banner::import_button(imports));
    children.push(Node::row(bar));
    let rows: Vec<Vec<Node>> = book
        .notes
        .iter()
        .filter(|note| kind.is_none_or(|kind| note.kind == kind))
        .filter(|note| matches(note, &query))
        .map(|note| row(repo, book, note))
        .collect();
    children.push(if rows.is_empty() {
        Node::Empty {
            text: if book.notes.is_empty() {
                "No notes yet."
            } else {
                "No notes match."
            }
            .into(),
        }
    } else {
        Node::Table {
            head: vec![
                "Note".into(),
                "Kind".into(),
                "Links".into(),
                "Edited".into(),
            ],
            rows,
            widths: vec![6., 1.3, 1., 1.5],
        }
    });
    Node::stack(children)
}

/// How linked a note is, in words.
fn links(n: usize) -> String {
    match n {
        0 => "no links".to_owned(),
        1 => "1 link".to_owned(),
        n => format!("{n} links"),
    }
}

/// A note in the list: its title, opening it, and id over the start of
/// its text; its kind; how linked it is; when it was edited.
fn row(repo: &str, book: &Notebook, note: &NoteView) -> Vec<Node> {
    let linked = note.links.len() + book.backlinks(&note.id).count();
    vec![
        Node::stack([
            Node::row([
                Node::Link {
                    label: note.title.clone(),
                    to: note_link(repo, &note.id),
                },
                Node::Mono {
                    text: note.id.clone(),
                },
            ]),
            Node::text(note.snippet().replace('`', "")),
        ]),
        Node::Badge {
            text: note.kind.as_str().into(),
            tone: kind_tone(note.kind),
        },
        Node::text(links(linked)),
        Node::text(note.edited.clone()),
    ]
}

/// A note: the way back, who wrote it and when, its title and text, the
/// files it is about, and its links out and in.
pub fn note_tree(repo: &str, book: &Notebook, note: &NoteView) -> Node {
    let mut children = vec![
        Node::row([
            icon("back", Tone::Neutral),
            Node::Link {
                label: "All notes".into(),
                to: notes_link(repo),
            },
        ]),
        Node::Mono {
            text: format!(
                "{} · {} · edited {}",
                note.id, note.written_by, note.edited
            ),
        },
        Node::Title {
            text: note.title.clone(),
            subtitle: None,
        },
        Node::Markdown {
            text: note.body.join("\n\n"),
        },
    ];
    if !note.paths.is_empty() {
        children.push(Node::row(note.paths.iter().map(|path| Node::Badge {
            text: path.clone(),
            tone: Tone::Neutral,
        })));
    }
    children.extend(neighborhood(repo, book, note));
    Node::stack(children)
}

/// The note's links out and in, as a small map and as lists.
fn neighborhood(repo: &str, book: &Notebook, note: &NoteView) -> Vec<Node> {
    let out: Vec<(&NoteView, &str)> = note
        .links
        .iter()
        .filter_map(|link| {
            book.note(&link.to).map(|to| (to, link.why.as_str()))
        })
        .collect();
    let back: Vec<(&NoteView, &str)> = book.backlinks(&note.id).collect();

    // Fixed spots, staggered so labels never share a line: links out
    // above the note, backlinks below it.
    const ABOVE: [(f32, f32); 3] = [(0.22, 0.2), (0.78, 0.26), (0.5, 0.06)];
    const BELOW: [(f32, f32); 3] = [(0.22, 0.7), (0.5, 0.82), (0.78, 0.66)];
    let mut nodes = vec![GraphNode {
        id: "note".into(),
        label: None,
        x: 0.5,
        y: 0.5,
        tone: Tone::Neutral,
        focus: true,
        to: None,
    }];
    let mut edges = Vec::new();
    let spots = out
        .iter()
        .zip(ABOVE)
        .map(|((to, _), spot)| ("out", *to, spot, Tone::Info))
        .chain(
            back.iter()
                .zip(BELOW)
                .map(|((from, _), spot)| ("back", *from, spot, Tone::Neutral)),
        );
    for (n, (side, other, (x, y), tone)) in spots.enumerate() {
        let id = format!("{side}-{n}");
        edges.push(("note".to_owned(), id.clone()));
        nodes.push(GraphNode {
            id,
            label: Some(other.title.clone()),
            x,
            y,
            tone,
            focus: false,
            to: Some(note_link(repo, &other.id)),
        });
    }
    let mut lists = vec![
        Node::Heading {
            text: "Neighborhood".into(),
        },
        Node::Graph {
            height: 12.5,
            nodes,
            edges,
        },
        Node::Heading {
            text: format!("Links out · {}", out.len()),
        },
    ];
    lists.extend(link_rows(repo, &out));
    lists.push(Node::Heading {
        text: format!("Backlinks · {}", back.len()),
    });
    lists.extend(link_rows(repo, &back));
    lists
}

/// A row for each of `links`: the note, opening it, and why.
fn link_rows(repo: &str, links: &[(&NoteView, &str)]) -> Vec<Node> {
    links
        .iter()
        .map(|(target, why)| {
            Node::stack([
                Node::Link {
                    label: target.title.clone(),
                    to: note_link(repo, &target.id),
                },
                Node::text(format!("why: {why}")),
            ])
        })
        .collect()
}

/// A recalled note's page: the user's for a note of the user's scope.
fn recalled_link(repo: &str, id: &str) -> Link {
    match id.strip_prefix(USER_ID) {
        Some(id) => note_link(USER, id),
        None => note_link(repo, id),
    }
}

/// The repository menu's entry: the repository's notes.
pub fn menu_entry(
    at: &AtRepo,
    _view: &mut ViewCx<'_, MemoryUi>,
) -> Option<Node> {
    Some(menu_tree(&at.repo))
}

/// The menu entry's tree.
pub fn menu_tree(repo: &str) -> Node {
    Node::row([
        icon("memory", Tone::Neutral),
        Node::Link {
            label: "Memory".into(),
            to: notes_link(repo),
        },
    ])
}

/// One of memory's notes in a transcript: the notes a run started with,
/// each opening its page; what it saved; what failed.
pub fn mark(at: &AtAnchor, view: &mut ViewCx<'_, MemoryUi>) -> Option<Node> {
    let mark = view.state?.marks.get(&at.key)?;
    Some(mark_tree(mark, &at.run.repo))
}

/// A transcript note's tree, for a run in `repo`.
pub fn mark_tree(mark: &Mark, repo: &str) -> Node {
    let (text, tone, notes) = match mark {
        Mark::Recalled(notes) => (
            match notes.len() {
                1 => "recalled 1 note for this task".to_owned(),
                n => format!("recalled {n} notes for this task"),
            },
            Tone::Neutral,
            notes.as_slice(),
        ),
        Mark::Saved(n) => (
            match n {
                1 => "saved 1 note".to_owned(),
                n => format!("saved {n} notes"),
            },
            Tone::Good,
            &[][..],
        ),
        Mark::Failed(message) => (message.clone(), Tone::Bad, &[][..]),
    };
    let mut children = vec![Node::row([
        icon("memory", tone),
        Node::Status {
            text: NAME.into(),
            detail: Some(text),
            tone,
        },
    ])];
    if !notes.is_empty() {
        children.push(Node::row(notes.iter().map(|(id, title)| Node::Link {
            label: title.clone(),
            to: recalled_link(repo, id),
        })));
    }
    Node::stack(children)
}
