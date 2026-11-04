//! The Memory tab's ways into imports (ADR 0044): "Import from Claude
//! Code or Codex…" on the notes page and in a repository's menu, and a
//! banner on a repository's notes when the tools know it: proposals
//! wait for it, or there is what they kept that no import read.

use gpui::AnyElement;
use tau_ui_plugin::{
    ViewCx,
    points::AtRepo,
    tree::{Kind, Node, Tone},
};

use super::{Do, MemoryUi, button, icon, on_do, review_link};
use crate::import::{Act, Banner, Imports, Request, Tool};

/// What the way into an import says.
pub const IMPORT: &str = "Import from Claude Code or Codex…";

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Starts an import of every listed repository, and opens the review
/// page to watch it.
fn import() -> Do {
    Do::Import {
        request: Request::default(),
    }
}

/// The notes page's button, when either tool is on this computer.
pub fn import_button(imports: &Imports) -> Option<Node> {
    imports
        .any_found()
        .then(|| button(IMPORT, Kind::Secondary, &import()))
}

/// The repository menu's entry, when either tool is on this computer.
pub fn menu_entry(
    _at: &AtRepo,
    view: &mut ViewCx<'_, MemoryUi>,
) -> Option<AnyElement> {
    let tree = menu_tree(&view.data.imports)?;
    let handle = view.handle.clone();
    Some(view.tree("menu-memory-import", &tree, on_do(handle, None)))
}

/// The menu entry's tree.
pub fn menu_tree(imports: &Imports) -> Option<Node> {
    imports.any_found().then(|| {
        Node::row([
            icon("memory", Tone::Neutral),
            button(IMPORT, Kind::Secondary, &import()),
        ])
    })
}

/// What the tools kept for the repository, in words: "Claude Code kept 9
/// notes for ~/work/docbert, and Codex 4 task summaries."
fn kept(banner: &Banner) -> String {
    let parts: Vec<String> = banner
        .saved
        .iter()
        .map(|(tool, n)| match tool {
            Tool::ClaudeCode => plural(*n, "note", "notes"),
            Tool::Codex => plural(*n, "task summary", "task summaries"),
        })
        .collect();
    let tools: Vec<&str> =
        banner.saved.iter().map(|(tool, _)| tool.label()).collect();
    match (tools.as_slice(), parts.as_slice()) {
        ([one], [what]) => format!("{one} kept {what} for {}.", banner.folder),
        ([first, second], [a, b]) => {
            format!("{first} kept {a} for {}, and {second} {b}.", banner.folder)
        }
        _ => format!("Claude Code or Codex worked in {}.", banner.folder),
    }
}

/// The banner on `repo`'s notes, when its tools know it.
pub fn banner(repo: &str, imports: &Imports) -> Option<Node> {
    let banner = imports.banners.get(repo)?.clone();
    let mut text = kept(&banner);
    if banner.ready > 0 {
        text.push_str(&format!(
            " {} ready to look at.",
            match banner.ready {
                1 => "1 note is".to_owned(),
                n => format!("{n} notes are"),
            }
        ));
    }
    let read = Do::Import {
        request: Request {
            only: banner.places.clone(),
            ..Request::default()
        },
    };
    let mut buttons = vec![if banner.ready > 0 {
        button(
            format!("Review {}", plural(banner.ready, "note", "notes")),
            Kind::Primary,
            &Do::Open {
                link: review_link(Some(repo)),
            },
        )
    } else {
        button("Bring what they kept", Kind::Primary, &read)
    }];
    if banner.unread_chats > 0 && banner.ready > 0 {
        // Reads the chats too, staying on the notes.
        buttons.push(button(
            format!(
                "Read its {} too",
                plural(banner.unread_chats, "chat", "chats")
            ),
            Kind::Secondary,
            &Do::Act {
                act: Act::Import(Request {
                    only: banner.places.clone(),
                    ..Request::default()
                }),
            },
        ));
    }
    buttons.push(button(
        "Not now",
        Kind::Secondary,
        &Do::Act {
            act: Act::NotNow {
                repo: repo.to_owned(),
            },
        },
    ));
    Some(Node::Card {
        children: vec![
            Node::Heading {
                text: "Claude Code and Codex know this repository".into(),
            },
            Node::text(text),
            Node::row(buttons),
        ],
    })
}
