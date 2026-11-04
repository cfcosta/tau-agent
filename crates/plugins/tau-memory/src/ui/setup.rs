//! Memory's step in first-run setup (`points::SETUP`, ADR 0044): shown
//! only when Claude Code or Codex is on this computer. For each tool,
//! what it saved and its chats, both ticked; where its folders go among
//! the repositories just picked, and what goes to the person's notes;
//! whether to bring folders tau does not have yet; and the button that
//! starts the import and ends the step. The import goes on after setup.
//! A view tree (ADR 0045); what the person chose is UI state.

use serde::{Deserialize, Serialize};
use tau_ui_plugin::{
    ViewCx,
    points::{AtSetup, SetupRepo, SetupView},
    tree::{Kind, Node},
};

use super::{DO, Do, MemoryUi, button, on_do};
use crate::import::{Material, Parts, Place, Request, Tool, ToolFound};

/// The step's name among memory's.
pub const STEP: &str = "memories";

/// The UI state key of what the person chose so far.
pub const CHOICE: &str = "setup.choice";

/// What the person chose so far, kept per window: every part, and not
/// the folders tau does not have.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(default)]
pub struct Choice {
    pub parts: Parts,
    pub others: bool,
}

/// The place a picked repository is.
fn place_of(repo: &SetupRepo) -> Place {
    Place {
        folder: repo.folder.as_ref().map(|f| f.display().to_string()),
        remote: repo.remote.clone(),
    }
}

/// The repository's short name: `hello` of `octocat/hello`.
fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The step, when either tool is on this computer.
pub fn step(
    at: &AtSetup,
    view: &mut ViewCx<'_, MemoryUi>,
) -> Option<SetupView> {
    let imports = &view.data.imports;
    if !imports.any_found() {
        return None;
    }
    let choice = view
        .ui_value(CHOICE)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let names: Vec<&str> =
        imports.found.iter().map(|f| f.tool.label()).collect();
    let on = match names.as_slice() {
        [one] => format!("{one} is"),
        _ => format!("{} are", names.join(" and ")),
    };
    let tree = tree(&imports.found, &at.repos, choice);
    let handle = view.handle.clone();
    let body = view.tree("memory-setup", &tree, on_do(handle, None));
    Some(SetupView {
        step: STEP,
        title: "Memories".into(),
        heading: "Bring what your other agents learned".into(),
        detail: format!(
            "{on} on this computer. tau can turn what they remember into its \
             own notes, for the repositories you picked and about you. You see \
             every note before it is kept."
        ),
        body,
    })
}

/// The step's tree: a card for each tool found, whether to bring the
/// folders tau does not have, and the button that starts the import.
pub fn tree(found: &[ToolFound], picked: &[SetupRepo], choice: Choice) -> Node {
    let mut children: Vec<Node> = found
        .iter()
        .map(|found| card(found, picked, choice))
        .collect();
    children.push(Node::Switch {
        label: "Also bring folders tau does not have yet, to use when you \
                add them"
            .into(),
        detail: None,
        on: choice.others,
        action: DO.into(),
        args: Do::Choose {
            choice: Choice {
                others: !choice.others,
                ..choice
            },
        }
        .args(),
    });
    let request = choice.parts.any().then(|| Request {
        parts: choice.parts,
        only: picked.iter().map(place_of).collect(),
        others: choice.others,
    });
    children.push(Node::row([
        Node::text("Only chats in the folders above"),
        Node::Spacer,
        button(
            "Read and show me the notes",
            if request.is_some() {
                Kind::Primary
            } else {
                Kind::Secondary
            },
            &Do::Start { request },
        ),
    ]));
    children.push(Node::text(
        "Read only: tau never changes Claude Code's or Codex's files. What \
         they wrote is treated as notes to keep, never as instructions.",
    ));
    Node::stack(children)
}

/// One tool: what it has, what to read of it, and where it goes.
fn card(found: &ToolFound, picked: &[SetupRepo], choice: Choice) -> Node {
    let tool = found.tool;
    let folders = found.folders.iter().filter(|f| f.saved > 0).count();
    let (saved, saved_detail, chats, chats_detail) = match tool {
        Tool::ClaudeCode => (
            format!("Its memory · {}", plural(found.saved, "note", "notes")),
            format!(
                "Notes it saved in {}. Copied as they are, without the model.",
                plural(folders, "folder", "folders")
            ),
            format!(
                "Its chats · {}",
                plural(found.chats, "conversation", "conversations")
            ),
            "Your model reads them and writes down what was decided, fixed or \
             preferred."
                .to_owned(),
        ),
        Tool::Codex => (
            format!(
                "Its memories · {}",
                plural(found.saved, "summary", "summaries")
            ),
            "What Codex kept from past tasks, grouped by folder. Your model \
             splits them into notes."
                .to_owned(),
            format!(
                "Its chats · {}",
                plural(found.chats, "session", "sessions")
            ),
            "Read in full by your model. It keeps going after setup."
                .to_owned(),
        ),
    };
    let what = |material: Material, label: String, detail: String| {
        let on = choice.parts.reads(tool, material);
        let mut next = choice;
        next.parts.set(tool, material, !on);
        Node::Switch {
            label,
            detail: Some(detail),
            on,
            action: DO.into(),
            args: Do::Choose { choice: next }.args(),
        }
    };
    // Where its folders go: each picked repository that claims one.
    let unit = match tool {
        Tool::ClaudeCode => ("note", "notes"),
        Tool::Codex => ("summary", "summaries"),
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut claimed = vec![false; found.folders.len()];
    for repo in picked {
        let place = place_of(repo);
        let mut count = 0;
        let mut label = None;
        for (n, folder) in found.folders.iter().enumerate() {
            if folder.place.matches(
                place.folder.as_deref().map(std::path::Path::new),
                place.remote.as_deref(),
            ) {
                claimed[n] = true;
                count += folder.saved;
                label.get_or_insert_with(|| folder.label.clone());
            }
        }
        if let Some(label) = label {
            rows.push((
                label,
                format!(
                    "{} · {}",
                    short(&repo.name),
                    plural(count, unit.0, unit.1)
                ),
            ));
        }
    }
    if found.about_you > 0 {
        rows.push((
            "about you, in any folder".into(),
            format!("About you · {}", plural(found.about_you, unit.0, unit.1)),
        ));
    }
    let left = claimed.iter().filter(|claimed| !**claimed).count();
    if left > 0 {
        rows.push((
            plural(left, "other folder", "other folders"),
            if choice.others {
                "kept for when you add them".into()
            } else {
                "left out".into()
            },
        ));
    }
    Node::Card {
        children: vec![
            Node::Heading {
                text: tool.label().into(),
            },
            Node::Mono {
                text: tool.root().into(),
            },
            what(Material::Saved, saved, saved_detail),
            what(Material::Chat, chats, chats_detail),
            Node::Table {
                head: Vec::new(),
                rows: rows
                    .into_iter()
                    .map(|(from, to)| {
                        vec![
                            Node::Mono { text: from },
                            Node::text("→"),
                            Node::text(to),
                        ]
                    })
                    .collect(),
                widths: vec![4., 0.5, 4.],
            },
        ],
    }
}
