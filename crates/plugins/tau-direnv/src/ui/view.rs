//! What tau-direnv draws: the question that takes the composer's place,
//! the cards above a run's transcript while its environment loads or
//! after it failed, and the repository menu's toggle. Each is a view
//! tree (ADR 0045): the loading card's clock is a ticker the interface
//! keeps, from when the load started.

use gpui::AnyElement;
use serde_json::Value;
use tau_ui_plugin::{
    Handle,
    ViewCx,
    points::{AtRepo, AtRun},
    tree::{Kind, Node, Tone},
};

use super::DirenvUi;
use crate::{Act, Record};

/// Every button's and switch's action: carry out the [`Act`] its
/// arguments hold.
const ACT: &str = "act";

/// An icon of tau's, by name.
fn icon(name: &str, tone: Tone) -> Node {
    Node::Icon {
        name: name.into(),
        tone,
    }
}

/// A button that asks the host half for `act`.
fn button(label: &str, kind: Kind, act: &Act) -> Node {
    Node::Button {
        label: label.into(),
        action: ACT.into(),
        args: args(act),
        kind,
        icon: None,
    }
}

fn args(act: &Act) -> Value {
    serde_json::to_value(act).unwrap_or(Value::Null)
}

/// What a tree's buttons and switches ask: the act in their arguments.
fn on_act(handle: Handle) -> impl Fn(&str, &Value, &mut gpui::App) + 'static {
    move |action, args, cx| {
        if action != ACT {
            return;
        }
        if let Ok(act) = serde_json::from_value::<Act>(args.clone()) {
            handle.act(act, cx);
        }
    }
}

/// The question, in the composer's place, while a live run waits for
/// the person to say whether the repository's `.envrc` loads.
pub fn question(
    at: &AtRun,
    view: &mut ViewCx<'_, DirenvUi>,
) -> Option<AnyElement> {
    if !at.run.live {
        return None;
    }
    let Some(Record::Asked { repo, envrc }) = view.state?.now.clone() else {
        return None;
    };
    let tree = question_tree(&repo, &envrc);
    let handle = view.handle.clone();
    Some(view.tree("envrc-question", &tree, on_act(handle)))
}

/// The question's tree: what loads, the `.envrc` as direnv runs it (with
/// bash), and the two answers.
pub fn question_tree(repo: &str, envrc: &str) -> Node {
    let decide = |load| Act::Decide {
        repo: repo.to_owned(),
        load,
    };
    Node::Card {
        children: vec![Node::stack([
            Node::row([
                icon("terminal", Tone::Info),
                Node::Heading {
                    text: format!("Load {repo}'s .envrc for agent commands?"),
                },
            ]),
            Node::text(
                "The repository sets up its tools with direnv. With it, \
                 commands run in that environment in main and in every \
                 chat, as they would in your shell. tau asks once; you \
                 can change it on the repository's menu.",
            ),
            Node::Code {
                lang: Some("bash".into()),
                text: envrc.trim_end().to_owned(),
            },
            Node::row([
                button("Load it", Kind::Primary, &decide(true)),
                button("Run without it", Kind::Secondary, &decide(false)),
            ]),
        ])],
    }
}

/// Above a run's transcript: its environment loading, while it runs, or
/// why it did not load.
pub fn card(at: &AtRun, view: &mut ViewCx<'_, DirenvUi>) -> Option<AnyElement> {
    let now = view.state?.now.clone()?;
    let run = at.run.id.0.to_string();
    let tree = card_tree(&now, at.run.live, &run)?;
    let handle = view.handle.clone();
    Some(view.tree(&format!("envrc-card-{run}"), &tree, on_act(handle)))
}

/// The card's tree for where `run`'s environment stands: loading, while
/// the run is live, with a clock from when it started, or failed, with
/// what direnv said and a way to try again; nothing otherwise.
pub fn card_tree(now: &Record, live: bool, run: &str) -> Option<Node> {
    match now {
        Record::Loading { since } if live => Some(Node::Card {
            children: vec![Node::row([
                icon("clock", Tone::Info),
                Node::text("Loading the repository's environment · .envrc ·"),
                Node::Ticker {
                    id: format!("envrc-loading-{run}"),
                    since: i64::try_from(*since).unwrap_or(i64::MAX),
                    until: None,
                },
                Node::Spacer,
                Node::text("commands wait for it"),
            ])],
        }),
        Record::Failed { status, output } => Some(Node::Card {
            children: vec![Node::stack([
                Node::row([
                    icon("warning", Tone::Bad),
                    Node::Heading {
                        text: "The repository's environment did not load"
                            .into(),
                    },
                    Node::Spacer,
                    Node::Badge {
                        text: status.clone(),
                        tone: Tone::Bad,
                    },
                ]),
                Node::Code {
                    lang: None,
                    text: output.clone(),
                },
                Node::row([
                    Node::text("Commands run without it until it loads."),
                    Node::Spacer,
                    button(
                        "Try again",
                        Kind::Secondary,
                        &Act::Reload {
                            run: run.to_owned(),
                        },
                    ),
                ]),
            ])],
        }),
        _ => None,
    }
}

/// The repository menu's entry, for a repository with an `.envrc`: the
/// toggle, or why there is none.
pub fn menu_entry(
    at: &AtRepo,
    view: &mut ViewCx<'_, DirenvUi>,
) -> Option<AnyElement> {
    let data = view.repo(&at.repo)?.clone();
    if !data.envrc {
        return None;
    }
    let on = view.settings.repos.get(&at.repo) == Some(&true);
    let tree = menu_tree(&at.repo, data.direnv, on);
    let handle = view.handle.clone();
    Some(view.tree(&format!("envrc-menu-{}", at.repo), &tree, on_act(handle)))
}

/// The menu entry's tree: the switch, or, without direnv, why there is
/// none.
pub fn menu_tree(repo: &str, direnv: bool, on: bool) -> Node {
    const LABEL: &str = "Load .envrc for agent commands";
    if !direnv {
        return Node::row([
            icon("terminal", Tone::Neutral),
            Node::stack([
                Node::text(LABEL),
                Node::text("direnv is not installed"),
            ]),
        ]);
    }
    Node::Switch {
        label: LABEL.into(),
        detail: None,
        on,
        action: ACT.into(),
        args: args(&Act::Decide {
            repo: repo.to_owned(),
            load: !on,
        }),
    }
}

/// Its settings pane (ADR 0029): whether each repository's `.envrc`
/// loads for agent commands. The choice is a repository's own, so the
/// pane shows the repository in scope, or every one with an `.envrc`.
pub fn settings_pane(view: &mut ViewCx<'_, DirenvUi>) -> AnyElement {
    let scope = view.scope().map(str::to_owned);
    let repos: Vec<(String, crate::RepoData, bool)> = view
        .repos()
        .filter(|(name, _)| scope.as_deref().is_none_or(|scope| scope == *name))
        .map(|(name, data)| {
            let on = view.settings.repos.get(name) == Some(&true);
            (name.to_owned(), data.clone(), on)
        })
        .collect();
    let tree = settings_tree(&repos, scope.is_some());
    let handle = view.handle.clone();
    view.tree("envrc-settings", &tree, on_act(handle))
}

/// The pane's tree: a row for each repository with an `.envrc` (or the
/// one in scope, with or without), its switch when it can load.
pub fn settings_tree(
    repos: &[(String, crate::RepoData, bool)],
    scoped: bool,
) -> Node {
    let rows: Vec<Node> = repos
        .iter()
        .filter(|(_, data, _)| data.envrc || scoped)
        .map(|(repo, data, on)| {
            let why = if !data.envrc {
                Some("No .envrc in this repository.")
            } else if !data.direnv {
                Some("direnv is not installed.")
            } else {
                None
            };
            match why {
                Some(why) => Node::stack([
                    Node::Mono { text: repo.clone() },
                    Node::text(why),
                ]),
                None => Node::Switch {
                    label: repo.clone(),
                    detail: Some(
                        "Agent commands run in its .envrc's environment."
                            .into(),
                    ),
                    on: *on,
                    action: ACT.into(),
                    args: args(&Act::Decide {
                        repo: repo.clone(),
                        load: !on,
                    }),
                },
            }
        })
        .collect();
    if rows.is_empty() {
        return Node::Empty {
            text: "No repository has an .envrc.".into(),
        };
    }
    let mut children = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            children.push(Node::Rule);
        }
        children.push(row);
    }
    Node::Card { children }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every piece of `node`, itself first.
    fn pieces(node: &Node) -> Vec<&Node> {
        let mut all = vec![node];
        if let Node::Card { children }
        | Node::Stack { children }
        | Node::Row { children } = node
        {
            all.extend(children.iter().flat_map(pieces));
        }
        all
    }

    #[test]
    fn the_loading_card_counts_from_when_the_load_started() {
        let tree = card_tree(
            &Record::Loading {
                since: 1_700_000_000_000,
            },
            true,
            "r",
        )
        .unwrap();
        assert!(pieces(&tree).iter().any(|piece| matches!(
            piece,
            Node::Ticker {
                since: 1_700_000_000_000,
                until: None,
                ..
            }
        )));
        let ended = Record::Loading { since: 1 };
        assert_eq!(card_tree(&ended, false, "r"), None);
    }

    #[test]
    fn try_again_reloads_the_run() {
        let failed = Record::Failed {
            status: "exit 1".into(),
            output: "boom".into(),
        };
        let tree = card_tree(&failed, false, "run-1").unwrap();
        let reload = args(&Act::Reload {
            run: "run-1".into(),
        });
        assert!(pieces(&tree).iter().any(|piece| matches!(
            piece,
            Node::Button { action, args, .. } if action == ACT && *args == reload
        )));
    }

    #[test]
    fn the_switch_asks_for_the_other_answer() {
        let Node::Switch { on, args: got, .. } = menu_tree("tau", true, true)
        else {
            panic!("no switch");
        };
        assert!(on);
        let off = Act::Decide {
            repo: "tau".into(),
            load: false,
        };
        assert_eq!(got, args(&off));
    }
}
