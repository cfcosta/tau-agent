//! The Servers page: a repository's MCP servers (or the user's alone),
//! where each comes from, how it is reached and exposed, whether it is
//! connected and its tools; the repository servers that wait for
//! approval; entries that were skipped; and the editor for the servers
//! the page adds.

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
    PendingRow,
    ServerRow,
    Servers,
    ToolRow,
    server_entry,
};
use crate::connection::Annotations;

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

    pub fn enable(
        &mut self,
        name: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.handle.act(
            Act::Enable {
                name: name.to_owned(),
                enabled,
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
        .and_then(|repo| Some((repo.to_owned(), view.repos.get(repo)?)))
    {
        Some((repo, servers)) => (Some(repo), servers),
        None => (None, view.data),
    }
}

/// The page.
pub fn render(view: &mut ViewCx<'_, McpUi>) -> AnyElement {
    let t = view.theme().clone();
    let compact = view.compact;
    let (repo, servers) = servers_of(view);
    let servers = servers.clone();
    let ui = view.ui.clone();
    let removing = view.read_ui().removing.clone();
    let content = div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(header(&ui, repo.as_deref(), &servers, compact, &t))
        .when(!servers.pending.is_empty(), |page| {
            page.child(pending(&ui, repo.as_deref(), &servers.pending, &t))
        })
        .children(servers.errors.iter().map(|error| {
            ui::notice(Icon::Warning, error.clone(), t.red, Type::SMALL, &t)
        }))
        .when(!servers.started && !servers.servers.is_empty(), |page| {
            let repo = repo.clone();
            page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .child(
                        ui::notice(
                            Icon::Info,
                            if repo.is_some() {
                                "Not connected yet: the servers start with the \
                                 first run in this repository."
                            } else {
                                "Not connected yet: runs start their \
                                 repository's servers."
                            },
                            t.blue,
                            Type::SMALL,
                            &t,
                        )
                        .flex_1(),
                    )
                    .child(action(
                        "mcp-connect",
                        "Connect now",
                        ButtonKind::Secondary,
                        &ui,
                        &t,
                        move |ui, cx| ui.reconnect(repo.as_deref(), None, cx),
                    )),
            )
        })
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
                .gap(sp(3.))
                .children(servers.servers.iter().map(|server| {
                    server_card(
                        &ui,
                        repo.as_deref(),
                        server,
                        removing.as_deref() == Some(server.name.as_str()),
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

/// The title, whose servers these are, and what can be done.
fn header(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    servers: &Servers,
    compact: bool,
    t: &Theme,
) -> Div {
    let files = [servers.user_file.as_deref(), servers.repo_file.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" and ");
    div()
        .flex()
        .items_start()
        .gap(sp(4.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.5))
                        .child(
                            div()
                                .typeset(Type::TITLE)
                                .font_weight(weight::STRONG)
                                .child("MCP servers"),
                        )
                        .child(ui::tag(
                            repo.unwrap_or("your servers").to_owned(),
                            Type::CAPTION,
                            t.text_soft,
                            t,
                        ))
                        .child(mono(servers.summary(), Type::CAPTION, t.dim)),
                )
                .when(!compact, |title| {
                    title.child(ui::text(
                        format!(
                            "Servers whose tools runs get, read from {files} \
                             and the servers added here. A repository's own \
                             servers wait for your approval."
                        ),
                        Type::BODY,
                        t.muted,
                    ))
                }),
        )
        .child(action(
            "mcp-add",
            "Add server",
            ButtonKind::Primary,
            ui,
            t,
            |ui, cx| ui.open_add(cx),
        ))
}

/// The repository servers waiting for approval, each with its whole
/// entry: approving runs what it says.
fn pending(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    pending: &[PendingRow],
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(heading(
            &format!("Waiting for approval · {}", pending.len()),
            t,
        ))
        .children(pending.iter().map(|row| {
            let (repo, row) = (repo.map(str::to_owned), row.clone());
            let entry =
                serde_json::to_string_pretty(&row.entry).unwrap_or_default();
            ui::card(t)
                .p(sp(3.5))
                .gap(sp(2.5))
                .border_color(t.accent_border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .child(icon(Icon::Lock, IconSize::COMPACT, t.accent))
                        .child(mono(row.name.clone(), Type::SMALL, t.text))
                        .child(
                            mono(row.transport.clone(), Type::CAPTION, t.dim)
                                .flex_1()
                                .min_w(px(0.))
                                .truncate(),
                        )
                        .when_some(repo, |line, repo| {
                            let id = format!("mcp-approve-{}", row.name);
                            let row = row.clone();
                            line.child(action(
                                id,
                                "Approve",
                                ButtonKind::Primary,
                                ui,
                                t,
                                move |ui, cx| ui.approve(&repo, &row, cx),
                            ))
                        }),
                )
                .child(ui::text(
                    "The repository's .tau/mcp.json starts this server. It \
                     connects once approved, and asks again if the entry \
                     changes.",
                    Type::CAPTION,
                    t.muted,
                ))
                .child(ui::code_block(Some("json"), &entry, t))
        }))
}

/// A state's color.
fn state_color(state: Option<&str>, t: &Theme) -> gpui::Hsla {
    match state {
        Some("connected") => t.green,
        Some("connecting") => t.accent,
        Some("failed") => t.red,
        _ => t.dim,
    }
}

/// One server: its name, where it comes from, how it is reached and
/// exposed, its state and last error, its tools, and what can be done.
fn server_card(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    server: &ServerRow,
    removing: bool,
    compact: bool,
    t: &Theme,
) -> Div {
    let name = server.name.clone();
    let state = server.state.clone().unwrap_or_else(|| "not started".into());
    let actions = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .when(server.state.is_some() && server.enabled, |row| {
            let (repo, name) = (repo.map(str::to_owned), name.clone());
            row.child(action(
                format!("mcp-reconnect-{name}"),
                "Reconnect",
                ButtonKind::Secondary,
                ui,
                t,
                move |ui, cx| ui.reconnect(repo.as_deref(), Some(&name), cx),
            ))
        })
        .when(server.defined == Defined::Settings, |row| {
            let entry = server.entry.clone().unwrap_or(Value::Null);
            let (edit, toggle, remove) =
                (name.clone(), name.clone(), name.clone());
            let enabled = server.enabled;
            row.child(
                div()
                    .id(SharedString::from(format!("mcp-enable-{name}")))
                    .child(ui::switch(enabled, t))
                    .on_click(on_ui(ui, move |ui, cx| {
                        ui.enable(&toggle, !enabled, cx)
                    })),
            )
            .child(action(
                format!("mcp-edit-{name}"),
                "Edit",
                ButtonKind::Secondary,
                ui,
                t,
                move |ui, cx| ui.open_edit(&edit, &entry, cx),
            ))
            .child(action(
                format!("mcp-remove-{name}"),
                if removing { "Remove it" } else { "Remove" },
                ButtonKind::Danger,
                ui,
                t,
                move |ui, cx| ui.remove(&remove, cx),
            ))
        });
    ui::card(t)
        .p(sp(3.5))
        .gap(sp(2.5))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.))
                .child(ui::dot(state_color(server.state.as_deref(), t), 8.))
                .child(mono(server.name.clone(), Type::SMALL, t.text))
                .child(ui::badge(server.defined.label(), t.muted, t.border))
                .child(ui::badge(
                    server.exposure.clone(),
                    t.blue,
                    t.blue_border,
                ))
                .child(mono(state, Type::CAPTION, t.dim))
                .child(div().flex_1())
                .when(!compact, |row| row.child(actions)),
        )
        .child(mono(server.transport.clone(), Type::CAPTION, t.dim).truncate())
        .when_some(server.description.clone(), |card, description| {
            card.child(ui::text(description, Type::SMALL, t.muted))
        })
        .when_some(server.error.clone(), |card, error| {
            card.child(mono(error, Type::CAPTION, t.red))
        })
        .when(!server.tools.is_empty(), |card| {
            card.child(
                div()
                    .flex()
                    .flex_col()
                    .child(heading(
                        &format!("Tools · {}", server.tools.len()),
                        t,
                    ))
                    .children(
                        server.tools.iter().map(|tool| tool_row(tool, t)),
                    ),
            )
        })
        .when(compact, |card| {
            card.child(
                div().flex().flex_wrap().child(server_actions_compact(
                    ui, repo, server, removing, t,
                )),
            )
        })
}

/// A server's actions on a phone, under it.
fn server_actions_compact(
    ui: &Entity<Ui>,
    repo: Option<&str>,
    server: &ServerRow,
    removing: bool,
    t: &Theme,
) -> Div {
    let name = server.name.clone();
    div()
        .flex()
        .flex_wrap()
        .gap(sp(2.))
        .when(server.state.is_some() && server.enabled, |row| {
            let (repo, name) = (repo.map(str::to_owned), name.clone());
            row.child(action(
                format!("mcp-reconnect-{name}"),
                "Reconnect",
                ButtonKind::Secondary,
                ui,
                t,
                move |ui, cx| ui.reconnect(repo.as_deref(), Some(&name), cx),
            ))
        })
        .when(server.defined == Defined::Settings, |row| {
            let entry = server.entry.clone().unwrap_or(Value::Null);
            let enabled = server.enabled;
            let (edit, toggle, remove) =
                (name.clone(), name.clone(), name.clone());
            row.child(action(
                format!("mcp-enable-{name}"),
                if enabled { "Turn off" } else { "Turn on" },
                ButtonKind::Secondary,
                ui,
                t,
                move |ui, cx| ui.enable(&toggle, !enabled, cx),
            ))
            .child(action(
                format!("mcp-edit-{name}"),
                "Edit",
                ButtonKind::Secondary,
                ui,
                t,
                move |ui, cx| ui.open_edit(&edit, &entry, cx),
            ))
            .child(action(
                format!("mcp-remove-{name}"),
                if removing { "Remove it" } else { "Remove" },
                ButtonKind::Danger,
                ui,
                t,
                move |ui, cx| ui.remove(&remove, cx),
            ))
        })
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
        icon(Icon::Plug, IconSize::LARGE, t.accent),
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
