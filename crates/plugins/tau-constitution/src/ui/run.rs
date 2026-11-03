//! What the checks draw on a run: the plugin's line in its list, its
//! notes, what it decided on each card, and its inspector section. Each
//! is a view tree (ADR 0045).

use gpui::AnyElement;
use tau_ui_kit::{format::usd, theme};
use tau_ui_plugin::{
    Link,
    PluginStatus,
    ViewCx,
    points::{AtAnchor, AtCard, AtRun},
    tree::{KeyValue, Node, Tone},
};

use super::{CallVerdict, ConstitutionUi, Stats};
use crate::{NAME, VerdictKind};

/// The rule `rule` on its repository's page.
fn rule_link(repo: &str, rule: &str) -> Link {
    Link::page("rules")
        .param("repo", repo.to_owned())
        .param("rule", rule.to_owned())
}

/// A note's tone, as a tree has it.
fn tone(tone: theme::Tone) -> Tone {
    match tone {
        theme::Tone::Info => Tone::Info,
        theme::Tone::Warn => Tone::Warn,
        theme::Tone::Danger => Tone::Bad,
        theme::Tone::Good => Tone::Good,
        theme::Tone::Quiet => Tone::Neutral,
    }
}

/// Trees on a run draw nothing of their own to click.
fn no_actions(_: &str, _: &serde_json::Value, _: &mut gpui::App) {}

pub fn status(
    _: &AtRun,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<PluginStatus> {
    let (state, tone) = view.state?.status()?;
    Some(PluginStatus {
        name: NAME.into(),
        state,
        tone,
    })
}

/// One of its notes: a check Jev could not answer, an answer held or
/// flagged.
pub fn note(
    at: &AtAnchor,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<AnyElement> {
    let note = view.state?.notes.get(&at.key)?.clone();
    let tree = note_tree(&note.text, &note.detail, tone(note.tone));
    let id = format!("rules-{}-{}", at.run.id.0, at.key);
    Some(view.tree(&id, &tree, no_actions))
}

/// A note's tree: the plugin, what it says and where.
pub fn note_tree(text: &str, detail: &str, tone: Tone) -> Node {
    Node::Card {
        children: vec![Node::row([
            Node::Icon {
                name: "blocked".into(),
                tone,
            },
            Node::Mono { text: NAME.into() },
            Node::text(text),
            Node::Spacer,
            Node::Mono {
                text: detail.to_owned(),
            },
        ])],
    }
}

/// Beside a checked call's title: the rule that flagged it and its
/// score, with a way to review it; or, quietly, the scores that passed.
pub fn badge(
    at: &AtCard,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<AnyElement> {
    let call = view.state?.calls.get(at.keys.first()?)?.clone();
    let tree = badge_tree(
        &at.run.repo,
        call.verdict.as_ref(),
        &call.scores,
        view.compact,
    )?;
    let id = format!("rules-badge-{}", at.call_id);
    Some(view.tree(&id, &tree, no_actions))
}

/// The badge's tree, for a call's verdict and scores in `repo`.
pub fn badge_tree(
    repo: &str,
    verdict: Option<&CallVerdict>,
    scores: &[(String, f64)],
    compact: bool,
) -> Option<Node> {
    match verdict {
        Some(verdict) if verdict.kind == VerdictKind::Flagged => {
            Some(Node::row([
                Node::Mono {
                    text: format!("{} {:.2}", verdict.rule, verdict.score),
                },
                Node::Link {
                    label: "Review".into(),
                    to: rule_link(repo, &verdict.rule),
                },
            ]))
        }
        Some(_) => None,
        None if !compact && !scores.is_empty() => {
            let scores: Vec<String> = scores
                .iter()
                .map(|(rule, score)| format!("{rule} {score:.2}"))
                .collect();
            Some(Node::Mono {
                text: scores.join(" · "),
            })
        }
        None => None,
    }
}

/// Under a blocked call: how sure Jev was against the rule's marks, and
/// the rule.
pub fn blocked(
    at: &AtCard,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<AnyElement> {
    let call = view.state?.calls.get(at.keys.first()?)?.clone();
    let verdict = call
        .verdict
        .filter(|verdict| verdict.kind == VerdictKind::Blocked)?;
    let marks = view
        .repo(&at.run.repo)
        .and_then(|rules| rules.rule(&verdict.rule))
        .map(|rule| (rule.review, rule.block));
    let tree = blocked_tree(&at.run.repo, &verdict, marks);
    let id = format!("rules-blocked-{}", at.call_id);
    Some(view.tree(&id, &tree, no_actions))
}

/// The blocked call's tree: the rule, and Jev's score against its
/// review and block marks, when known.
pub fn blocked_tree(
    repo: &str,
    verdict: &CallVerdict,
    marks: Option<(f64, f64)>,
) -> Node {
    let p = verdict.score;
    let mut meter = format!("p(violation) {p:.2}");
    if let Some((review, block)) = marks {
        meter.push_str(&format!(" · review {review:.2} · block {block:.2}"));
    }
    Node::row([
        Node::Badge {
            text: NAME.into(),
            tone: Tone::Bad,
        },
        Node::Link {
            label: format!("Rule {}", verdict.rule),
            to: rule_link(repo, &verdict.rule),
        },
        Node::Spacer,
        Node::Progress {
            share: p,
            label: Some(meter),
        },
    ])
}

/// The inspector's section: what the checks did, and a flagged call to
/// review once the run is done.
pub fn inspector(
    at: &AtRun,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<AnyElement> {
    let stats = view.state?.stats.clone();
    let tree = inspector_tree(&stats, &at.run.repo, at.run.live)?;
    let id = format!("rules-inspector-{}", at.run.id.0);
    Some(view.tree(&id, &tree, no_actions))
}

/// The inspector's tree for a run's `stats`, in `repo`; nothing before
/// anything is checked.
pub fn inspector_tree(stats: &Stats, repo: &str, live: bool) -> Option<Node> {
    if stats.is_empty() {
        return None;
    }
    let with_rules = |list: &[String]| {
        if list.is_empty() {
            "0".to_owned()
        } else {
            format!("{} ({})", list.len(), list.join(", "))
        }
    };
    let row = |key: &str, value: String| KeyValue {
        key: key.into(),
        value,
    };
    let held = match (stats.max_holds, stats.held.is_empty()) {
        (Some(max), false) => {
            format!("{} of {max} ({})", stats.held.len(), stats.held.join(", "))
        }
        _ => with_rules(&stats.held),
    };
    let mut section = vec![
        Node::Heading {
            text: "Constitution checks".into(),
        },
        Node::Rows {
            rows: vec![
                row("calls checked", stats.calls.to_string()),
                row("answers checked", stats.answers.to_string()),
                row("questions asked", stats.questions.to_string()),
                row("blocked", with_rules(&stats.blocked)),
                row("flagged", with_rules(&stats.flagged)),
                row("held", held),
                row("Jev cost", usd(stats.cost)),
            ],
        },
        Node::text(
            "Only the fields the model wrote are checked, such as \
             edit.newText or exec_command.cmd. Tool output never goes to Jev.",
        ),
    ];
    if let Some(rule) = stats.flagged.first().filter(|_| !live) {
        section.push(Node::Link {
            label: "Review the flagged call".into(),
            to: rule_link(repo, rule),
        });
    }
    Some(Node::stack(section))
}
