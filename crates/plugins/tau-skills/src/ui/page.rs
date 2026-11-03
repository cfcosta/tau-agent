//! The Skills screen: each skill in the person's folder, with what it
//! is for, and each folder that is not one, with why. A view tree
//! (ADR 0045), in the frame the interface gives a page.

use serde_json::json;
use tau_ui_plugin::{
    ViewCx,
    tree::{Kind, Node, Tone},
};

use super::SkillsUi;
use crate::{Problem, SKILL_FILE, Skill, Skills};

/// The button's action: show the skills folder in the file manager.
const OPEN_FOLDER: &str = "open-folder";

pub fn title(_: &mut ViewCx<'_, SkillsUi>) -> String {
    "Skills".to_owned()
}

pub fn render(view: &mut ViewCx<'_, SkillsUi>) -> gpui::AnyElement {
    let skills = view.data.clone();
    let tree = tree(&skills);
    let dir = skills.dir.clone();
    view.tree("skills", &tree, move |action, _, cx| {
        if action == OPEN_FOLDER
            && let Some(dir) = &dir
        {
            cx.reveal_path(dir);
        }
    })
}

/// The screen: its header, then the skills and the folders that are not.
pub fn tree(skills: &Skills) -> Node {
    Node::stack([header(skills), list(skills)])
}

fn header(skills: &Skills) -> Node {
    let mut title = vec![
        Node::Heading {
            text: "Skills".into(),
        },
        Node::Spacer,
    ];
    if skills.dir.is_some() {
        title.push(Node::Button {
            label: "Open folder".into(),
            action: OPEN_FOLDER.into(),
            args: json!({}),
            kind: Kind::Secondary,
            icon: None,
        });
    }
    let mut header = vec![
        Node::row(title),
        Node::text(format!(
            "Instructions the model loads when a task calls for them, or \
             that you start with /name. Each is a folder with a {SKILL_FILE}."
        )),
    ];
    if let Some(dir) = &skills.dir {
        header.push(Node::row([
            Node::text("From"),
            Node::Mono {
                text: dir.display().to_string(),
            },
            Node::text(
                "read as each run starts; new ones show in the next run",
            ),
        ]));
    }
    Node::stack(header)
}

fn list(skills: &Skills) -> Node {
    if skills.found.is_empty() && skills.problems.is_empty() {
        return Node::Empty {
            text: format!(
                "No skills yet. A skill is a folder here with a {SKILL_FILE}: \
                 YAML frontmatter with a name and a description, then the \
                 instructions."
            ),
        };
    }
    Node::stack(
        skills
            .found
            .iter()
            .map(row)
            .chain(skills.problems.iter().map(problem_row)),
    )
}

fn row(skill: &Skill) -> Node {
    let mut files = match skill.files {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    if skill.builtin {
        files.push_str(" · built in");
    }
    let mut children = vec![
        Node::row([
            Node::Mono {
                text: skill.name.clone(),
            },
            Node::Spacer,
            Node::Mono { text: files },
        ]),
        Node::text(skill.description.clone()),
    ];
    if !skill.ignored.is_empty() {
        children.push(Node::text(format!(
            "Also sets {}, which tau does not act on yet.",
            skill.ignored.join(", ")
        )));
    }
    Node::Card { children }
}

fn problem_row(problem: &Problem) -> Node {
    let name = problem
        .dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Node::Card {
        children: vec![
            Node::row([
                Node::Icon {
                    name: "warning".into(),
                    tone: Tone::Bad,
                },
                Node::Mono { text: name },
                Node::Badge {
                    text: "not offered to the model".into(),
                    tone: Tone::Bad,
                },
            ]),
            Node::text(problem.reason.clone()),
            Node::Mono {
                text: problem.dir.display().to_string(),
            },
        ],
    }
}
