//! The rules tau-ui's demo shows, and its Constitution page's states.
//! The rules go in through the plugin's own [`Act`]s, so the real host
//! half keeps them.

use gpui::App;
use serde_json::{Value, json};
use tau_constitution::ui::{
    Act,
    Rules,
    page::{self, Do, Snapshot},
};
use tau_ui_plugin::{Handle, Request, tree};

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
            &["exec_command.cmd"],
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
            &["exec_command.cmd"],
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

/// What the page's UI state holds once `request` is carried out, as far
/// as its clicks read it.
fn fold(now: &mut Snapshot, request: &Request) {
    let Request::Ui { key, value } = request else {
        return;
    };
    let text = || {
        value
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    if key == page::EDITOR {
        now.draft = value
            .as_ref()
            .and_then(|value| serde_json::from_value(value.clone()).ok());
    } else if *key == tree::draft_key(page::RULE_TEXT) {
        now.text = text();
    } else if *key == tree::draft_key(page::RULE_ON) {
        now.on = text();
    }
}

/// Asks the interface for `request`, as the page holds it in `now`.
fn ask(handle: &Handle, now: &mut Snapshot, request: Request, cx: &mut App) {
    fold(now, &request);
    handle.request(request, cx);
}

/// Clicks `doing` on the page as `now` holds it.
fn click(handle: &Handle, now: &mut Snapshot, doing: Do, cx: &mut App) {
    for request in page::apply(now, doing) {
        ask(handle, now, request, cx);
    }
}

/// Writes a new rule for `repo` in the editor: [`RULE`], as typed.
fn write_rule(handle: &Handle, now: &mut Snapshot, repo: &str, cx: &mut App) {
    let open = Do::Open {
        repo: repo.to_owned(),
        rule: None,
    };
    click(handle, now, open, cx);
    let typed = Request::Ui {
        key: tree::draft_key(page::RULE_TEXT),
        value: Some(json!(RULE)),
    };
    ask(handle, now, typed, cx);
}

/// The rules page of `repo`, through `handle`, in the state `name`
/// names: a rule being written and tried on `calls`, one saved without
/// where it applies, or the review tab. False for a state it does not
/// know.
pub fn open(
    name: &str,
    handle: &Handle,
    repo: &str,
    calls: Vec<(String, Value)>,
    cx: &mut App,
) -> bool {
    let mut now = Snapshot::default();
    match name {
        "rule-editor" => {
            write_rule(handle, &mut now, repo, cx);
            let place = Do::Place {
                place: "exec_command.cmd".into(),
            };
            click(handle, &mut now, place, cx);
            let tried = Do::Try {
                calls,
                answers: Vec::new(),
            };
            click(handle, &mut now, tried, cx);
        }
        "rule-missing" => {
            write_rule(handle, &mut now, repo, cx);
            click(handle, &mut now, Do::Save { typed: None }, cx);
        }
        // The second tab: what waits for review.
        "rules-review" => {
            handle.set_ui(tree::tab_key(page::TABS), Some(json!(1)), cx);
        }
        _ => return false,
    }
    true
}
