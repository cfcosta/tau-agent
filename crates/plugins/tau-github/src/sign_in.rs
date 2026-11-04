//! The sign-in's trees: the setup step before the repositories, and the
//! Models screen's section. Both draw [`form`], which follows the sign-in
//! as the host half says it stands.

use tau_ui_plugin::{
    ViewCx,
    points::{AtApp, AtSetup, SetupAt, SetupView},
    tree::{Kind, Node, Tone},
};

use crate::{
    Act,
    DeviceCode,
    Do,
    GitHubUi,
    SIGN_IN,
    STEP,
    SignIn,
    TOKEN,
    TOKEN_FORM,
};

/// What the app and a fine-grained token need on each repository.
pub const PERMISSIONS: [(&str, &str); 3] = [
    ("Contents", "Read and write"),
    ("Pull requests", "Read and write"),
    ("Metadata", "Read-only"),
];

/// A button of the sign-in that asks for `what`.
fn button(label: &str, kind: Kind, what: &Do) -> Node {
    Node::Button {
        label: label.into(),
        action: SIGN_IN.into(),
        args: what.args(),
        kind,
        icon: None,
    }
}

fn act(label: &str, kind: Kind, act: Act) -> Node {
    button(label, kind, &Do::Act { act })
}

fn token_link() -> Node {
    button(
        "Use a personal access token",
        Kind::Secondary,
        &Do::TokenForm { open: true },
    )
}

fn permissions() -> Node {
    Node::Table {
        head: vec!["Permission".into(), "Level".into()],
        rows: PERMISSIONS
            .iter()
            .map(|(name, level)| vec![Node::text(*name), Node::text(*level)])
            .collect(),
        widths: Vec::new(),
    }
}

/// The code to enter, where, and the ways on.
fn waiting(code: &DeviceCode) -> Vec<Node> {
    vec![
        Node::Title {
            text: code.code.clone(),
            subtitle: Some(format!(
                "{} · code expires in {} · only on the repositories you pick",
                code.url, code.expires
            )),
        },
        Node::row([
            button(
                "Copy code and open GitHub",
                Kind::Primary,
                &Do::OpenDevice { code: code.clone() },
            ),
            act("I have approved it", Kind::Secondary, Act::Check),
            token_link(),
        ]),
        permissions(),
    ]
}

/// The token form: what the token needs, its input, and the way back.
fn token_form(sign_in: &SignIn) -> Vec<Node> {
    let mut children = vec![
        Node::text(
            "A fine-grained token works in place of the GitHub App. Give it \
             these repository permissions, then paste it here and press \
             Enter.",
        ),
        permissions(),
        Node::Input {
            id: "token".into(),
            placeholder: Some("github_pat_…".into()),
            multiline: false,
            submit: Some(TOKEN.into()),
            keep: false,
            secret: true,
            focus: None,
        },
    ];
    match sign_in {
        SignIn::Checking => children.push(Node::Status {
            text: "Checking the token…".into(),
            detail: None,
            tone: Tone::Info,
        }),
        SignIn::Failed { error } => children.push(Node::Toned {
            text: error.clone(),
            tone: Tone::Bad,
            mono: false,
        }),
        _ => {}
    }
    children.push(Node::row([button(
        "Back to the device code",
        Kind::Secondary,
        &Do::TokenForm { open: false },
    )]));
    children
}

/// The sign-in as it stands: a way to start it, the code to enter, the
/// token form, or who is signed in. `done` is the setup step's way on,
/// once signed in.
pub fn form(sign_in: &SignIn, token: bool, done: bool) -> Node {
    if token && !matches!(sign_in, SignIn::SignedIn { .. }) {
        return Node::stack(token_form(sign_in));
    }
    let children = match sign_in {
        SignIn::Unknown | SignIn::SignedOut => vec![
            Node::text(
                "GitHub shows a code; enter it on GitHub and approve tau's \
                 app for the repositories you want.",
            ),
            Node::row([
                act("Sign in with GitHub", Kind::Primary, Act::SignIn),
                token_link(),
            ]),
        ],
        SignIn::Starting => vec![Node::Status {
            text: "Asking GitHub for a code…".into(),
            detail: None,
            tone: Tone::Info,
        }],
        SignIn::Waiting { code } => waiting(code),
        SignIn::Checking => vec![Node::Status {
            text: "Checking…".into(),
            detail: None,
            tone: Tone::Info,
        }],
        SignIn::SignedIn { user } => {
            let mut buttons = Vec::new();
            if done {
                buttons.push(button("Continue", Kind::Primary, &Do::Continue));
            }
            buttons.push(button(
                "Give tau's GitHub App access to more repositories",
                Kind::Secondary,
                &Do::InstallApp,
            ));
            vec![
                Node::Status {
                    text: format!("Signed in as @{user}"),
                    detail: None,
                    tone: Tone::Good,
                },
                Node::row(buttons),
            ]
        }
        SignIn::Failed { error } => vec![
            Node::Toned {
                text: error.clone(),
                tone: Tone::Bad,
                mono: false,
            },
            Node::row([
                act("Try again", Kind::Primary, Act::SignIn),
                token_link(),
            ]),
        ],
    };
    Node::stack(children)
}

/// Whether the token form shows, as the window's UI state keeps it.
fn token_open(view: &ViewCx<'_, GitHubUi>) -> bool {
    view.ui_value(TOKEN_FORM)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// The step of first-run setup before the repositories, whose list comes
/// from the sign-in: none until a host says how the sign-in stands.
pub fn step(
    _at: &AtSetup,
    view: &mut ViewCx<'_, GitHubUi>,
) -> Option<SetupView> {
    let sign_in = &view.data.sign_in;
    let heading = match sign_in {
        SignIn::Unknown => return None,
        SignIn::SignedIn { .. } => "Signed in to GitHub",
        SignIn::Waiting { .. } => "Enter this code on GitHub",
        _ => "Sign in with GitHub",
    };
    Some(SetupView {
        step: STEP,
        at: SetupAt::BeforeRepositories,
        title: "GitHub".into(),
        heading: heading.into(),
        detail: "tau clones the repositories you pick next from GitHub, \
                 pushes to them and opens pull requests. The sign-in stays \
                 in tau's config directory, readable only by you."
            .into(),
        body: form(sign_in, token_open(view), true),
    })
}

/// The Models screen's section: who is signed in, with Sign out, or the
/// sign-in while signed out.
pub fn section(_at: &AtApp, view: &mut ViewCx<'_, GitHubUi>) -> Option<Node> {
    let sign_in = &view.data.sign_in;
    let (line, tone, head_button) = match sign_in {
        SignIn::Unknown => return None,
        SignIn::SignedIn { user } => (
            format!("Signed in as @{user} · clones repositories"),
            Tone::Good,
            Some(act("Sign out", Kind::Secondary, Act::SignOut)),
        ),
        SignIn::SignedOut => (
            "Not signed in · for cloning repositories".to_owned(),
            Tone::Neutral,
            None,
        ),
        _ => ("Signing in…".to_owned(), Tone::Info, None),
    };
    let mut head = vec![
        Node::Icon {
            name: "repo".into(),
            tone,
        },
        Node::Stack {
            children: vec![
                Node::Heading {
                    text: "GitHub".into(),
                },
                Node::text(line),
            ],
        },
        Node::Spacer,
    ];
    head.extend(head_button);
    let mut children = vec![Node::row(head)];
    if !matches!(sign_in, SignIn::SignedIn { .. }) {
        children.push(form(sign_in, token_open(view), false));
    }
    Some(Node::Card { children })
}
