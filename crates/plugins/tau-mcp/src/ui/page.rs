//! The Servers page, a tab of a repository's page: in one quiet column,
//! a line on its MCP servers (or the user's alone) and what waits on
//! the user, the repository servers that wait for approval, entries
//! that were skipped, and each server with where it comes from, how it
//! is reached and exposed, whether it is connected and its tools; and
//! the editor for the servers the page adds.

use std::collections::BTreeSet;

use gpui::{
    AnyElement,
    App,
    ClickEvent,
    Context,
    Div,
    Entity,
    SharedString,
    Window,
    div,
    prelude::*,
    px,
};
use serde_json::Value;
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, ButtonKind, Material as _, heading, icon, mono},
    input::TextInput,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
};
use tau_ui_plugin::{Handle, ViewCx};

use super::{
    Act,
    Defined,
    McpUi,
    Off,
    PendingRow,
    ServerRow,
    Servers,
    ToolRow,
    server_entry,
};
use crate::info::Annotations;

/// A server being added or edited.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Editor {
    /// The server being edited, or `None` for a new one.
    pub editing: Option<String>,
    /// Why Save did nothing, once pressed.
    pub problem: Option<String>,
}

/// The page's state in one window.
pub struct Ui {
    handle: Handle,
    pub(super) name: Entity<TextInput>,
    pub(super) entry: Entity<TextInput>,
    editor: Option<Editor>,
    /// The server whose removal waits for a second click.
    removing: Option<String>,
    /// The servers whose tools, resources and prompts show whole.
    details: BTreeSet<String>,
}

impl Ui {
    pub(super) fn new(
        handle: Handle,
        name: Entity<TextInput>,
        entry: Entity<TextInput>,
    ) -> Self {
        Self {
            handle,
            name,
            entry,
            editor: None,
            removing: None,
            details: BTreeSet::new(),
        }
    }

    /// What the host answered a prompt: its messages fill the composer;
    /// a failure says why and puts the command back to fix.
    pub(super) fn answered(&self, reply: super::Reply, cx: &mut Context<Self>) {
        match reply {
            super::Reply::Prompt { text } => self.handle.composer(text, cx),
            super::Reply::PromptFailed { error, command } => {
                self.handle.alert("The prompt could not be had", error, cx);
                self.handle.composer(command, cx);
            }
            // The browser signs in; the host waits for it to come back.
            super::Reply::SignIn { url, .. } => cx.open_url(&url),
        }
    }

    fn changed(&self, cx: &mut Context<Self>) {
        cx.notify();
        self.handle.refresh(cx);
    }

    pub fn editor(&self) -> Option<&Editor> {
        self.editor.as_ref()
    }

    pub fn removing(&self) -> Option<&str> {
        self.removing.as_deref()
    }

    /// Whether `name`'s tools, resources and prompts show whole.
    pub fn showing_details(&self, name: &str) -> bool {
        self.details.contains(name)
    }

    /// Shows `name`'s tools, resources and prompts whole, or as chips
    /// again.
    pub fn toggle_details(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.details.remove(name) {
            self.details.insert(name.to_owned());
        }
        cx.notify();
        self.handle.refresh(cx);
    }

    /// Opens the editor on a new server.
    pub fn open_add(&mut self, cx: &mut Context<Self>) {
        self.name.update(cx, |input, cx| input.set_text("", cx));
        self.entry.update(cx, |input, cx| input.set_text("", cx));
        self.editor = Some(Editor::default());
        self.changed(cx);
    }

    /// Opens the editor on the settings server `name`, with its entry.
    pub fn open_edit(
        &mut self,
        name: &str,
        entry: &Value,
        cx: &mut Context<Self>,
    ) {
        let text = serde_json::to_string_pretty(entry).unwrap_or_default();
        self.name
            .update(cx, |input, cx| input.set_text(name.to_owned(), cx));
        self.entry.update(cx, |input, cx| input.set_text(text, cx));
        self.editor = Some(Editor {
            editing: Some(name.to_owned()),
            problem: None,
        });
        self.changed(cx);
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.removing = None;
        self.changed(cx);
    }

    pub fn set_name(&mut self, text: &str, cx: &mut Context<Self>) {
        self.name
            .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
    }

    pub fn set_entry(&mut self, text: &str, cx: &mut Context<Self>) {
        self.entry
            .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
    }

    /// Sends the server being written to the host, when it reads as a
    /// server the page may add: a new name the user's file does not
    /// have (`user_names`) and an entry that parses. Otherwise the
    /// editor says why, and nothing is sent. The host checks again.
    pub fn save(
        &mut self,
        user_names: &BTreeSet<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        let name = match &editor.editing {
            Some(name) => name.clone(),
            None => self.name.read(cx).text().trim().to_owned(),
        };
        let checked = (|| {
            if editor.editing.is_none() && user_names.contains(&name) {
                return Err(format!(
                    "Your file mcp.json has a server named `{name}`: edit it \
                     there, or pick another name."
                ));
            }
            let text = self.entry.read(cx).text().to_owned();
            let entry: Value = serde_json::from_str(&text)
                .map_err(|error| format!("The entry is not JSON: {error}"))?;
            server_entry(&name, &entry).map(|_| entry)
        })();
        match checked {
            Ok(entry) => {
                let act = match &editor.editing {
                    Some(_) => Act::Edit { name, entry },
                    None => Act::Add { name, entry },
                };
                self.handle.act(act, cx);
                self.editor = None;
                self.changed(cx);
                true
            }
            Err(problem) => {
                if let Some(editor) = &mut self.editor {
                    editor.problem = Some(problem);
                }
                self.changed(cx);
                false
            }
        }
    }

    /// Approves `pending`, a server of `repo`'s file, as shown.
    pub fn approve(
        &mut self,
        repo: &str,
        pending: &PendingRow,
        cx: &mut Context<Self>,
    ) {
        self.handle.act(
            Act::Approve {
                repo: repo.to_owned(),
                server: pending.name.clone(),
                hash: pending.hash.clone(),
            },
            cx,
        );
        self.changed(cx);
    }

    /// Asks to remove `name`; a second call removes it.
    pub fn remove(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.removing.as_deref() == Some(name) {
            self.removing = None;
            self.handle.act(
                Act::Remove {
                    name: name.to_owned(),
                },
                cx,
            );
        } else {
            self.removing = Some(name.to_owned());
        }
        self.changed(cx);
    }

    /// Turns `name` on or off in `repo`, or in every repository without
    /// one.
    pub fn enable(
        &mut self,
        repo: Option<&str>,
        name: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.handle.act(
            Act::Enable {
                repo: repo.map(str::to_owned),
                name: name.to_owned(),
                enabled,
            },
            cx,
        );
        self.changed(cx);
    }

    /// Starts signing in to `server`: the host answers with the page to
    /// open in the browser.
    pub fn sign_in(
        &mut self,
        repo: Option<&str>,
        server: &str,
        cx: &mut Context<Self>,
    ) {
        self.handle.act(
            Act::SignIn {
                repo: repo.map(str::to_owned),
                server: server.to_owned(),
            },
            cx,
        );
        self.changed(cx);
    }

    /// Signs out of `server`.
    pub fn sign_out(
        &mut self,
        repo: Option<&str>,
        server: &str,
        cx: &mut Context<Self>,
    ) {
        self.handle.act(
            Act::SignOut {
                repo: repo.map(str::to_owned),
                server: server.to_owned(),
            },
            cx,
        );
        self.changed(cx);
    }

    /// Starts `repo`'s servers, and connects `server` again, or all.
    pub fn reconnect(
        &mut self,
        repo: Option<&str>,
        server: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.handle.act(
            Act::Reconnect {
                repo: repo.map(str::to_owned),
                server: server.map(str::to_owned),
            },
            cx,
        );
        self.changed(cx);
    }
}

fn on_ui(
    ui: &Entity<Ui>,
    f: impl Fn(&mut Ui, &mut Context<Ui>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let ui = ui.clone();
    move |_, _, cx| ui.update(cx, |ui, cx| f(ui, cx))
}

/// A button that runs `f` on the page's state.
fn action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    ui: &Entity<Ui>,
    t: &Theme,
    f: impl Fn(&mut Ui, &mut Context<Ui>) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .child(ui::button(label, kind, t))
        .on_click(on_ui(ui, f))
}

/// The servers the page shows: the repository's in `repo`, or the
/// user's alone when it names none, or one not listed.
pub fn servers_of<'a>(
    view: &'a ViewCx<'_, McpUi>,
) -> (Option<String>, &'a Servers) {
    match view
        .param("repo")
        .filter(|repo| !repo.is_empty())
        .and_then(|repo| Some((repo.to_owned(), view.repo(repo)?)))
    {
        Some((repo, servers)) => (Some(repo), servers),
        None => (None, view.data),
    }
}

/// The widest the page's column grows.
const COLUMN: f32 = 860.;

/// The most tools a server shows as chips; the rest are counted on a
/// chip that opens them all.
pub const CHIPS: usize = 12;

/// How far a server's lines sit in from its dot: past the dot and its
/// gap, under the name.
const INDENT: f32 = 17.;

/// The page: what the servers are and what waits on the user, the
/// repository servers waiting for approval, entries that were skipped,
/// and each server with its tools.
pub fn render(view: &mut ViewCx<'_, McpUi>) -> AnyElement {
    let t = view.theme().clone();
    let compact = view.compact;
    let (repo, servers) = servers_of(view);
    let servers = servers.clone();
    let ui = view.ui.clone();
    let (removing, details) = {
        let state = view.read_ui();
        (state.removing.clone(), state.details.clone())
    };
    let at = repo.as_deref();
    let content = div()
        .w_full()
        .max_w(px(COLUMN))
        .mx_auto()
        .flex()
        .flex_col()
        .gap(sp(3.5))
        .child(intro(&ui, at, &servers, compact, &t))
        .children(servers.pending.iter().map(|row| pending(&ui, at, row, &t)))
        .children(servers.errors.iter().map(|error| {
            ui::notice(Icon::Warning, error.clone(), t.red, Type::SMALL, &t)
        }))
        .child(if servers.servers.is_empty() {
            ui::empty(
                "No servers yet. Add one here, or in ~/.config/tau/mcp.json.",
                &t,
            )
            .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(servers.servers.iter().map(|server| {
                    server_section(
                        &ui,
                        at,
                        server,
                        removing.as_deref() == Some(server.name.as_str()),
                        details.contains(&server.name),
                        compact,
                        &t,
                    )
                }))
                .into_any_element()
        });
    let editor = view
        .read_ui()
        .editor
        .clone()
        .map(|editor| editor_modal(view, &editor, &servers.user_names, &t));
    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .child(ui::screen("mcp-servers", compact, content))
        .when_some(editor, |page, editor| page.child(editor))
        .into_any_element()
}

/// `1 server`, `2 servers`: `one` or `many` after a count.
fn counted(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// What these servers are, how many are connected and what waits on
/// the user, each count in its color; where they are read from; and
/// what can be done: connect them before a run does, or add one.
fn intro(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    servers: &Servers,
    compact: bool,
    t: &Theme,
) -> Div {
    let mut counts = Vec::new();
    if !servers.servers.is_empty() {
        counts.push(match servers.started {
            true => (format!("{} connected", servers.connected()), t.green),
            false => ("none connected yet".to_owned(), t.muted),
        });
    }
    if servers.needs_sign_in() > 0 {
        counts.push((
            counted(servers.needs_sign_in(), "needs sign-in", "need sign-in"),
            t.roles.live,
        ));
    }
    if servers.failed() > 0 {
        counts.push((format!("{} failed", servers.failed()), t.red));
    }
    if !servers.pending.is_empty() {
        counts.push((
            counted(
                servers.pending.len(),
                "needs your approval",
                "need your approval",
            ),
            t.roles.live,
        ));
    }
    let last = counts.len().saturating_sub(1);
    let line = div()
        .flex()
        .flex_wrap()
        .typeset(Type::SMALL.sized(13.5))
        .text_color(t.muted)
        .child(match repo {
            Some(_) => "Servers this repository's runs can call. ",
            None => "Your servers, which every repository's runs can call. ",
        })
        .children(counts.into_iter().enumerate().map(
            |(at, (count, color))| {
                div()
                    .flex()
                    .child(div().text_color(color).child(count))
                    .child(if at == last { "." } else { ", " })
            },
        ));
    let files = [servers.user_file.as_deref(), servers.repo_file.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" and ");
    let connect = (!servers.started && !servers.servers.is_empty())
        .then(|| repo.map(str::to_owned));
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(3.))
        .child(
            div()
                .flex_1()
                .min_w(px(if compact { 0. } else { 300. }))
                .flex()
                .flex_col()
                .gap(sp(1.))
                .child(line)
                .when(!compact && !files.is_empty(), |intro| {
                    intro.child(ui::text(
                        format!(
                            "Read from {files}, and the servers added here. \
                             A repository's own servers wait for your \
                             approval; until a run starts them, Connect does."
                        ),
                        Type::CAPTION,
                        t.dim,
                    ))
                }),
        )
        .when_some(connect, |row, repo| {
            row.child(action(
                "mcp-connect",
                "Connect now",
                ButtonKind::Secondary,
                ui,
                t,
                move |ui, cx| ui.reconnect(repo.as_deref(), None, cx),
            ))
        })
        .child(action(
            "mcp-add",
            "Add a server",
            ButtonKind::Secondary,
            ui,
            t,
            |ui, cx| ui.open_add(cx),
        ))
}

/// A repository server waiting for approval, in a filled panel with
/// its whole entry: approving runs what it says.
fn pending(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    row: &PendingRow,
    t: &Theme,
) -> Div {
    let (repo, row) = (repo.map(str::to_owned), row.clone());
    let entry = serde_json::to_string_pretty(&row.entry).unwrap_or_default();
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .px(sp(4.5))
        .py(sp(4.))
        .rounded(radius::CARD)
        .bg(t.card)
        .border_1()
        .border_color(t.border_strong)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(weight::STRONG)
                        .text_color(t.text)
                        .child(row.name.clone()),
                )
                .child(
                    mono(row.transport.clone(), Type::CAPTION, t.dim)
                        .flex_1()
                        .min_w(px(0.))
                        .truncate(),
                )
                .child(
                    ui::text("needs approval", Type::SMALL, t.roles.live)
                        .flex_shrink_0(),
                ),
        )
        .child(ui::text(
            "The repository's .tau/mcp.json starts this server. It connects \
             once approved, and asks again if the entry changes.",
            Type::SMALL.sized(13.5),
            t.text_soft,
        ))
        .child(ui::code_block(Some("json"), &entry, t))
        .when_some(repo, |panel, repo| {
            let id = format!("mcp-approve-{}", row.name);
            panel.child(div().flex().gap(sp(2.)).child(action(
                id,
                "Approve",
                ButtonKind::Primary,
                ui,
                t,
                move |ui, cx| ui.approve(&repo, &row, cx),
            )))
        })
}

/// A state's color.
fn state_color(state: Option<&str>, t: &Theme) -> gpui::Hsla {
    match state {
        Some("connected") => t.green,
        Some("connecting") => t.roles.waiting,
        Some("failed") => t.red,
        Some(super::NEEDS_AUTH) => t.roles.live,
        _ => t.dim,
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

/// Words that act, quieter than a button: a server's own actions.
fn quiet_action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    color: gpui::Hsla,
    ui: &Entity<Ui>,
    f: impl Fn(&mut Ui, &mut Context<Ui>) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .flex_shrink_0()
        .typeset(Type::CAPTION)
        .text_color(color)
        .cursor_pointer()
        .hover(|style| style.underline())
        .child(label.into())
        .on_click(on_ui(ui, f))
}

/// What can be done to a server: sign in or out, see its tools whole,
/// connect it again, edit or remove it, and turn it on or off.
fn server_actions(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    server: &ServerRow,
    removing: bool,
    open: bool,
    compact: bool,
    t: &Theme,
) -> Div {
    let name = server.name.clone();
    let waiting = server.state.as_deref() == Some(super::NEEDS_AUTH);
    let signed_in = server.auth.as_ref().is_some_and(|auth| auth.signed_in);
    let has_details = !server.tools.is_empty() || has_more(server);
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(3.))
        .when(waiting && server.auth.is_some(), |row| {
            let (repo, name) = (repo.map(str::to_owned), name.clone());
            row.child(action(
                format!("mcp-sign-in-{name}"),
                if signed_in {
                    "Sign in again"
                } else {
                    "Sign in"
                },
                ButtonKind::Primary,
                ui,
                t,
                move |ui, cx| ui.sign_in(repo.as_deref(), &name, cx),
            ))
        })
        .when(has_details, |row| {
            let name = name.clone();
            row.child(quiet_action(
                format!("mcp-details-{name}"),
                if open { "Hide details" } else { "Details" },
                t.muted,
                ui,
                move |ui, cx| ui.toggle_details(&name, cx),
            ))
        })
        .when(signed_in, |row| {
            let (repo, name) = (repo.map(str::to_owned), name.clone());
            row.child(quiet_action(
                format!("mcp-sign-out-{name}"),
                "Sign out",
                t.muted,
                ui,
                move |ui, cx| ui.sign_out(repo.as_deref(), &name, cx),
            ))
        })
        .when(server.state.is_some() && server.enabled, |row| {
            let (repo, name) = (repo.map(str::to_owned), name.clone());
            row.child(quiet_action(
                format!("mcp-reconnect-{name}"),
                "Reconnect",
                t.muted,
                ui,
                move |ui, cx| ui.reconnect(repo.as_deref(), Some(&name), cx),
            ))
        })
        .when(server.defined == Defined::Settings, |row| {
            let entry = server.entry.clone().unwrap_or(Value::Null);
            let (edit, remove) = (name.clone(), name.clone());
            row.child(quiet_action(
                format!("mcp-edit-{name}"),
                "Edit",
                t.muted,
                ui,
                move |ui, cx| ui.open_edit(&edit, &entry, cx),
            ))
            .child(quiet_action(
                format!("mcp-remove-{name}"),
                if removing { "Remove it" } else { "Remove" },
                if removing { t.red } else { t.muted },
                ui,
                move |ui, cx| ui.remove(&remove, cx),
            ))
        })
        .child(toggle(ui, repo, server, compact, t))
}

/// One server, as a section over a rule: its state's dot, its name and
/// how it is reached, what it has; where it comes from and what can be
/// done; its description, sign-in and last error; and its tools as
/// chips, or whole with its resources and prompts once opened.
fn server_section(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    server: &ServerRow,
    removing: bool,
    open: bool,
    compact: bool,
    t: &Theme,
) -> Div {
    let below = || div().pl(px(INDENT));
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .px(sp(1.))
        .pt(sp(3.5))
        .pb(sp(4.))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(ui::dot(state_color(server.state.as_deref(), t), 7.))
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(weight::STRONG)
                        .text_color(t.text)
                        .child(server.name.clone()),
                )
                .child(
                    mono(server.transport.clone(), Type::CAPTION, t.dim)
                        .flex_1()
                        .min_w(px(0.))
                        .truncate(),
                )
                .when(!compact, |line| {
                    line.child(
                        ui::text(
                            server_note(server, repo),
                            Type::SMALL,
                            t.muted,
                        )
                        .flex_shrink_0(),
                    )
                }),
        )
        .child(
            below()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(3.))
                .child(
                    mono(
                        match compact {
                            true => format!(
                                "{} · {}",
                                origin(server),
                                server_note(server, repo)
                            ),
                            false => origin(server),
                        },
                        Type::CAPTION,
                        t.dim,
                    )
                    .flex_1(),
                )
                .child(server_actions(
                    ui, repo, server, removing, open, compact, t,
                )),
        )
        .when_some(server.description.clone(), |section, description| {
            section.child(below().child(ui::text(
                description,
                Type::SMALL,
                t.text_soft,
            )))
        })
        .when_some(sign_in_line(server), |section, line| {
            section.child(below().child(mono(line, Type::CAPTION, t.muted)))
        })
        .when_some(server.error.clone(), |section, error| {
            let color = match server.state.as_deref() {
                Some(super::NEEDS_AUTH) => t.roles.live,
                _ => t.red,
            };
            section.child(below().child(mono(error, Type::CAPTION, color)))
        })
        .when(!open && !server.tools.is_empty(), |section| {
            section.child(below().child(tool_chips(ui, server, t)))
        })
        .when(open, |section| {
            section.child(
                below()
                    .flex()
                    .flex_col()
                    .gap(sp(3.))
                    .when(!server.tools.is_empty(), |details| {
                        details.child(
                            div()
                                .flex()
                                .flex_col()
                                .child(heading(
                                    &format!("Tools · {}", server.tools.len()),
                                    t,
                                ))
                                .children(
                                    server
                                        .tools
                                        .iter()
                                        .map(|tool| tool_row(tool, t)),
                                ),
                        )
                    })
                    .when(has_more(server), |details| {
                        details.child(offers(server, t))
                    }),
            )
        })
}

/// A tool's chip color: hidden tools quiet, destructive ones red.
fn chip_color(tool: &ToolRow, t: &Theme) -> gpui::Hsla {
    if tool.name.is_none() {
        t.dim
    } else if tool.annotations.destructive == Some(true) {
        t.red
    } else {
        t.blue
    }
}

/// A short mono name on a small raised chip.
fn chip(text: impl Into<SharedString>, color: gpui::Hsla, t: &Theme) -> Div {
    mono(text, Type::CAPTION, color)
        .flex_shrink_0()
        .px(sp(2.))
        .py(sp(0.5))
        .rounded(radius::CONTROL)
        .bg(t.raised)
}

/// A server's first [`CHIPS`] tools as chips, and the rest counted on
/// one that opens them all.
fn tool_chips(ui: &Entity<Ui>, server: &ServerRow, t: &Theme) -> Div {
    let more = server.tools.len().saturating_sub(CHIPS);
    let name = server.name.clone();
    div()
        .flex()
        .flex_wrap()
        .gap(sp(1.5))
        .children(
            server
                .tools
                .iter()
                .take(CHIPS)
                .map(|tool| chip(tool.tool.clone(), chip_color(tool, t), t)),
        )
        .when(more > 0, |chips| {
            chips.child(
                div()
                    .id(SharedString::from(format!("mcp-more-{name}")))
                    .cursor_pointer()
                    .child(chip(format!("+{more}"), t.muted, t))
                    .on_click(on_ui(ui, move |ui, cx| {
                        ui.toggle_details(&name, cx)
                    })),
            )
        })
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

/// The switch that turns `server` on or off at the page's level (words
/// on a phone), or why it cannot.
fn toggle(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    server: &ServerRow,
    compact: bool,
    t: &Theme,
) -> AnyElement {
    if let Some(why) = locked(server, repo.is_some()) {
        return mono(why, Type::CAPTION, t.dim).into_any_element();
    }
    let repo_name = repo;
    let (repo, name, enabled) =
        (repo.map(str::to_owned), server.name.clone(), server.enabled);
    let id = SharedString::from(format!("mcp-enable-{name}"));
    let flip = move |ui: &mut Ui, cx: &mut Context<Ui>| {
        ui.enable(repo.as_deref(), &name, !enabled, cx)
    };
    if compact {
        return quiet_action(
            id,
            if enabled { "Turn off" } else { "Turn on" },
            t.muted,
            ui,
            flip,
        )
        .into_any_element();
    }
    div()
        .flex()
        .items_center()
        .gap(sp(1.5))
        .child(mono(reach(server, repo_name), Type::MICRO, t.dim))
        .child(
            div()
                .id(id)
                .child(ui::switch(enabled, t))
                .on_click(on_ui(ui, flip)),
        )
        .into_any_element()
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

/// A hint's badge: destructive in red.
pub fn hint_badge(hint: &'static str, t: &Theme) -> Div {
    match hint {
        "destructive" => ui::badge(hint, t.red, t.red_border),
        "read-only" => ui::badge(hint, t.green, t.border),
        _ => ui::badge(hint, t.muted, t.border),
    }
}

/// The most rows of each list a server's card shows; the rest are
/// counted.
pub const LIST_ROWS: usize = 20;

/// Whether the server offers resources, templates or prompts.
fn has_more(server: &ServerRow) -> bool {
    !server.resources.is_empty()
        || !server.templates.is_empty()
        || !server.prompts.is_empty()
}

/// A server's resources, resource templates and prompts: how many of
/// each, and the first [`LIST_ROWS`] of each.
fn offers(server: &ServerRow, t: &Theme) -> Div {
    let resources = server.resources.iter().map(|resource| {
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
            resource.description.clone(),
            t,
        )
    });
    let templates = server.templates.iter().map(|template| {
        listed_row(
            template.uri_template.clone(),
            template
                .title
                .clone()
                .unwrap_or_else(|| template.name.clone()),
            template.description.clone(),
            t,
        )
    });
    let prompts = server.prompts.iter().map(|prompt| {
        listed_row(
            format!("/{}", prompt.command),
            crate::prompts::arguments_hint(&prompt.info()),
            prompt.description.clone().or_else(|| prompt.title.clone()),
            t,
        )
    });
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(sp(1.5))
                .child(count_badge("resources", server.resources.len(), t))
                .child(count_badge("templates", server.templates.len(), t))
                .child(count_badge("prompts", server.prompts.len(), t)),
        )
        .when(!server.resources.is_empty(), |list| {
            list.child(section(
                "Resources",
                server.resources.len(),
                resources,
                t,
            ))
        })
        .when(!server.templates.is_empty(), |list| {
            list.child(section(
                "Resource templates",
                server.templates.len(),
                templates,
                t,
            ))
        })
        .when(!server.prompts.is_empty(), |list| {
            list.child(section("Prompts", server.prompts.len(), prompts, t))
        })
}

/// `4 resources`, quiet when there are none.
fn count_badge(what: &str, count: usize, t: &Theme) -> Div {
    let color = if count == 0 { t.dim } else { t.muted };
    ui::badge(format!("{count} {what}"), color, t.border)
}

/// A list under its heading and count, cut at [`LIST_ROWS`].
fn section(
    title: &str,
    count: usize,
    rows: impl Iterator<Item = Div>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(heading(&format!("{title} · {count}"), t))
        .children(rows.take(LIST_ROWS))
        .when(count > LIST_ROWS, |list| {
            list.child(mono(
                format!("… {} more", count - LIST_ROWS),
                Type::MICRO,
                t.dim,
            ))
        })
}

/// One resource, template or prompt: what names it, what is said of it
/// beside, and its description's first line.
fn listed_row(
    name: String,
    beside: String,
    description: Option<String>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .py(sp(1.))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .child(mono(name, Type::CAPTION, t.blue))
                .when(!beside.is_empty(), |row| {
                    row.child(mono(beside, Type::MICRO, t.dim))
                }),
        )
        .when_some(description, |row, description| {
            row.child(
                mono(
                    description.lines().next().unwrap_or_default().to_owned(),
                    Type::MICRO,
                    t.muted,
                )
                .truncate(),
            )
        })
}

/// One of a server's tools: its name, exposure and hints.
fn tool_row(tool: &ToolRow, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .py(sp(1.))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .child(mono(tool.tool.clone(), Type::CAPTION, t.blue))
                .child(ui::badge(tool.exposure.clone(), t.muted, t.border))
                .children(
                    hints(&tool.annotations)
                        .into_iter()
                        .map(|hint| hint_badge(hint, t)),
                )
                .when_some(tool.name.clone(), |row, name| {
                    row.child(mono(name, Type::MICRO, t.dim))
                }),
        )
        .when_some(tool.description.clone(), |row, description| {
            row.child(
                mono(
                    description.lines().next().unwrap_or_default().to_owned(),
                    Type::MICRO,
                    t.muted,
                )
                .truncate(),
            )
        })
}

/// The editor of a server the page adds: its name, and its entry as
/// the files write it.
fn editor_modal(
    view: &mut ViewCx<'_, McpUi>,
    editor: &Editor,
    user_names: &BTreeSet<String>,
    t: &Theme,
) -> Div {
    let ui = view.ui.clone();
    let (name, entry) = {
        let state = view.read_ui();
        (state.name.clone(), state.entry.clone())
    };
    let user_names = user_names.clone();
    let body = div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(match &editor.editing {
            Some(editing) => mono(editing.clone(), Type::SMALL, t.text),
            None => ui::field(&name, true, t),
        })
        .child(
            div()
                .min_h(px(160.))
                .px(sp(3.))
                .py(sp(2.5))
                .rounded(radius::BOX)
                .well(t)
                .typeset(Type::SMALL.mono())
                .child(entry),
        )
        .when_some(editor.problem.clone(), |body, problem| {
            body.child(mono(problem, Type::CAPTION, t.red))
        });
    ui::modal(
        icon(Icon::Plug, IconSize::LARGE, t.roles.mcp),
        match &editor.editing {
            Some(_) => "Edit server",
            None => "Add server",
        },
        "The entry as mcp.json writes it: `command` and `args` for a \
         program, or `url` and `headers` for HTTP; `exposure`, \
         `toolExposure`, `description`, `timeout`. `${VAR}` reads tau's \
         environment.",
        Some(body.into_any_element()),
        div()
            .flex()
            .gap(sp(2.))
            .child(action(
                "mcp-editor-cancel",
                "Cancel",
                ButtonKind::Secondary,
                &ui,
                t,
                |ui, cx| ui.cancel(cx),
            ))
            .child(action(
                "mcp-editor-save",
                "Save",
                ButtonKind::Primary,
                &ui,
                t,
                move |ui, cx| {
                    ui.save(&user_names, cx);
                },
            )),
        t,
    )
}
