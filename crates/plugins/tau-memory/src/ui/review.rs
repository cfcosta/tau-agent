//! The review page of what imports propose (ADR 0044): how many notes
//! wait and where they came from, the import going on, tabs for every
//! proposal, the person's, each repository's and those that need a
//! look, and a row per proposal to tick or untick. "Keep N notes" keeps
//! the ticked ones in view, "Keep none" drops those in view. New
//! proposals join as the import goes. It draws from memory's catalog
//! data and asks its host half, so it works on a phone too. A view tree
//! (ADR 0045), in the frame the interface gives a page.

use serde_json::Value;
use tau_ui_plugin::{
    Link,
    ViewCx,
    tree::{Kind, Node, Tone, draft_key},
};

use super::{
    Do,
    MemoryUi,
    REVIEW,
    REVIEW_SEARCH,
    USER,
    icon,
    page::kind_tone,
    review_link,
};
use crate::import::{Act, Imports, Kept, Material, Pending, Request, Tool};

/// The tab a page shows: every proposal, the person's, a repository's,
/// or those that need a look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    All,
    You,
    Repo(String),
    Look,
}

impl Tab {
    fn of(view: &ViewCx<'_, MemoryUi>) -> Self {
        if view.param("look").is_some() {
            return Self::Look;
        }
        match view.param("repo") {
            None | Some("" | "all") => Self::All,
            Some(USER) => Self::You,
            Some(repo) => Self::Repo(repo.to_owned()),
        }
    }

    fn holds(&self, pending: &Pending) -> bool {
        match self {
            Self::All => true,
            Self::You => pending.repo.is_none(),
            Self::Repo(repo) => pending.repo.as_deref() == Some(repo),
            Self::Look => pending.flags.need_a_look(),
        }
    }

    fn link(&self) -> Link {
        match self {
            Self::All => review_link(None),
            Self::You => review_link(Some(USER)),
            Self::Repo(repo) => review_link(Some(repo)),
            Self::Look => review_link(None).param("look", "1"),
        }
    }

    fn name(&self) -> String {
        match self {
            Self::All => "All".to_owned(),
            Self::You => "About you".to_owned(),
            Self::Repo(repo) => repo.clone(),
            Self::Look => "Need a look".to_owned(),
        }
    }
}

/// The page's title.
pub fn title(_view: &mut ViewCx<'_, MemoryUi>) -> String {
    "Notes to keep".to_owned()
}

/// A button of the page's.
fn button(label: impl Into<String>, kind: Kind, what: &Do) -> Node {
    super::button(REVIEW, label, kind, what)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Where the proposals came from, and what was left out or merged: "From
/// 501 Claude Code notes and 17 Codex summaries: 380 were about folders
/// you left out, 15 said the same as another."
pub fn summary(imports: &Imports) -> Option<String> {
    let tally = &imports.tally;
    let mut from = Vec::new();
    let read = |tool, material| tally.read_of(tool, material);
    let claude = read(Tool::ClaudeCode, Material::Saved);
    if claude > 0 {
        from.push(format!(
            "{} Claude Code {}",
            claude,
            if claude == 1 { "note" } else { "notes" }
        ));
    }
    let codex = read(Tool::Codex, Material::Saved);
    if codex > 0 {
        from.push(format!(
            "{} Codex {}",
            codex,
            if codex == 1 { "summary" } else { "summaries" }
        ));
    }
    let chats = read(Tool::ClaudeCode, Material::Chat)
        + read(Tool::Codex, Material::Chat);
    if chats > 0 {
        from.push(plural(chats, "chat", "chats"));
    }
    if from.is_empty() {
        return None;
    }
    let from = match from.as_slice() {
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => unreachable!("not empty"),
    };
    let mut then = Vec::new();
    if tally.left_out > 0 {
        then.push(format!(
            "{} about folders you left out",
            if tally.left_out == 1 {
                "1 was".to_owned()
            } else {
                format!("{} were", tally.left_out)
            }
        ));
    }
    if tally.merged > 0 {
        then.push(format!(
            "{} the same as another",
            if tally.merged == 1 {
                "1 said".to_owned()
            } else {
                format!("{} said", tally.merged)
            }
        ));
    }
    if tally.known > 0 {
        then.push(format!(
            "{} what a note says already",
            if tally.known == 1 {
                "1 said".to_owned()
            } else {
                format!("{} said", tally.known)
            }
        ));
    }
    if tally.refused > 0 {
        then.push(format!(
            "{} refused as instructions to the model",
            if tally.refused == 1 {
                "1 was".to_owned()
            } else {
                format!("{} were", tally.refused)
            }
        ));
    }
    Some(if then.is_empty() {
        format!("From {from}.")
    } else {
        format!("From {from}: {}.", then.join(", "))
    })
}

/// The page.
pub fn render(view: &mut ViewCx<'_, MemoryUi>) -> Node {
    let tab = Tab::of(view);
    let query = view
        .ui_value(&draft_key(REVIEW_SEARCH))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kept: Option<Kept> = view
        .ui_value(super::KEPT)
        .and_then(|kept| serde_json::from_value(kept.clone()).ok());
    tree(&view.data.imports, &tab, query, kept.as_ref())
}

/// The page's tree: the proposals of `tab` that match `query`, and what
/// keeping last did.
pub fn tree(
    imports: &Imports,
    tab: &Tab,
    query: &str,
    kept: Option<&Kept>,
) -> Node {
    let query = query.to_lowercase();
    let shown: Vec<&Pending> = imports
        .pending
        .iter()
        .filter(|pending| tab.holds(pending))
        .filter(|pending| {
            query.is_empty()
                || pending.title.to_lowercase().contains(&query)
                || pending.description.to_lowercase().contains(&query)
        })
        .collect();
    let mut children = head(imports);
    children.push(tabs(imports, tab));
    if let Some(kept) = kept {
        children.push(Node::text(said(kept)));
    }
    children.extend(list(&shown, imports));
    if !shown.is_empty() {
        children.push(foot(&shown));
    }
    if !imports.failed.is_empty() {
        children.push(failed(imports));
    }
    Node::stack(children)
}

/// What keeping did.
fn said(kept: &Kept) -> String {
    let mut line = format!("Kept {}.", plural(kept.kept, "note", "notes"));
    for (id, why) in &kept.refused {
        line.push_str(&format!(" {id} was not kept: {why}."));
    }
    line
}

/// How many wait, where they came from, and the import going on.
fn head(imports: &Imports) -> Vec<Node> {
    let mut head = vec![Node::Title {
        text: match imports.pending.len() {
            0 => "No notes to keep".to_owned(),
            n => format!("{} to keep", plural(n, "note", "notes")),
        },
        subtitle: summary(imports),
    }];
    match &imports.progress {
        Some(progress) => {
            head.push(Node::Progress {
                share: f64::from(progress.fraction),
                label: Some(format!(
                    "{} · {:.0}%",
                    progress.line,
                    progress.fraction * 100.
                )),
            });
            head.push(Node::row([
                Node::text("New notes join this list as they come."),
                button("Stop", Kind::Secondary, &Do::Act { act: Act::Stop }),
            ]));
        }
        None if imports.any_found() => head.push(Node::row([
            Node::Spacer,
            button(
                super::banner::IMPORT,
                Kind::Secondary,
                &Do::Act {
                    act: Act::Import(Request::default()),
                },
            ),
        ])),
        None => {}
    }
    head
}

/// The tabs, with how many each holds, and the search.
fn tabs(imports: &Imports, on: &Tab) -> Node {
    let count =
        |tab: &Tab| imports.pending.iter().filter(|p| tab.holds(p)).count();
    let mut all = vec![Tab::All, Tab::You];
    let mut repos: Vec<String> = imports
        .pending
        .iter()
        .filter_map(|p| p.repo.clone())
        .collect();
    repos.sort();
    repos.dedup();
    all.extend(repos.into_iter().map(Tab::Repo));
    all.push(Tab::Look);
    let mut row: Vec<Node> = all
        .into_iter()
        .filter_map(|tab| {
            let n = count(&tab);
            // A repository's tab, and Need a look, only with something in.
            if n == 0 && matches!(tab, Tab::Repo(_) | Tab::Look) {
                return None;
            }
            let label = format!("{} {n}", tab.name());
            Some(if &tab == on {
                Node::Pill {
                    text: label,
                    tone: if tab == Tab::Look {
                        Tone::Warn
                    } else {
                        Tone::Info
                    },
                }
            } else {
                Node::Link {
                    label,
                    to: tab.link(),
                }
            })
        })
        .collect();
    row.extend([
        Node::Spacer,
        icon("search", Tone::Neutral),
        Node::Input {
            id: REVIEW_SEARCH.into(),
            placeholder: Some("Search".into()),
            multiline: false,
            submit: None,
            keep: true,
            secret: false,
            focus: None,
        },
    ]);
    Node::row(row)
}

/// The proposals in view, a heading and a table per scope.
fn list(shown: &[&Pending], imports: &Imports) -> Vec<Node> {
    if shown.is_empty() {
        let text = if imports.pending.is_empty() {
            if imports.progress.is_some() {
                "Nothing yet: proposals show here as they come."
            } else if imports.any_found() {
                "Nothing waits. Import to read what Claude Code and Codex kept."
            } else {
                "Nothing waits, and neither Claude Code nor Codex is on this computer."
            }
        } else {
            "Nothing here."
        };
        return vec![Node::Empty { text: text.into() }];
    }
    let mut sections: Vec<(Option<String>, Vec<&Pending>)> = Vec::new();
    for pending in shown {
        match sections.iter_mut().find(|(repo, _)| *repo == pending.repo) {
            Some((_, rows)) => rows.push(pending),
            None => sections.push((pending.repo.clone(), vec![pending])),
        }
    }
    // The person's first, then the repositories by name.
    sections
        .sort_by(|a, b| a.0.is_some().cmp(&b.0.is_some()).then(a.0.cmp(&b.0)));
    sections
        .into_iter()
        .flat_map(|(repo, rows)| {
            let mut heading = format!(
                "{} · {}",
                repo.as_deref().unwrap_or("About you"),
                rows.len()
            );
            if repo.is_none() {
                heading.push_str(" · saved for every repository");
            }
            [
                Node::Heading { text: heading },
                Node::Table {
                    head: Vec::new(),
                    rows: rows.into_iter().map(row).collect(),
                    widths: vec![5., 2.],
                },
            ]
        })
        .collect()
}

/// One proposal: its box, title, description and where it came from;
/// its type and flags.
fn row(pending: &Pending) -> Vec<Node> {
    let mut flags = vec![Node::Badge {
        text: pending.kind.as_str().into(),
        tone: kind_tone(pending.kind),
    }];
    if pending.in_both {
        let other = if pending
            .origins
            .first()
            .is_some_and(|o| o.starts_with(Tool::Codex.label()))
        {
            Tool::ClaudeCode
        } else {
            Tool::Codex
        };
        flags.push(Node::Badge {
            text: format!("also in {}", other.label()),
            tone: Tone::Info,
        });
    }
    if let Some(id) = &pending.flags.disagrees {
        flags.push(Node::Badge {
            text: format!("disagrees with {id}"),
            tone: Tone::Warn,
        });
    }
    if let Some(id) = &pending.flags.replaces {
        flags.push(Node::Badge {
            text: format!("replaces {id}"),
            tone: Tone::Warn,
        });
    }
    if pending.flags.secret {
        flags.push(Node::Badge {
            text: "held a secret".into(),
            tone: Tone::Bad,
        });
    }
    let from = match pending.origins.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => {
            format!("{first}, and {}", plural(rest.len(), "more", "more"))
        }
    };
    let detail = if from.is_empty() {
        pending.description.clone()
    } else {
        format!("{} · {from}", pending.description)
    };
    vec![
        Node::Switch {
            label: pending.title.clone(),
            detail: Some(detail),
            on: pending.keep,
            action: REVIEW.into(),
            args: Do::Act {
                act: Act::Tick {
                    id: pending.id.clone(),
                    keep: !pending.keep,
                },
            }
            .args(),
        },
        Node::row(flags),
    ]
}

/// How many are ticked, and the buttons that keep them or drop all.
fn foot(shown: &[&Pending]) -> Node {
    let ticked: Vec<String> = shown
        .iter()
        .filter(|p| p.keep)
        .map(|p| p.id.clone())
        .collect();
    let all: Vec<String> = shown.iter().map(|p| p.id.clone()).collect();
    let left = shown.len() - ticked.len();
    let label = match ticked.len() {
        1 => "Keep 1 note".to_owned(),
        n => format!("Keep {n} notes"),
    };
    let mut row = vec![
        Node::text(format!(
            "{} to keep · {} left for you to decide",
            ticked.len(),
            left
        )),
        Node::Spacer,
        button(
            "Keep none",
            Kind::Secondary,
            &Do::Act {
                act: Act::Drop { ids: all },
            },
        ),
    ];
    // With nothing ticked the button keeps nothing: it asks for nothing.
    row.push(if ticked.is_empty() {
        Node::Button {
            label,
            action: REVIEW.into(),
            args: Value::Null,
            kind: Kind::Secondary,
            icon: None,
        }
    } else {
        button(
            label,
            Kind::Primary,
            &Do::Act {
                act: Act::Keep { ids: ticked },
            },
        )
    });
    Node::row(row)
}

/// The files that did not read.
fn failed(imports: &Imports) -> Node {
    let mut children = vec![Node::Heading {
        text: format!(
            "Could not read {}",
            plural(imports.failed.len(), "file", "files")
        ),
    }];
    children.extend(
        imports
            .failed
            .iter()
            .map(|path| Node::Mono { text: path.clone() }),
    );
    Node::stack(children)
}
