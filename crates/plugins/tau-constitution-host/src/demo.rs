//! The rules tau-ui's demo shows, and its Constitution page's states.
//! The rules go in through the plugin's own [`Act`]s, so the real host
//! half keeps them.

use gpui::{App, Entity};
use serde_json::Value;
use tau_constitution::ui::{
    Act,
    Rules,
    page::{self, Tab},
};

/// What a person did on the demo's rules pages: tau-agent's rules, as
/// the mockups show them, and docbert's.
pub fn acts() -> Vec<Act> {
    let add = |repo: &str, text: &str, on: &[&str], review, block| Act::Add {
        repo: repo.into(),
        text: text.into(),
        on: on.iter().map(|place| (*place).to_owned()).collect(),
        review,
        block,
    };
    let holds = |repo: &str| Act::Settings {
        repo: repo.into(),
        blocks_unchecked: false,
        max_holds: 3,
    };
    vec![
        add(
            "tau-agent",
            "Never delete outside target/ or rewrite published history.",
            &["bash.command"],
            0.30,
            0.60,
        ),
        add(
            "tau-agent",
            "Library code returns errors. No unwrap or expect outside tests.",
            &["edit.newText", "write.content"],
            0.30,
            0.80,
        ),
        add(
            "tau-agent",
            "Every sqlx query uses the checked macros.",
            &["edit.newText", "write.content"],
            0.40,
            0.85,
        ),
        add(
            "tau-agent",
            "Comments explain why, not what the code does.",
            &["edit.newText"],
            0.50,
            0.90,
        ),
        add(
            "tau-agent",
            "No network calls from tests except the fake server.",
            &["write.content"],
            0.35,
            0.80,
        ),
        add(
            "tau-agent",
            "The final answer names the tests that ran and their result.",
            &["final answer"],
            0.40,
            0.75,
        ),
        holds("tau-agent"),
        add(
            "docbert",
            "Never rebuild the whole index to fix one document.",
            &["bash.command"],
            0.30,
            0.70,
        ),
        add(
            "docbert",
            "Search results keep their scores; never sort them away.",
            &["edit.newText"],
            0.40,
            0.85,
        ),
        add(
            "docbert",
            "The final answer names the tests that ran.",
            &["final answer"],
            0.40,
            0.75,
        ),
        holds("docbert"),
    ]
}

/// What the page shows when the stored rules cannot be read.
pub fn broken() -> Rules {
    Rules {
        error: Some(
            "The constitution stored for tau-agent is not valid: Rule R4: \
             review (0.95) is above block (0.9)"
                .into(),
        ),
        ..Rules::default()
    }
}

const RULE: &str = "Never run migrations against the production database.";

/// The rules page of `repo` in the state `name` names: a rule being
/// written and tried on `calls`, one saved without where it applies,
/// or the review tab. False for a state it does not know.
pub fn open(
    name: &str,
    ui: &Entity<page::Ui>,
    repo: &str,
    calls: Vec<(String, Value)>,
    cx: &mut App,
) -> bool {
    match name {
        "rule-editor" => ui.update(cx, |ui, cx| {
            ui.open_editor(repo, None, cx);
            ui.set_rule_text(RULE, cx);
            ui.toggle_place("bash.command", cx);
            ui.try_rule(calls, Vec::new(), cx);
        }),
        "rule-missing" => ui.update(cx, |ui, cx| {
            ui.open_editor(repo, None, cx);
            ui.set_rule_text(RULE, cx);
            ui.save(cx);
        }),
        "rules-review" => ui.update(cx, |ui, cx| ui.set_tab(Tab::Review, cx)),
        _ => return false,
    }
    true
}
