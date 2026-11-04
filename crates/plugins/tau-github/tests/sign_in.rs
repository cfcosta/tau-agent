//! tau-github's sign-in: its setup step comes before the repositories
//! once a host says how the sign-in stands, its buttons ask the host
//! half for what they say, the token's Enter sends the token, and a
//! saved sign-in ends the step. The Models screen's section shows who is
//! signed in.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_github::{
    Act,
    DeviceCode,
    GitHubData,
    GitHubUi,
    NAME,
    SIGN_IN,
    STEP,
    SignIn,
    TOKEN,
    section,
    step,
};
use tau_ui_plugin::{
    Handle,
    Request,
    UiPlugin,
    ViewCx,
    points::{AtApp, AtSetup, SetupAt},
    tree::Node,
};

type Requests = Rc<RefCell<Vec<Request>>>;

/// Calls `with` on a view of `data`, and gives what it asked.
fn with_view<T>(
    data: &GitHubData,
    with: impl FnOnce(&mut ViewCx<'_, GitHubUi>) -> T,
) -> (T, Vec<Request>) {
    let requests: Requests = Rc::default();
    let received = requests.clone();
    let handle = Handle::new(
        NAME,
        Rc::new(move |_, request| received.borrow_mut().push(request)),
    );
    let (params, repos) = (BTreeMap::new(), BTreeMap::new());
    let list = Vec::new;
    let cards = |_: &RunId| Vec::new();
    let mut view = ViewCx::new(
        &GitHubUi,
        None,
        data,
        &(),
        &repos,
        None,
        &params,
        false,
        false,
        1400.,
        handle,
        &list,
        &cards,
    );
    let out = with(&mut view);
    let asked = requests.borrow().clone();
    (out, asked)
}

fn data(sign_in: SignIn) -> GitHubData {
    GitHubData { sign_in }
}

/// Every button in `node`, as `(label, action, args)`.
fn buttons(node: &Node) -> Vec<(String, String, Value)> {
    let mut found = Vec::new();
    let mut stack = vec![node.clone()];
    while let Some(node) = stack.pop() {
        match node {
            Node::Button {
                label,
                action,
                args,
                ..
            } => found.push((label, action, args)),
            Node::Stack { children }
            | Node::Row { children }
            | Node::Card { children } => {
                stack.extend(children.into_iter().rev())
            }
            _ => {}
        }
    }
    found
}

fn has_input(node: &Node) -> bool {
    match node {
        Node::Input { secret, submit, .. } => {
            *secret && submit.as_deref() == Some(TOKEN)
        }
        Node::Stack { children }
        | Node::Row { children }
        | Node::Card { children } => children.iter().any(has_input),
        _ => false,
    }
}

/// Clicks the button labelled `label` in `node`, and gives what it asked.
fn click(data: &GitHubData, node: &Node, label: &str) -> Vec<Request> {
    let (_, action, args) = buttons(node)
        .into_iter()
        .find(|(found, ..)| found == label)
        .unwrap_or_else(|| panic!("a button {label}: {node:?}"));
    with_view(data, |view| GitHubUi.on_action(&action, &args, view)).1
}

/// Without a host the plugin draws nothing; with one, its step comes
/// before the repositories, and its first button asks the host half to
/// start a device sign-in.
#[test]
fn the_step_comes_before_the_repositories_and_starts_a_sign_in() {
    let unknown = data(SignIn::Unknown);
    let (none, _) = with_view(&unknown, |view| step(&AtSetup::default(), view));
    assert!(none.is_none(), "no host has said");
    let (none, _) = with_view(&unknown, |view| section(&AtApp, view));
    assert!(none.is_none());

    let signed_out = data(SignIn::SignedOut);
    let (view, _) =
        with_view(&signed_out, |view| step(&AtSetup::default(), view));
    let view = view.expect("a step");
    assert_eq!(view.step, STEP);
    assert_eq!(view.at, SetupAt::BeforeRepositories);
    assert_eq!(view.title, "GitHub");
    let asked = click(&signed_out, &view.body, "Sign in with GitHub");
    assert_eq!(
        asked,
        [
            Request::Ui {
                key: tau_github::TOKEN_FORM.into(),
                value: None,
            },
            Request::Act(json!({ "act": "sign_in" })),
        ]
    );
}

/// While GitHub waits for the code, its button copies the code and
/// opens the page, and "I have approved it" asks to check now.
#[test]
fn the_code_opens_github_and_approving_checks() {
    let code = DeviceCode {
        code: "ABCD-1234".into(),
        url: "github.com/login/device".into(),
        expires: "15 minutes".into(),
    };
    let waiting = data(SignIn::Waiting { code: code.clone() });
    let (view, _) = with_view(&waiting, |view| step(&AtSetup::default(), view));
    let body = view.expect("a step").body;
    assert_eq!(
        click(&waiting, &body, "Copy code and open GitHub"),
        [
            Request::Copy("ABCD-1234".into()),
            Request::OpenUrl("https://github.com/login/device".into()),
        ]
    );
    assert_eq!(
        click(&waiting, &body, "I have approved it"),
        [Request::Act(serde_json::to_value(Act::Check).unwrap())]
    );
}

/// The token form has a secret input; its Enter sends the token, trimmed,
/// and an empty one nothing.
#[test]
fn the_token_s_enter_sends_it() {
    let signed_out = data(SignIn::SignedOut);
    let body = tau_github::sign_in::form(&signed_out.sign_in, true, true);
    assert!(has_input(&body), "{body:?}");
    let (_, asked) = with_view(&signed_out, |view| {
        GitHubUi.on_action(TOKEN, &json!({ "text": " github_pat_x " }), view)
    });
    assert_eq!(
        asked,
        [Request::Act(
            json!({ "act": "token", "token": "github_pat_x" })
        )]
    );
    let (_, asked) = with_view(&signed_out, |view| {
        GitHubUi.on_action(TOKEN, &json!({ "text": "  " }), view)
    });
    assert!(asked.is_empty());
    // Another plugin's action names are not its own.
    let (_, asked) = with_view(&signed_out, |view| {
        GitHubUi.on_action("other", &json!({ "do": "continue" }), view)
    });
    assert!(asked.is_empty());
}

/// Signed in, the step goes on with Continue, and the host half's
/// answer to a saved sign-in ends it; the Models section offers Sign
/// out.
#[test]
fn signed_in_the_step_goes_on_and_the_section_signs_out() {
    let signed_in = data(SignIn::SignedIn {
        user: "octocat".into(),
    });
    let (view, _) =
        with_view(&signed_in, |view| step(&AtSetup::default(), view));
    let body = view.expect("a step").body;
    assert_eq!(click(&signed_in, &body, "Continue"), [Request::SetupDone]);

    let (section, _) = with_view(&signed_in, |view| section(&AtApp, view));
    let section = section.expect("a section");
    assert_eq!(
        click(&signed_in, &section, "Sign out"),
        [Request::Act(json!({ "act": "sign_out" }))]
    );

    let requests: Requests = Rc::default();
    let received = requests.clone();
    let handle = Handle::new(
        NAME,
        Rc::new(move |_, request| received.borrow_mut().push(request)),
    );
    GitHubUi.reply(json!({ "user": "octocat" }), &handle);
    assert_eq!(
        *requests.borrow(),
        [
            Request::Ui {
                key: tau_github::TOKEN_FORM.into(),
                value: None,
            },
            Request::SetupDone,
        ]
    );
    // The button's action is the sign-in's.
    assert!(
        buttons(&section)
            .iter()
            .all(|(_, action, _)| action == SIGN_IN)
    );
}
