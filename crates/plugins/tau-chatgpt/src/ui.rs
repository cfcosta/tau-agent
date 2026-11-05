//! tau-chatgpt's UI half (ADR 0045): the first step of setup, which
//! connects tau to the person's ChatGPT plan, and the Models screen's
//! accounts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_plugin::{
    Declaration,
    Manifest,
    UiPlugin,
    ViewCx,
    points::{self, AtApp, AtSetup, LinkState, SetupAt, SetupLink, SetupView},
    provider::PROVIDER,
    tree::{Kind, Node, Tone},
};

use crate::NAME;

/// Where ChatGPT says what usage counts toward the plan.
pub const USAGE_SETTINGS_URL: &str = tau_ai::chatgpt::USAGE_SETTINGS_URL;

/// Where OpenAI says who may use their plan in apps.
pub const PLAN_HELP_URL: &str = "https://help.openai.com";

/// What tau-chatgpt knows across repositories, from its host half.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Data {
    pub sign_in: SignIn,
    /// The saved sign-ins, by label.
    pub accounts: Vec<Account>,
}

/// How signing in stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SignIn {
    /// Nothing to run on yet.
    #[default]
    None,
    /// A sign-in is open in the browser at `url`; its redirect can also
    /// be pasted.
    Waiting {
        url: String,
    },
    /// Runs use `account`'s plan.
    Connected {
        account: String,
    },
    /// Signed in as `account`, without plan usage.
    PlanDisabled {
        account: String,
    },
    /// Signed in, but OpenAI says plan use is not available to this
    /// account or workspace (`Recovery::Restricted`); `detail` is the
    /// refusal as OpenAI sent it.
    NotEligible {
        account: String,
        detail: String,
    },
    Failed {
        error: String,
    },
}

/// A saved sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub label: String,
    pub state: AccountState,
    /// The account tau signs in with.
    pub active: bool,
}

/// A saved sign-in's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountState {
    Plan,
    PlanDisabled,
    SignedOut,
}

/// What the UI asks the host half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Act {
    /// Signs in in the browser: `account` again, or a new sign-in;
    /// `consent` asks again for plan usage.
    SignIn {
        account: Option<String>,
        consent: bool,
    },
    /// Finishes the sign-in waiting with the redirect pasted.
    Callback { url: String },
    /// Stops the sign-in waiting.
    Cancel,
    /// Makes `account` the one tau signs in with.
    Switch { account: String },
    /// Signs the active account out.
    SignOut,
}

/// What a click asks: the host half, or a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "click", rename_all = "snake_case")]
enum Do {
    Act { act: Act },
    Open { url: String },
    Continue,
}

/// The name of its setup step.
pub const STEP: &str = "sign-in";

/// Every button's action; its arguments say what it does ([`Do`]).
const CLICK: &str = "click";
/// The pasted redirect's Enter.
const PASTE: &str = "paste";

/// tau-chatgpt with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatGptUi;

impl UiPlugin for ChatGptUi {
    type State = ();
    type Data = Data;
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Declaration {
        Declaration::new()
            .fills(&[PROVIDER.name()])
            .network(&["chatgpt.com", "auth.openai.com"])
            .secrets(&["chatgpt"])
            .computer_only()
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::SETUP, step)
            .contribute(points::MODELS, section)
    }

    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        let handle = &view.handle;
        match action {
            PASTE => {
                let url = args.get("text").and_then(Value::as_str);
                if let Some(url) = url.map(str::trim).filter(|u| !u.is_empty())
                {
                    handle.act(Act::Callback {
                        url: url.to_owned(),
                    });
                }
            }
            CLICK => match serde_json::from_value::<Do>(args.clone()) {
                Ok(Do::Act { act }) => handle.act(act),
                Ok(Do::Open { url }) => handle.open_url(url),
                Ok(Do::Continue) => handle.setup_done(),
                Err(_) => {}
            },
            _ => {}
        }
    }
}

fn button(label: &str, kind: Kind, what: Do) -> Node {
    Node::Button {
        label: label.into(),
        action: CLICK.into(),
        args: serde_json::to_value(what).unwrap_or_default(),
        kind,
        icon: None,
    }
}

fn act(label: &str, kind: Kind, act: Act) -> Node {
    button(label, kind, Do::Act { act })
}

fn sign_in(label: &str, kind: Kind, consent: bool) -> Node {
    act(
        label,
        kind,
        Act::SignIn {
            account: None,
            consent,
        },
    )
}

fn note(text: &str) -> Node {
    Node::Toned {
        text: text.into(),
        tone: Tone::Neutral,
        mono: false,
    }
}

fn link(
    state: LinkState,
    above: Option<&str>,
    below: &str,
) -> Option<SetupLink> {
    Some(SetupLink {
        state,
        above: above.map(str::to_owned),
        below: Some(below.to_owned()),
    })
}

/// Setup's first step: connecting tau to the person's plan.
fn step(_: &AtSetup, view: &mut ViewCx<'_, ChatGptUi>) -> Option<SetupView> {
    let (heading, detail, link, body) = match &view.data.sign_in {
        SignIn::None | SignIn::Failed { .. } => {
            let mut body = vec![
                Node::List {
                    items: [
                        "Sign in to ChatGPT",
                        "Pick a workspace",
                        "Keep the name “tau”",
                        "Allow plan use",
                    ]
                    .into_iter()
                    .map(Node::text)
                    .collect(),
                },
                sign_in("Continue with ChatGPT", Kind::Primary, false),
                note(
                    "Needs ChatGPT Plus or Pro · tau can't read your conversations",
                ),
            ];
            if let SignIn::Failed { error } = &view.data.sign_in {
                body.insert(
                    1,
                    Node::Toned {
                        text: error.clone(),
                        tone: Tone::Bad,
                        mono: false,
                    },
                );
            }
            (
                "Connect tau to ChatGPT",
                "Runs use your ChatGPT plan through OpenAI's Responses API. One \
                 sign-in in the browser; nothing to paste, no key to keep.",
                link(LinkState::Idle, Some("plan usage"), "not connected"),
                body,
            )
        }
        SignIn::Waiting { url } => (
            "Finish in your browser",
            "A ChatGPT page just opened. Allow plan use there and this screen \
             moves on by itself.",
            link(
                LinkState::Waiting,
                Some("plan usage"),
                "waiting for the browser",
            ),
            vec![
                Node::row([
                    button(
                        "Open the page again",
                        Kind::Secondary,
                        Do::Open { url: url.clone() },
                    ),
                    act("Cancel", Kind::Secondary, Act::Cancel),
                ]),
                Node::text(
                    "Signing in on another device? Paste the address the \
                     browser ended on.",
                ),
                Node::Input {
                    id: "chatgpt-callback".into(),
                    placeholder: Some(
                        "http://localhost:1455/auth/callback?…".into(),
                    ),
                    multiline: false,
                    submit: Some(PASTE.into()),
                    keep: false,
                    secret: true,
                    focus: None,
                },
                note(
                    "listening on this machine · nothing is saved until the \
                     browser comes back",
                ),
            ],
        ),
        SignIn::Connected { account } => (
            "You're using your ChatGPT plan",
            "Eligible usage in tau now counts toward your plan. Pick the model \
             runs start with on the Models screen; you can change it per run.",
            link(LinkState::Connected, None, "connected"),
            vec![
                Node::Status {
                    text: account.clone(),
                    detail: Some("plan use allowed".into()),
                    tone: Tone::Good,
                },
                Node::row([
                    button("Continue", Kind::Primary, Do::Continue),
                    button(
                        "Manage usage",
                        Kind::Secondary,
                        Do::Open {
                            url: USAGE_SETTINGS_URL.into(),
                        },
                    ),
                ]),
            ],
        ),
        SignIn::PlanDisabled { account } => (
            "Allow tau to use your plan",
            "You're signed in, but plan use wasn't allowed. tau runs only on \
             your ChatGPT plan, so it can't start a run without it.",
            link(LinkState::Declined, None, "plan use off"),
            vec![
                Node::Status {
                    text: account.clone(),
                    detail: Some("plan use off".into()),
                    tone: Tone::Warn,
                },
                Node::row([
                    sign_in("Enable ChatGPT plan use", Kind::Primary, true),
                    sign_in("Use another account", Kind::Secondary, false),
                ]),
                note(
                    "Opens the same ChatGPT page, asking only for plan use · \
                     take it back any time in ChatGPT settings",
                ),
            ],
        ),
        SignIn::NotEligible { account, detail } => (
            "This account can't share its plan",
            "OpenAI says ChatGPT plan use isn't available for this account or \
             workspace. Usually because:",
            link(LinkState::Blocked, None, "not eligible"),
            vec![
                Node::Status {
                    text: account.clone(),
                    detail: Some("not eligible".into()),
                    tone: Tone::Bad,
                },
                Node::List {
                    items: [
                        "The plan isn't Plus or Pro",
                        "A workspace admin turned app access off",
                        "Plan use isn't offered in your region yet",
                    ]
                    .into_iter()
                    .map(Node::text)
                    .collect(),
                },
                Node::row([
                    sign_in(
                        "Use another ChatGPT account",
                        Kind::Primary,
                        false,
                    ),
                    button(
                        "Learn more",
                        Kind::Secondary,
                        Do::Open {
                            url: PLAN_HELP_URL.into(),
                        },
                    ),
                ]),
                Node::Mono {
                    text: detail.clone(),
                },
            ],
        ),
    };
    Some(SetupView {
        step: STEP,
        at: SetupAt::Model,
        title: "Model".into(),
        heading: heading.into(),
        detail: detail.into(),
        body: Node::stack(body),
        required: true,
        link,
    })
}

/// The Models screen's section: the saved sign-ins, the active one
/// marked, and what can be done with each.
fn section(_: &AtApp, view: &mut ViewCx<'_, ChatGptUi>) -> Option<Node> {
    let data = &view.data;
    let active = data.accounts.iter().find(|account| account.active);
    let (line, tone) = match (&data.sign_in, active.map(|a| a.state)) {
        (SignIn::Connected { .. }, _) | (_, Some(AccountState::Plan)) => {
            ("Runs use your ChatGPT plan", Tone::Good)
        }
        (_, Some(AccountState::PlanDisabled)) => (
            "Signed in, but plan use isn't enabled · enable it to run tasks",
            Tone::Warn,
        ),
        (_, Some(AccountState::SignedOut)) => {
            ("Signed out · sign in again", Tone::Neutral)
        }
        _ => ("Not signed in", Tone::Neutral),
    };
    let add = sign_in(
        if data.accounts.is_empty() {
            "Continue with ChatGPT"
        } else {
            "Add account"
        },
        Kind::Secondary,
        false,
    );
    let mut children = vec![Node::row([
        Node::Icon {
            name: "chat".into(),
            tone,
        },
        Node::Stack {
            children: vec![
                Node::Heading {
                    text: "ChatGPT".into(),
                },
                Node::text(line),
            ],
        },
        Node::Spacer,
        add,
    ])];
    for account in &data.accounts {
        let state = match account.state {
            AccountState::Plan => "Plan use enabled",
            AccountState::PlanDisabled => "Plan use not enabled",
            AccountState::SignedOut => "Signed out",
        };
        let pick = if account.active {
            Node::Badge {
                text: "Active".into(),
                tone: Tone::Good,
            }
        } else {
            act(
                "Switch",
                Kind::Secondary,
                Act::Switch {
                    account: account.id.clone(),
                },
            )
        };
        children.push(Node::row([
            Node::Stack {
                children: vec![Node::text(account.label.clone()), note(state)],
            },
            Node::Spacer,
            pick,
        ]));
    }
    if let Some(active) = active {
        let main = match active.state {
            AccountState::Plan => button(
                "Manage usage",
                Kind::Secondary,
                Do::Open {
                    url: USAGE_SETTINGS_URL.into(),
                },
            ),
            AccountState::PlanDisabled => {
                sign_in("Enable ChatGPT plan use", Kind::Primary, true)
            }
            AccountState::SignedOut => {
                sign_in("Sign in again", Kind::Primary, false)
            }
        };
        children.push(Node::row([
            main,
            act("Sign out", Kind::Secondary, Act::SignOut),
        ]));
    }
    Some(Node::Card { children })
}
