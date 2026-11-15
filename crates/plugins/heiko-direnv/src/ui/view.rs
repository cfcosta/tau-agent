//! What heiko-direnv draws: the question that takes the composer's place,
//! the lines above a run's transcript while its environment loads or
//! after it failed, and the repository menu's toggle. Each is a view
//! tree (ADR 0045): the loading card's clock is a ticker the interface
//! keeps, from when the load started.

use heiko_ui_plugin::{
    ViewCx,
    points::{AtRepo, AtRun},
    tree::{ButtonSize, Ink, Kind, MarkState, Node, SquareLook, Tone},
};
use serde_json::Value;

use super::DirenvUi;
use crate::{Act, Record};

/// Every button's and segment's action: carry out the [`Act`] its
/// arguments hold.
const ACT: &str = "act";

/// A button that asks the host half for `act`.
fn button(label: &str, kind: Kind, act: &Act) -> Node {
    Node::Button {
        label: label.into(),
        action: ACT.into(),
        args: args(act),
        kind,
        icon: None,
        size: Default::default(),
        wide: false,
        detail: None,
    }
}

fn args(act: &Act) -> Value {
    serde_json::to_value(act).unwrap_or(Value::Null)
}

/// What every tree's buttons and segments ask: the act in their
/// arguments.
pub fn on_action(action: &str, args: &Value, view: &mut ViewCx<'_, DirenvUi>) {
    if action != ACT {
        return;
    }
    if let Ok(act) = serde_json::from_value::<Act>(args.clone()) {
        view.handle.act(act);
    }
}

/// The question, in the composer's place, while a live run waits for
/// the person to say whether the repository's `.envrc` loads.
pub fn question(at: &AtRun, view: &mut ViewCx<'_, DirenvUi>) -> Option<Node> {
    if !at.run.live {
        return None;
    }
    let Some(Record::Asked { repo, envrc }) = view.state?.now.clone() else {
        return None;
    };
    Some(question_tree(&repo, &envrc))
}

/// The question's tree: its head, the `.envrc` as direnv runs it (with
/// bash) on a flat band under the file's name, and the two answers:
/// loading it is the block, running without it a link.
pub fn question_tree(repo: &str, envrc: &str) -> Node {
    let decide = |load| Act::Decide {
        repo: repo.to_owned(),
        load,
    };
    Node::stack([
        Node::Header {
            title: format!("Load {repo}'s .envrc?"),
            meta: Some("heiko-direnv · asked once".into()),
            actions: vec![],
        },
        Node::stack([
            Node::row([Node::Stamp {
                label: ".envrc".into(),
                tone: Ink::Neutral,
            }]),
            Node::Code {
                lang: Some("bash".into()),
                text: envrc.trim_end().to_owned(),
                first_line: None,
            },
        ]),
        Node::row([
            Node::Button {
                label: "Load it".into(),
                action: ACT.into(),
                args: args(&decide(true)),
                kind: Kind::Primary,
                icon: None,
                size: ButtonSize::Medium,
                wide: false,
                detail: None,
            },
            button("run without it", Kind::Secondary, &decide(false)),
        ]),
    ])
}

/// Above a run's transcript: its environment loading, while it runs, or
/// why it did not load.
pub fn card(at: &AtRun, view: &mut ViewCx<'_, DirenvUi>) -> Option<Node> {
    let now = view.state?.now.clone()?;
    let run = at.run.id.0.to_string();
    card_tree(&now, at.run.live, &run)
}

/// The tree for where `run`'s environment stands: loading, while the run
/// is live, with a clock from when it started, or failed, with what
/// direnv said and a way to try again; nothing otherwise.
pub fn card_tree(now: &Record, live: bool, run: &str) -> Option<Node> {
    let mark = |state| Node::Mark { state, count: None };
    match now {
        Record::Loading { since } if live => Some(Node::row([
            mark(MarkState::Working),
            Node::text("Loading .envrc"),
            Node::Ticker {
                id: format!("envrc-loading-{run}"),
                since: i64::try_from(*since).unwrap_or(i64::MAX),
                until: None,
            },
            Node::Spacer,
            Node::Toned {
                text: "commands wait for it".into(),
                tone: Tone::Neutral,
                mono: false,
            },
        ])),
        Record::Failed { status, output } => Some(Node::Card {
            children: vec![
                Node::row([
                    mark(MarkState::Failing),
                    Node::Rich {
                        text: "**.envrc did not load**".into(),
                    },
                    Node::Badge {
                        text: status.clone(),
                        tone: Tone::Bad,
                    },
                    Node::Spacer,
                    button(
                        "try again",
                        Kind::Secondary,
                        &Act::Reload {
                            run: run.to_owned(),
                        },
                    ),
                ]),
                Node::Code {
                    lang: None,
                    text: output.clone(),
                    first_line: None,
                },
                Node::Toned {
                    text: "commands run without it".into(),
                    tone: Tone::Neutral,
                    mono: false,
                },
            ],
        }),
        _ => None,
    }
}

/// The repository menu's entry, for a repository with an `.envrc`: the
/// toggle, or why there is none.
pub fn menu_entry(
    at: &AtRepo,
    view: &mut ViewCx<'_, DirenvUi>,
) -> Option<Node> {
    let data = view.repo(&at.repo)?.clone();
    if !data.envrc {
        return None;
    }
    let on = view.settings.repos.get(&at.repo) == Some(&true);
    Some(menu_tree(&at.repo, data.direnv, on))
}

/// The menu entry's tree: the setting's two segments, or, without
/// direnv, why there is none.
pub fn menu_tree(repo: &str, direnv: bool, on: bool) -> Node {
    const LABEL: &str = "Load .envrc for agent commands";
    if !direnv {
        return Node::stack([
            Node::text(LABEL),
            Node::Toned {
                text: "direnv is not installed".into(),
                tone: Tone::Neutral,
                mono: false,
            },
        ]);
    }
    setting(LABEL, None, repo, on)
}

/// Whether `repo`'s `.envrc` loads, as a row: `label` over `detail`,
/// then two segments, the one that holds outlined and the other a click
/// away.
fn setting(label: &str, detail: Option<&str>, repo: &str, on: bool) -> Node {
    let segment = |text: &str, load: bool| {
        let chosen = load == on;
        Node::Square {
            label: text.into(),
            style: if chosen {
                SquareLook::Outlined
            } else {
                SquareLook::Quiet
            },
            tone: if chosen { Ink::Info } else { Ink::Neutral },
            action: (!chosen).then(|| ACT.to_owned()),
            args: if chosen {
                Value::Null
            } else {
                args(&Act::Decide {
                    repo: repo.to_owned(),
                    load,
                })
            },
        }
    };
    let mut said = vec![Node::text(label)];
    said.extend(detail.map(|detail| Node::Toned {
        text: detail.into(),
        tone: Tone::Neutral,
        mono: false,
    }));
    Node::row([
        Node::Stack { children: said },
        Node::Spacer,
        segment("off", false),
        segment("on", true),
    ])
}

/// Its settings pane (ADR 0029): whether each repository's `.envrc`
/// loads for agent commands. The choice is a repository's own, so the
/// pane shows the repository in scope, or every one with an `.envrc`.
pub fn settings_pane(view: &mut ViewCx<'_, DirenvUi>) -> Node {
    let scope = view.scope().map(str::to_owned);
    let repos: Vec<(String, crate::RepoData, bool)> = view
        .repos()
        .filter(|(name, _)| scope.as_deref().is_none_or(|scope| scope == *name))
        .map(|(name, data)| {
            let on = view.settings.repos.get(name) == Some(&true);
            (name.to_owned(), data.clone(), on)
        })
        .collect();
    settings_tree(&repos, scope.is_some())
}

/// The pane's tree: a row for each repository with an `.envrc` (or the
/// one in scope, with or without), its segments when it can load.
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
                    Node::Toned {
                        text: why.into(),
                        tone: Tone::Neutral,
                        mono: false,
                    },
                ]),
                None => setting(
                    repo,
                    Some("Load its .envrc for agent commands."),
                    repo,
                    *on,
                ),
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
    Node::Stack { children }
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
    fn the_segment_not_chosen_asks_for_the_other_answer() {
        let tree = menu_tree("heiko", true, true);
        let segments: Vec<_> = pieces(&tree)
            .into_iter()
            .filter_map(|piece| match piece {
                Node::Square {
                    label,
                    style,
                    action,
                    args,
                    ..
                } => Some((label.as_str(), *style, action.clone(), args)),
                _ => None,
            })
            .collect();
        let off = args(&Act::Decide {
            repo: "heiko".into(),
            load: false,
        });
        assert_eq!(
            segments,
            [
                ("off", SquareLook::Quiet, Some(ACT.to_owned()), &off),
                ("on", SquareLook::Outlined, None, &Value::Null),
            ]
        );
    }
}
