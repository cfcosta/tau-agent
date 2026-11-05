//! tau-jev's UI half (ADR 0045): whether a TypeSafe key is saved, what
//! Jev did this session, and where the key goes: a section of the
//! Models screen and a page of its own, which other plugins link to.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_plugin::{
    Declaration,
    Link,
    Manifest,
    Page,
    UiPlugin,
    ViewCx,
    points::{self, AtApp},
    tree::{Kind, Node, Tone},
};

use crate::{NAME, PRICE_PER_MILLION_INPUT, SYSTEM_ONE_URL};

/// What tau-jev knows across repositories.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Data {
    /// A TypeSafe key is saved, so the plugins that ask Jev run.
    pub keyed: bool,
    /// What Jev did this session, once there is a key.
    pub stats: Option<Stats>,
}

/// What Jev did this session.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Stats {
    pub model: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub spent: f64,
    pub latency_p50_ms: u32,
    /// Requests that got no answer.
    pub failed: u32,
}

/// What the UI asks the host half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Act {
    /// Saves the TypeSafe key, or forgets it.
    Key { key: Option<String> },
}

/// Whether a TypeSafe key is saved, as another plugin's view reads it:
/// what its rows say when Jev cannot be asked.
pub fn keyed<P: UiPlugin>(view: &ViewCx<'_, P>) -> bool {
    view.data_of(NAME)
        .and_then(|data| data.get("keyed"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The page where the person saves the TypeSafe key, for another
/// plugin's button.
pub fn key_page() -> Link {
    Link::to(NAME, KEY_PAGE)
}

/// What a plugin that asks Jev says of itself on the Plugins screen:
/// `what` it does, and that it needs a TypeSafe key when there is none.
pub fn needs_jev(jev: bool, what: &str) -> String {
    if jev {
        format!("{what}, with Jev")
    } else {
        format!("{what}: needs a TypeSafe key (Models)")
    }
}

/// What a plugin that asks Jev says in a run's plugin list when there is
/// no TypeSafe key.
pub const NO_KEY: &str = "off · no TypeSafe key";

const KEY_PAGE: &str = "key";

/// The key input's Enter, and the Remove button.
const KEY: &str = "key";
const FORGET: &str = "forget";

/// tau-jev with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct JevUi;

impl UiPlugin for JevUi {
    type State = ();
    type Data = Data;
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Declaration {
        let host = SYSTEM_ONE_URL
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or_default();
        Declaration::new()
            .provides(&[crate::SERVICE])
            .network(&[host])
            .secrets(&["typesafe"])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::MODELS, section).page(
            Page::new(KEY_PAGE, page)
                .title(|_| "TypeSafe key".to_owned())
                .framed(),
        )
    }

    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        match action {
            KEY => {
                let key = args.get("text").and_then(Value::as_str);
                if let Some(key) = key.map(str::trim).filter(|k| !k.is_empty())
                {
                    view.handle.act(Act::Key {
                        key: Some(key.to_owned()),
                    });
                }
            }
            FORGET => view.handle.act(Act::Key { key: None }),
            _ => {}
        }
    }
}

/// The Models screen's section: whether a key is saved, with Remove, or
/// the field to save one.
fn section(_: &AtApp, view: &mut ViewCx<'_, JevUi>) -> Option<Node> {
    Some(Node::Card {
        children: key_card(view.data),
    })
}

/// The key's own page, which plugins that ask Jev link to.
fn page(view: &mut ViewCx<'_, JevUi>) -> Node {
    Node::stack([
        Node::text(
            "Jev, TypeSafe's model, picks efforts on auto, prunes the \
             context, checks each repository's rules and decides when a \
             /goal holds. The key stays in tau's config directory, readable \
             only by you.",
        ),
        Node::Card {
            children: key_card(view.data),
        },
    ])
}

fn key_card(data: &Data) -> Vec<Node> {
    let line = if data.keyed {
        "Saved · picks efforts on auto, prunes the context, checks each \
         repository's rules and /goal"
    } else {
        "None · without it, efforts on auto are the model's, and rules and \
         goals are not checked"
    };
    let mut head = vec![
        Node::Icon {
            name: "blocked".into(),
            tone: if data.keyed {
                Tone::Good
            } else {
                Tone::Neutral
            },
        },
        Node::Stack {
            children: vec![
                Node::Heading {
                    text: "TypeSafe (Jev)".into(),
                },
                Node::text(line),
            ],
        },
        Node::Spacer,
    ];
    if data.keyed {
        head.push(Node::Button {
            label: "Remove".into(),
            action: FORGET.into(),
            args: Value::Null,
            kind: Kind::Secondary,
            icon: None,
        });
    }
    let mut children = vec![Node::row(head)];
    match (&data.stats, data.keyed) {
        (Some(stats), true) => children.push(Node::Mono {
            text: stats_line(stats),
        }),
        (_, false) => children.push(Node::Input {
            id: "typesafe-key".into(),
            placeholder: Some("TypeSafe key, then Enter".into()),
            multiline: false,
            submit: Some(KEY.into()),
            keep: false,
            secret: true,
            focus: None,
        }),
        _ => {}
    }
    children
}

/// What Jev did this session, in a line: `jev-1.13 · 3 requests · 1.2k
/// tokens · $0.0001 · p50 210 ms`.
pub fn stats_line(stats: &Stats) -> String {
    let mut line = format!(
        "{} · {} requests · {} input tokens · ${:.4} at ${PRICE_PER_MILLION_INPUT} / M · p50 {} ms",
        stats.model,
        stats.requests,
        stats.input_tokens,
        stats.spent,
        stats.latency_p50_ms,
    );
    if stats.failed > 0 {
        line.push_str(&format!(" · {} failed", stats.failed));
    }
    line
}
