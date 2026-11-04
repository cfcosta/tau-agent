//! The Servers page, a tab of a repository's page: a line on its MCP
//! servers (or the user's alone) and what waits on the user, the
//! repository servers that wait for approval, entries that were
//! skipped, and each server with where it comes from, how it is reached
//! and exposed, whether it is connected and its tools; and the editor
//! for the servers the page adds. A view tree (ADR 0045), in the frame
//! the interface gives a page. The tree has no modal, so the editor is
//! a card at the page's top while it is open.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_ui_plugin::{
    Handle,
    ViewCx,
    tree::{Kind, Node, Tone, draft_key},
};

use super::{
    Act,
    Defined,
    McpUi,
    Off,
    PendingRow,
    Reply,
    ServerRow,
    Servers,
    ToolRow,
    server_entry,
};
use crate::info::Annotations;

/// The UI state key of the editor while it is open: an [`Editor`].
pub const EDITOR: &str = "editor";
/// The UI state key of the server whose removal waits for a second
/// click.
pub const REMOVING: &str = "removing";
/// The editor's inputs: a new server's name, and its entry. Their
/// drafts are kept under [`draft_key`].
pub const NAME_INPUT: &str = "mcp-name";
pub const ENTRY_INPUT: &str = "mcp-entry";

/// Carries out the [`Act`] its arguments hold.
pub const ACT: &str = "act";
/// Opens the editor on a new server.
pub const ADD: &str = "add";
/// Opens the editor on a settings server: `{ name, entry }`.
pub const EDIT: &str = "edit";
/// Closes the editor and forgets a removal asked.
pub const CANCEL: &str = "cancel";
/// Saves the server written: `{ editing, name, entry }`.
pub const SAVE: &str = "save";
/// Asks to remove a server, or removes it once asked: `{ name,
/// confirmed }`.
pub const REMOVE: &str = "remove";

/// A server being added or edited.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Editor {
    /// The server being edited, or `None` for a new one.
    pub editing: Option<String>,
    /// Why Save did nothing, once pressed.
    pub problem: Option<String>,
}

/// What the host answered a prompt: its messages fill the composer; a
/// failure says why and puts the command back to fix.
pub(super) fn answered(handle: &Handle, reply: Reply) {
    match reply {
        Reply::Prompt { text } => handle.composer(text),
        Reply::PromptFailed { error, command } => {
            handle.alert("The prompt could not be had", error);
            handle.composer(command);
        }
        // The browser signs in; the host waits for it to come back.
        Reply::SignIn { url, .. } => handle.open_url(url),
    }
}

/// What the page keeps in the window's UI state: the editor, the
/// removal asked, and the editor's drafts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Held {
    pub editor: Option<Editor>,
    pub removing: Option<String>,
    pub name: String,
    pub entry: String,
}

impl Held {
    pub fn of(view: &ViewCx<'_, McpUi>) -> Self {
        let draft = |input: &str| {
            view.ui_value(&draft_key(input))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Self {
            editor: view
                .ui_value(EDITOR)
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
            removing: view
                .ui_value(REMOVING)
                .and_then(Value::as_str)
                .map(str::to_owned),
            name: draft(NAME_INPUT),
            entry: draft(ENTRY_INPUT),
        }
    }
}

/// What saving the server written sends, when it reads as a server the
/// page may add: a new name the user's file does not have
/// (`user_names`) and an entry that parses. Otherwise why not. The host
/// checks again.
pub fn checked(
    editing: Option<&str>,
    name: &str,
    entry: &str,
    user_names: &BTreeSet<String>,
) -> Result<Act, String> {
    let name = editing.unwrap_or(name).trim().to_owned();
    if editing.is_none() && user_names.contains(&name) {
        return Err(format!(
            "Your file mcp.json has a server named `{name}`: edit it there, \
             or pick another name."
        ));
    }
    let entry: Value = serde_json::from_str(entry)
        .map_err(|error| format!("The entry is not JSON: {error}"))?;
    server_entry(&name, &entry)?;
    Ok(match editing {
        Some(_) => Act::Edit { name, entry },
        None => Act::Add { name, entry },
    })
}

/// What the page's buttons and switches do, in `view`: a new server
/// may not take a name of the user's file, as the page shows them now.
pub fn on_action(action: &str, args: &Value, view: &mut ViewCx<'_, McpUi>) {
    let handle = &view.handle;
    let text =
        |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    match action {
        ACT => {
            if let Ok(act) = serde_json::from_value::<Act>(args.clone()) {
                handle.act(act);
            }
        }
        ADD => open(handle, Editor::default(), String::new()),
        EDIT => {
            let entry = args.get("entry").unwrap_or(&Value::Null);
            let entry = serde_json::to_string_pretty(entry).unwrap_or_default();
            let editor = Editor {
                editing: text("name"),
                problem: None,
            };
            open(handle, editor, entry);
        }
        CANCEL => {
            handle.set_ui(EDITOR, None);
            handle.set_ui(REMOVING, None);
        }
        SAVE => {
            let editing = text("editing");
            let (_, servers) = servers_of(view);
            match checked(
                editing.as_deref(),
                &text("name").unwrap_or_default(),
                &text("entry").unwrap_or_default(),
                &servers.user_names,
            ) {
                Ok(act) => {
                    handle.act(act);
                    handle.set_ui(EDITOR, None);
                }
                Err(problem) => {
                    let editor = Editor {
                        editing,
                        problem: Some(problem),
                    };
                    handle.set_ui(EDITOR, serde_json::to_value(editor).ok());
                }
            }
        }
        REMOVE => {
            let Some(name) = text("name") else {
                return;
            };
            if args.get("confirmed").and_then(Value::as_bool) == Some(true) {
                handle.set_ui(REMOVING, None);
                handle.act(Act::Remove { name });
            } else {
                handle.set_ui(REMOVING, Some(Value::String(name)));
            }
        }
        _ => {}
    }
}

/// Opens `editor`, its name empty and its entry `entry`.
fn open(handle: &Handle, editor: Editor, entry: String) {
    handle.set_ui(EDITOR, serde_json::to_value(editor).ok());
    handle.set_ui(draft_key(NAME_INPUT), None);
    handle.set_ui(
        draft_key(ENTRY_INPUT),
        (!entry.is_empty()).then_some(Value::String(entry)),
    );
}

/// The servers the page shows: the repository's in `repo`, or the
/// user's alone when it names none, or one not listed.
pub fn servers_of<'a>(
    view: &'a ViewCx<'_, McpUi>,
) -> (Option<String>, &'a Servers) {
    // A settings pane names the repository as its scope (ADR 0029).
    match view
        .param("repo")
        .or_else(|| view.scope())
        .filter(|repo| !repo.is_empty())
        .and_then(|repo| Some((repo.to_owned(), view.repo(repo)?)))
    {
        Some((repo, servers)) => (Some(repo), servers),
        None => (None, view.data),
    }
}

/// The most tools a server shows as chips; the rest are counted, and
/// its details show them all.
pub const CHIPS: usize = 12;

/// The page: what the servers are and what waits on the user, the
/// editor while open, the repository servers waiting for approval,
/// entries that were skipped, and each server with its tools.
pub fn render(view: &mut ViewCx<'_, McpUi>) -> Node {
    draw(view)
}

/// Its settings pane: the servers, everywhere or in the repository in
/// scope, as the page has them, inside the host's pane.
pub fn settings_pane(view: &mut ViewCx<'_, McpUi>) -> Node {
    draw(view)
}

fn draw(view: &ViewCx<'_, McpUi>) -> Node {
    let (repo, servers) = servers_of(view);
    tree(repo.as_deref(), servers, &Held::of(view), view.compact)
}

/// The page's tree, for `repo`'s servers or the user's alone, on a
/// phone when `compact`.
pub fn tree(
    repo: Option<&str>,
    servers: &Servers,
    held: &Held,
    compact: bool,
) -> Node {
    let mut page = vec![intro(repo, servers, compact)];
    if let Some(editor) = &held.editor {
        page.push(editor_card(editor, held));
    }
    page.extend(servers.pending.iter().map(|row| pending(repo, row)));
    page.extend(servers.errors.iter().map(|error| {
        Node::row([icon("warning", Tone::Bad), Node::text(error.clone())])
    }));
    if servers.servers.is_empty() {
        page.push(Node::Empty {
            text: "No servers yet. Add one here, or in ~/.config/tau/mcp.json."
                .into(),
        });
    }
    for (at, server) in servers.servers.iter().enumerate() {
        if at > 0 {
            page.push(Node::Rule);
        }
        let removing = held.removing.as_deref() == Some(server.name.as_str());
        page.push(server_node(repo, server, removing, compact));
    }
    Node::stack(page)
}

fn icon(name: &str, tone: Tone) -> Node {
    Node::Icon {
        name: name.into(),
        tone,
    }
}

fn mono(text: impl Into<String>) -> Node {
    Node::Mono { text: text.into() }
}

fn button(label: &str, kind: Kind, action: &str, args: Value) -> Node {
    Node::Button {
        label: label.into(),
        action: action.into(),
        args,
        kind,
        icon: None,
    }
}

/// A button that asks the host half for `act`.
fn act_button(label: &str, kind: Kind, act: &Act) -> Node {
    button(
        label,
        kind,
        ACT,
        serde_json::to_value(act).unwrap_or(Value::Null),
    )
}

/// `1 server`, `2 servers`: `one` or `many` after a count.
fn counted(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// What these servers are, how many are connected and what waits on
/// the user, each count in its tone; where they are read from; and what
/// can be done: connect them before a run does, or add one.
fn intro(repo: Option<&str>, servers: &Servers, compact: bool) -> Node {
    let mut line = vec![Node::text(match repo {
        Some(_) => "Servers this repository's runs can call.",
        None => "Your servers, which every repository's runs can call.",
    })];
    let mut count = |text: String, tone| line.push(Node::Pill { text, tone });
    if !servers.servers.is_empty() {
        match servers.started {
            true => {
                count(format!("{} connected", servers.connected()), Tone::Good)
            }
            false => count("none connected yet".to_owned(), Tone::Neutral),
        }
    }
    if servers.needs_sign_in() > 0 {
        count(
            counted(servers.needs_sign_in(), "needs sign-in", "need sign-in"),
            Tone::Warn,
        );
    }
    if servers.failed() > 0 {
        count(format!("{} failed", servers.failed()), Tone::Bad);
    }
    if !servers.pending.is_empty() {
        count(
            counted(
                servers.pending.len(),
                "needs your approval",
                "need your approval",
            ),
            Tone::Warn,
        );
    }
    let mut intro = vec![Node::row(line)];
    let files = [servers.user_file.as_deref(), servers.repo_file.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" and ");
    if !compact && !files.is_empty() {
        intro.push(Node::text(format!(
            "Read from {files}, and the servers added here. A repository's \
             own servers wait for your approval; until a run starts them, \
             Connect does."
        )));
    }
    let mut buttons = Vec::new();
    if !servers.started && !servers.servers.is_empty() {
        buttons.push(act_button(
            "Connect now",
            Kind::Secondary,
            &Act::Reconnect {
                repo: repo.map(str::to_owned),
                server: None,
            },
        ));
    }
    buttons.push(button("Add a server", Kind::Secondary, ADD, Value::Null));
    intro.push(Node::row(buttons));
    Node::stack(intro)
}

/// The editor of a server the page adds, over the page: its name, and
/// its entry as the files write it. A click outside it cancels.
fn editor_card(editor: &Editor, held: &Held) -> Node {
    let title = match &editor.editing {
        Some(_) => "Edit server",
        None => "Add server",
    };
    let mut body = vec![
        Node::Rich {
            text: "The entry as mcp.json writes it: `command` and `args` for \
                   a program, or `url` and `headers` for HTTP; `exposure`, \
                   `toolExposure`, `description`, `timeout`. `${VAR}` reads \
                   tau's environment."
                .into(),
        },
        match &editor.editing {
            Some(editing) => mono(editing.clone()),
            None => Node::Input {
                id: NAME_INPUT.into(),
                placeholder: Some("linear".into()),
                multiline: false,
                submit: None,
                keep: false,
                secret: false,
                focus: None,
            },
        },
        Node::Input {
            id: ENTRY_INPUT.into(),
            placeholder: Some(
                "{ \"command\": \"uvx\", \"args\": [\"mcp-server-git\"] }"
                    .into(),
            ),
            multiline: true,
            submit: None,
            keep: false,
            secret: false,
            focus: None,
        },
    ];
    if let Some(problem) = &editor.problem {
        body.push(Node::row([
            icon("warning", Tone::Bad),
            Node::text(problem.clone()),
        ]));
    }
    body.push(Node::row([
        button("Cancel", Kind::Secondary, CANCEL, Value::Null),
        button(
            "Save",
            Kind::Primary,
            SAVE,
            json!({
                "editing": editor.editing,
                "name": held.name,
                "entry": held.entry,
            }),
        ),
    ]));
    Node::Modal {
        title: title.into(),
        children: body,
        dismiss: Some(CANCEL.into()),
    }
}

/// A repository server waiting for approval, in a card with its whole
/// entry: approving runs what it says.
fn pending(repo: Option<&str>, row: &PendingRow) -> Node {
    let mut card = vec![
        Node::row([
            Node::Heading {
                text: row.name.clone(),
            },
            mono(row.transport.clone()),
            Node::Spacer,
            Node::Badge {
                text: "needs approval".into(),
                tone: Tone::Warn,
            },
        ]),
        Node::text(
            "The repository's .tau/mcp.json starts this server. It connects \
             once approved, and asks again if the entry changes.",
        ),
        Node::Code {
            lang: Some("json".into()),
            text: serde_json::to_string_pretty(&row.entry).unwrap_or_default(),
            first_line: None,
        },
    ];
    if let Some(repo) = repo {
        card.push(Node::row([act_button(
            "Approve",
            Kind::Primary,
            &Act::Approve {
                repo: repo.to_owned(),
                server: row.name.clone(),
                hash: row.hash.clone(),
            },
        )]));
    }
    Node::Card {
        children: vec![Node::stack(card)],
    }
}

/// A state's tone.
fn state_tone(state: Option<&str>) -> Tone {
    match state {
        Some("connected") => Tone::Good,
        Some("connecting") => Tone::Info,
        Some("failed") => Tone::Bad,
        Some(super::NEEDS_AUTH) => Tone::Warn,
        _ => Tone::Neutral,
    }
}

/// A state as the page says it.
pub fn state_label(state: &str) -> &str {
    match state {
        super::NEEDS_AUTH => "needs sign-in",
        other => other,
    }
}

/// Who is signed in, where, with which scopes; or what the server wants.
pub fn sign_in_line(server: &ServerRow) -> Option<String> {
    let auth = server.auth.as_ref()?;
    if !auth.signed_in {
        return auth
            .wants_scope
            .as_ref()
            .map(|scope| format!("Asks for {scope}"));
    }
    let mut parts = vec![match &auth.account {
        Some(account) => format!("Signed in as {account}"),
        None => "Signed in".to_owned(),
    }];
    if let Some(issuer) = &auth.issuer {
        parts[0].push_str(&format!(" at {issuer}"));
    }
    if !auth.scopes.is_empty() {
        parts.push(format!("scopes {}", auth.scopes.join(" ")));
    }
    if let Some(scope) = &auth.wants_scope {
        parts.push(format!("asks for {scope}"));
    }
    if auth.refreshes {
        parts.push("refreshes".to_owned());
    }
    Some(parts.join(" · "))
}

/// What a server's line says on its right: its state when it is not
/// connected, and how many tools, resources and prompts it has.
pub fn server_note(server: &ServerRow, repo: Option<&str>) -> String {
    let mut parts = Vec::new();
    match server.state.as_deref() {
        Some("connected") => {}
        Some(state) => parts.push(state_label(state).to_owned()),
        None if repo.is_none() && per_repo(server) => {
            parts.push("starts in each repository".to_owned())
        }
        None => parts.push("not started".to_owned()),
    }
    parts.push(counted(server.tools.len(), "tool", "tools"));
    if !server.resources.is_empty() {
        parts.push(counted(server.resources.len(), "resource", "resources"));
    }
    if !server.templates.is_empty() {
        parts.push(counted(server.templates.len(), "template", "templates"));
    }
    if !server.prompts.is_empty() {
        parts.push(counted(server.prompts.len(), "prompt", "prompts"));
    }
    parts.join(" · ")
}

/// Where a server comes from and how its tools reach runs:
/// `settings · codemode · shared`.
fn origin(server: &ServerRow) -> String {
    let mut parts = vec![server.defined.label(), server.exposure.as_str()];
    if server.shared {
        parts.push("shared");
    }
    if per_repo(server) {
        parts.push("per repository");
    }
    parts.join(" · ")
}

/// What can be done to a server: sign in or out, connect it again, and
/// edit or remove it.
fn server_actions(
    repo: Option<&str>,
    server: &ServerRow,
    removing: bool,
) -> Vec<Node> {
    let (repo, name) = (repo.map(str::to_owned), server.name.clone());
    let waiting = server.state.as_deref() == Some(super::NEEDS_AUTH);
    let signed_in = server.auth.as_ref().is_some_and(|auth| auth.signed_in);
    let mut actions = Vec::new();
    if waiting && server.auth.is_some() {
        actions.push(act_button(
            if signed_in {
                "Sign in again"
            } else {
                "Sign in"
            },
            Kind::Primary,
            &Act::SignIn {
                repo: repo.clone(),
                server: name.clone(),
            },
        ));
    }
    if signed_in {
        actions.push(act_button(
            "Sign out",
            Kind::Secondary,
            &Act::SignOut {
                repo: repo.clone(),
                server: name.clone(),
            },
        ));
    }
    if server.state.is_some() && server.enabled {
        actions.push(act_button(
            "Reconnect",
            Kind::Secondary,
            &Act::Reconnect {
                repo,
                server: Some(name.clone()),
            },
        ));
    }
    if server.defined == Defined::Settings {
        actions.push(button(
            "Edit",
            Kind::Secondary,
            EDIT,
            json!({
                "name": name,
                "entry": server.entry.clone().unwrap_or(Value::Null),
            }),
        ));
        actions.push(button(
            if removing { "Remove it" } else { "Remove" },
            if removing {
                Kind::Danger
            } else {
                Kind::Secondary
            },
            REMOVE,
            json!({ "name": name, "confirmed": removing }),
        ));
    }
    actions
}

/// One server: its state's dot, its name and how it is reached, what it
/// has; where it comes from and what can be done; its description,
/// sign-in and last error; whether it is on; and its tools as chips,
/// with its details (tools whole, resources and prompts) to open.
fn server_node(
    repo: Option<&str>,
    server: &ServerRow,
    removing: bool,
    compact: bool,
) -> Node {
    let mut head = vec![Node::Status {
        text: server.name.clone(),
        detail: Some(server.transport.clone()),
        tone: state_tone(server.state.as_deref()),
    }];
    if !compact {
        head.push(Node::Spacer);
        head.push(Node::text(server_note(server, repo)));
    }
    let about = match compact {
        true => format!("{} · {}", origin(server), server_note(server, repo)),
        false => origin(server),
    };
    let mut actions = vec![mono(about), Node::Spacer];
    actions.extend(server_actions(repo, server, removing));
    let mut lines = vec![Node::row(head), Node::row(actions)];
    if let Some(description) = &server.description {
        lines.push(Node::text(description.clone()));
    }
    if let Some(line) = sign_in_line(server) {
        lines.push(mono(line));
    }
    if let Some(error) = &server.error {
        let tone = match server.state.as_deref() {
            Some(super::NEEDS_AUTH) => Tone::Warn,
            _ => Tone::Bad,
        };
        lines.push(Node::row([icon("warning", tone), mono(error.clone())]));
    }
    lines.push(toggle(repo, server));
    if !server.tools.is_empty() {
        lines.push(tool_chips(server));
    }
    if !server.tools.is_empty() || has_more(server) {
        lines.push(Node::Section {
            id: format!("mcp-details-{}", server.name),
            title: "Details".into(),
            open: false,
            children: details(server),
        });
    }
    Node::stack(lines)
}

/// A tool's chip tone: hidden tools quiet, destructive ones bad.
fn chip_tone(tool: &ToolRow) -> Tone {
    if tool.name.is_none() {
        Tone::Neutral
    } else if tool.annotations.destructive == Some(true) {
        Tone::Bad
    } else {
        Tone::Info
    }
}

/// A server's first [`CHIPS`] tools as chips, and the rest counted.
fn tool_chips(server: &ServerRow) -> Node {
    let more = server.tools.len().saturating_sub(CHIPS);
    let mut chips: Vec<Node> = server
        .tools
        .iter()
        .take(CHIPS)
        .map(|tool| Node::Badge {
            text: tool.tool.clone(),
            tone: chip_tone(tool),
        })
        .collect();
    if more > 0 {
        chips.push(Node::Badge {
            text: format!("+{more}"),
            tone: Tone::Neutral,
        });
    }
    Node::row(chips)
}

/// Whether `server` is one of the user's that is on and runs once per
/// repository: its `cwd` is relative, or a repository turned it off for
/// itself alone.
fn per_repo(server: &ServerRow) -> bool {
    !server.shared && server.defined != Defined::Repo && server.enabled
}

/// Why the page cannot turn `server` on here, when it cannot: its
/// entry turns it off, or, in a repository, it is one of the user's
/// servers turned off for every repository.
pub fn locked(server: &ServerRow, in_repo: bool) -> Option<&'static str> {
    match server.off {
        Some(Off::Entry) => Some("off in its entry"),
        Some(Off::Everywhere) if in_repo => Some("off in every repository"),
        _ => None,
    }
}

/// What turning `server` on or off says, and where: in `repo`, or for
/// every repository on the page of the user's servers.
pub fn reach(server: &ServerRow, repo: Option<&str>) -> &'static str {
    match (repo, server.defined) {
        (None, _) => "every repository",
        (Some(_), Defined::Repo) => "this repository",
        (Some(_), _) => "this repository only",
    }
}

/// The switch that turns `server` on or off at the page's level, or why
/// it cannot.
fn toggle(repo: Option<&str>, server: &ServerRow) -> Node {
    if let Some(why) = locked(server, repo.is_some()) {
        return mono(why);
    }
    let act = Act::Enable {
        repo: repo.map(str::to_owned),
        name: server.name.clone(),
        enabled: !server.enabled,
    };
    Node::Switch {
        label: format!("On in {}", reach(server, repo)),
        detail: None,
        on: server.enabled,
        action: ACT.into(),
        args: serde_json::to_value(act).unwrap_or(Value::Null),
    }
}

/// What a tool's hints say, as badges: read-only, destructive,
/// idempotent, open world. A hint the tool did not give says nothing.
pub fn hints(annotations: &Annotations) -> Vec<&'static str> {
    let mut hints = Vec::new();
    if annotations.read_only == Some(true) {
        hints.push("read-only");
    }
    if annotations.destructive == Some(true) {
        hints.push("destructive");
    }
    if annotations.idempotent == Some(true) {
        hints.push("idempotent");
    }
    if annotations.open_world == Some(true) {
        hints.push("open world");
    }
    hints
}

/// A hint's badge: destructive bad, read-only good.
pub fn hint_badge(hint: &'static str) -> Node {
    Node::Badge {
        text: hint.into(),
        tone: match hint {
            "destructive" => Tone::Bad,
            "read-only" => Tone::Good,
            _ => Tone::Neutral,
        },
    }
}

/// The most rows of each list a server's details show; the rest are
/// counted.
pub const LIST_ROWS: usize = 20;

/// Whether the server offers resources, templates or prompts.
fn has_more(server: &ServerRow) -> bool {
    !server.resources.is_empty()
        || !server.templates.is_empty()
        || !server.prompts.is_empty()
}

/// A description's first line.
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

/// A server's details: its tools whole, then its resources, resource
/// templates and prompts.
fn details(server: &ServerRow) -> Vec<Node> {
    let mut details = Vec::new();
    if !server.tools.is_empty() {
        details.push(Node::Heading {
            text: format!("Tools · {}", server.tools.len()),
        });
        details.push(Node::Table {
            head: vec![
                "Tool".into(),
                "Exposure".into(),
                "Hints".into(),
                "Called as".into(),
            ],
            rows: server.tools.iter().map(tool_row).collect(),
            widths: vec![1.5, 1., 1.5, 2.],
        });
    }
    if has_more(server) {
        details.extend(offers(server));
    }
    details
}

/// One of a server's tools: its name and description, exposure, hints,
/// and the name tools call it by.
fn tool_row(tool: &ToolRow) -> Vec<Node> {
    let mut named = vec![mono(tool.tool.clone())];
    if let Some(description) = &tool.description {
        named.push(Node::text(first_line(description)));
    }
    vec![
        Node::stack(named),
        Node::Badge {
            text: tool.exposure.clone(),
            tone: Tone::Neutral,
        },
        Node::row(hints(&tool.annotations).into_iter().map(hint_badge)),
        mono(tool.name.clone().unwrap_or_default()),
    ]
}

/// A server's resources, resource templates and prompts: how many of
/// each, and the first [`LIST_ROWS`] of each.
fn offers(server: &ServerRow) -> Vec<Node> {
    let count = |what: &str, count: usize| Node::Badge {
        text: format!("{count} {what}"),
        tone: Tone::Neutral,
    };
    let mut offers = vec![Node::row([
        count("resources", server.resources.len()),
        count("templates", server.templates.len()),
        count("prompts", server.prompts.len()),
    ])];
    if !server.resources.is_empty() {
        let rows = server.resources.iter().map(|resource| {
            let mut about = vec![
                resource
                    .title
                    .clone()
                    .unwrap_or_else(|| resource.name.clone()),
            ];
            if let Some(mime) = &resource.mime_type {
                about.push(mime.clone());
            }
            listed_row(
                resource.uri.clone(),
                about.join(" · "),
                resource.description.as_deref(),
            )
        });
        offers.push(listed("Resources", server.resources.len(), rows));
    }
    if !server.templates.is_empty() {
        let rows = server.templates.iter().map(|template| {
            listed_row(
                template.uri_template.clone(),
                template
                    .title
                    .clone()
                    .unwrap_or_else(|| template.name.clone()),
                template.description.as_deref(),
            )
        });
        offers.push(listed("Resource templates", server.templates.len(), rows));
    }
    if !server.prompts.is_empty() {
        let rows = server.prompts.iter().map(|prompt| {
            listed_row(
                format!("/{}", prompt.command),
                crate::prompts::arguments_hint(&prompt.info()),
                prompt.description.as_deref().or(prompt.title.as_deref()),
            )
        });
        offers.push(listed("Prompts", server.prompts.len(), rows));
    }
    offers
}

/// A list under its heading and count, cut at [`LIST_ROWS`].
fn listed(title: &str, count: usize, rows: impl Iterator<Item = Node>) -> Node {
    let mut list = vec![
        Node::Heading {
            text: format!("{title} · {count}"),
        },
        Node::List {
            items: rows.take(LIST_ROWS).collect(),
        },
    ];
    if count > LIST_ROWS {
        list.push(Node::text(format!("… {} more", count - LIST_ROWS)));
    }
    Node::stack(list)
}

/// One resource, template or prompt: what names it, what is said of it
/// beside, and its description's first line.
fn listed_row(name: String, beside: String, description: Option<&str>) -> Node {
    let mut line = vec![mono(name)];
    if !beside.is_empty() {
        line.push(mono(beside));
    }
    let mut row = vec![Node::row(line)];
    if let Some(description) = description {
        row.push(Node::text(first_line(description)));
    }
    Node::stack(row)
}
