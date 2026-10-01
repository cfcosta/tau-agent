//! Slash commands in the composer. Typing `/` lists what the app can do
//! from there, filtered as the user types: ↑↓ move, Tab completes, Enter
//! runs, Esc closes. `/goal` opens its own popover, with the goal's
//! limits.

use gpui::{
    AnyElement,
    Context,
    Div,
    SharedString,
    Window,
    div,
    prelude::*,
    px,
};

use crate::{
    assets::Icon,
    route::Route,
    theme::{IconSize, Theme, Type, radius, sp},
    ui,
    ui::Material as _,
    workspace::{PickerTarget, Workspace},
};

/// A command the composer runs: tau-ui's own, or a plugin's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// With its slash: `/fork`.
    pub name: String,
    /// What follows the name, as the menu shows it.
    pub args: String,
    pub about: String,
    pub glyph: Icon,
    /// Whether it acts on the open conversation.
    pub needs_run: bool,
    /// The plugin whose command it is.
    pub plugin: Option<&'static str>,
    /// Whether the plugin's command has a popover while it is written.
    pub popover: bool,
}

/// tau-ui's own commands.
fn builtin() -> [Command; 5] {
    let command = |name: &str,
                   about: &'static str,
                   glyph: Icon,
                   needs_run: bool| Command {
        name: name.to_owned(),
        args: String::new(),
        about: about.to_owned(),
        glyph,
        needs_run,
        plugin: None,
        popover: false,
    };
    [
        command(
            "/model",
            "Pick the model for new runs",
            Icon::Settings,
            false,
        ),
        command(
            "/fork",
            "Fork this conversation at its last turn",
            Icon::Fork,
            true,
        ),
        command(
            "/attach",
            "Attach text files to the message",
            Icon::Paperclip,
            false,
        ),
        command(
            "/pr",
            "Open a pull request from this run",
            Icon::PullRequest,
            true,
        ),
        command("/close", "Close this conversation", Icon::Close, true),
    ]
}

/// What the composer's text asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Slash {
    /// Nothing: a message.
    None,
    /// A command being typed: the ones that match.
    Menu(Vec<Command>),
    /// A plugin's command being written, past its name: what follows.
    Writing {
        plugin: &'static str,
        command: String,
        args: String,
    },
}

impl Workspace {
    /// The commands that work here, plugins' first: the ones about a
    /// conversation only when one is open.
    fn commands(&self) -> Vec<Command> {
        let run = self.current().filter(|_| self.route != Route::NewRun);
        let in_run = run.is_some();
        // Only a main chat can be forked.
        let forks = run.is_some_and(|run| self.can_fork(run));
        let plugins = crate::plugins::registry().plugins().flat_map(|plugin| {
            let name = plugin.name();
            self.plugin_commands(plugin.as_ref()).into_iter().map(
                move |command| Command {
                    name: format!("/{}", command.name),
                    args: command.args,
                    about: command.hint,
                    glyph: command.icon,
                    needs_run: false,
                    plugin: Some(name),
                    popover: command.popover,
                },
            )
        });
        plugins
            .chain(builtin())
            .filter(|command| in_run || !command.needs_run)
            .filter(|command| command.name != "/fork" || forks)
            .collect()
    }

    /// What `text` in the composer asks for.
    pub fn slash(&self, text: &str) -> Slash {
        for plugin in crate::plugins::registry().plugins() {
            for command in self
                .plugin_commands(plugin.as_ref())
                .into_iter()
                .filter(|c| c.popover)
            {
                if let Some(rest) =
                    text.strip_prefix(&format!("/{} ", command.name))
                {
                    return Slash::Writing {
                        plugin: plugin.name(),
                        command: command.name,
                        args: rest.trim_start().to_owned(),
                    };
                }
            }
        }
        if !text.starts_with('/') || text.contains(char::is_whitespace) {
            return Slash::None;
        }
        let matches: Vec<Command> = self
            .commands()
            .into_iter()
            .filter(|command| command.name.starts_with(text))
            .collect();
        if matches.is_empty() {
            Slash::None
        } else {
            Slash::Menu(matches)
        }
    }

    /// What the composer's text asks for, unless the popover was
    /// dismissed for it.
    pub fn composer_slash(&self, cx: &gpui::App) -> Slash {
        let text = self.composer.read(cx).text();
        if self.slash_dismissed.as_deref() == Some(text) {
            return Slash::None;
        }
        self.slash(text)
    }

    /// How many entries the open popover lists.
    fn slash_len(&self, cx: &gpui::App) -> usize {
        match self.composer_slash(cx) {
            Slash::None => 0,
            Slash::Menu(commands) => commands.len(),
            Slash::Writing { .. } => 0,
        }
    }

    /// ↑ and ↓ in the composer: move through the popover. Without one,
    /// the key goes on.
    pub fn slash_move(&mut self, delta: i32, cx: &mut Context<Self>) -> bool {
        let len = self.slash_len(cx);
        if len == 0 {
            return false;
        }
        let at = self.slash_selected as i32 + delta;
        self.slash_selected = at.rem_euclid(len as i32) as usize;
        cx.notify();
        true
    }

    /// Tab in the composer: complete the selected command.
    pub fn slash_complete(&mut self, cx: &mut Context<Self>) -> bool {
        let text = match self.composer_slash(cx) {
            Slash::None => return false,
            Slash::Menu(commands) => {
                let Some(command) = commands.get(self.slash_selected) else {
                    return false;
                };
                if command.args.is_empty() {
                    command.name.clone()
                } else {
                    format!("{} ", command.name)
                }
            }
            Slash::Writing { .. } => return false,
        };
        self.slash_selected = 0;
        self.composer
            .update(cx, |input, cx| input.set_text(text, cx));
        cx.notify();
        true
    }

    /// Esc in the composer: close the popover, which stays closed until
    /// the text changes.
    pub fn slash_dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if self.composer_slash(cx) == Slash::None {
            return false;
        }
        self.slash_dismissed = Some(self.composer.read(cx).text().to_owned());
        cx.notify();
        true
    }

    /// The composer's text changed: the popover starts at its top.
    pub(crate) fn composer_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text();
        if self.slash_dismissed.as_deref() != Some(text) {
            self.slash_dismissed = None;
        }
        if self.slash_seen != text {
            self.slash_seen = text.to_owned();
            self.slash_selected = 0;
        }
        cx.notify();
    }

    /// Runs `text` if it is a command, and says whether it was. The
    /// composer has already cleared it.
    pub(crate) fn run_slash(
        &mut self,
        text: &str,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        // A command still being typed runs the one selected.
        if let Slash::Menu(commands) = self.slash(text)
            && self.slash_dismissed.as_deref() != Some(text)
        {
            let command = commands
                .get(self.slash_selected)
                .cloned()
                .unwrap_or_else(|| commands[0].clone());
            self.slash_selected = 0;
            return self.run_command(command, window, cx);
        }
        self.run_plugin_command(text, cx)
    }

    fn run_command(
        &mut self,
        command: Command,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        // A plugin's command with arguments is written first.
        if command.plugin.is_some() {
            if command.args.is_empty() {
                return self.run_plugin_command(&command.name, cx);
            }
            let text = format!("{} ", command.name);
            self.composer
                .update(cx, |input, cx| input.set_text(text, cx));
            cx.notify();
            return true;
        }
        let run = self
            .current()
            .filter(|_| self.route != Route::NewRun)
            .map(|run| run.id.clone());
        match (command.name.as_str(), run) {
            ("/model", _) => match window {
                Some(window) => {
                    self.open_picker(PickerTarget::Next, window, cx)
                }
                None => self.show_picker(PickerTarget::Next, cx),
            },
            ("/attach", _) => self.pick_attachments(cx),
            ("/fork", Some(run)) => {
                let turn = self
                    .run(&run)
                    .map_or(1, |view| Self::last_fork_turn_of(view).max(1));
                self.start_fork_at(&run, turn, cx);
            }
            ("/pr", Some(run)) => self.open_pull_request(&run, cx),
            ("/close", Some(run)) => self.close_run(&run, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// The composer's popover, if its text opens one.
    pub(crate) fn slash_popover(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let body = match self.composer_slash(cx) {
            Slash::None => return None,
            Slash::Menu(commands) => self
                .command_menu(&commands, compact, t, cx)
                .into_any_element(),
            Slash::Writing {
                plugin,
                command,
                args,
            } => self.plugin_popover(plugin, &command, &args, cx)?,
        };
        let popover = div()
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .p(sp(1.5))
            .raised(t)
            .border_1()
            .border_color(t.border_strong)
            .rounded(if compact {
                radius::SHEET
            } else {
                radius::LARGE
            })
            .shadow_lg()
            .child(body);
        Some(if compact {
            popover.into_any_element()
        } else {
            div()
                .absolute()
                .bottom(px(64.))
                .left(sp(6.))
                .w(px(640.))
                .max_w_full()
                .child(popover)
                .into_any_element()
        })
    }

    fn command_menu(
        &self,
        commands: &[Command],
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let rows = commands.iter().enumerate().map(|(n, command)| {
            let selected = n == self.slash_selected;
            let command = command.clone();
            div()
                .id(SharedString::from(format!("command-{}", command.name)))
                .flex()
                .items_center()
                .gap(sp(2.5))
                .h(px(if compact { 48. } else { 38. }))
                .px(sp(2.5))
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .when(selected, |row| row.pressed(t))
                .hover(|style| style.bg(t.selected))
                .child(
                    div()
                        .size(px(if compact { 30. } else { 24. }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::CONTROL)
                        .key(t)
                        .child(ui::icon(
                            command.glyph,
                            IconSize::COMPACT,
                            if selected { t.accent } else { t.muted },
                        )),
                )
                .child(
                    ui::mono(command.name.clone(), Type::SMALL, t.text)
                        .min_w(px(64.))
                        .flex_shrink_0(),
                )
                .when(!compact, |row| {
                    row.child(
                        ui::mono(command.args.clone(), Type::CAPTION, t.dim)
                            .w(px(100.))
                            .flex_shrink_0(),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(if selected {
                            t.text_soft
                        } else {
                            t.muted
                        })
                        .child(command.about.clone()),
                )
                .when(selected && !compact, |row| {
                    row.child(ui::key_hint("Tab", t))
                })
                .on_click(cx.listener(move |ws, _, window, cx| {
                    ws.composer.update(cx, |input, cx| input.clear(cx));
                    ws.run_command(command.clone(), Some(window), cx);
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap(sp(0.25))
            .child(
                div()
                    .px(sp(2.5))
                    .pt(sp(1.5))
                    .pb(sp(2.))
                    .child(ui::heading("Commands", t)),
            )
            .children(rows)
            .when(!compact, |menu| {
                menu.child(ui::hints(
                    &[
                        ("↑↓", "move"),
                        ("Tab", "complete"),
                        ("Enter", "run"),
                        ("Esc", "close"),
                    ],
                    t,
                ))
            })
    }
}
