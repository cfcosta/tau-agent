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
use tau_agent::tool::RunId;

use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
    ui,
    ui::Material as _,
    workspace::{PickerTarget, Workspace, WorkspaceEvent},
};

/// A command the composer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub name: &'static str,
    /// What follows the name, as the menu shows it.
    pub args: &'static str,
    pub about: &'static str,
    pub glyph: Icon,
    /// Whether it acts on the open conversation.
    pub needs_run: bool,
}

pub const COMMANDS: [Command; 6] = [
    Command {
        name: "/goal",
        args: "<condition>",
        about: "Keep working until a condition holds",
        glyph: Icon::Target,
        needs_run: false,
    },
    Command {
        name: "/model",
        args: "",
        about: "Pick the model for new runs",
        glyph: Icon::Settings,
        needs_run: false,
    },
    Command {
        name: "/fork",
        args: "",
        about: "Fork this conversation at its last turn",
        glyph: Icon::Fork,
        needs_run: true,
    },
    Command {
        name: "/attach",
        args: "",
        about: "Attach text files to the message",
        glyph: Icon::Paperclip,
        needs_run: false,
    },
    Command {
        name: "/pr",
        args: "",
        about: "Open a pull request from this run",
        glyph: Icon::PullRequest,
        needs_run: true,
    },
    Command {
        name: "/close",
        args: "",
        about: "Close this conversation",
        glyph: Icon::Close,
        needs_run: true,
    },
];

/// What the composer's text asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Slash {
    /// Nothing: a message.
    None,
    /// A command being typed: the ones that match.
    Menu(Vec<Command>),
    /// A goal being written: what follows `/goal `.
    Goal(String),
}

impl Slash {
    pub fn is_goal(&self) -> bool {
        matches!(self, Self::Goal(_))
    }
}

impl Workspace {
    /// The commands that work here: the ones about a conversation only
    /// when one is open.
    fn commands(&self) -> impl Iterator<Item = Command> + '_ {
        let run = self.current().filter(|_| self.route != Route::NewRun);
        let in_run = run.is_some();
        // Only a main chat can be forked.
        let forks = run.is_some_and(|run| self.can_fork(run));
        COMMANDS
            .into_iter()
            .filter(move |command| in_run || !command.needs_run)
            .filter(move |command| command.name != "/fork" || forks)
    }

    /// What `text` in the composer asks for.
    pub fn slash(&self, text: &str) -> Slash {
        if let Some(rest) = text.strip_prefix("/goal ") {
            return Slash::Goal(rest.trim_start().to_owned());
        }
        if !text.starts_with('/') || text.contains(char::is_whitespace) {
            return Slash::None;
        }
        let matches: Vec<Command> = self
            .commands()
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
            Slash::Goal(_) => 0,
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
                    command.name.to_owned()
                } else {
                    format!("{} ", command.name)
                }
            }
            Slash::Goal(_) => return false,
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
        // `/goal` alone is not a goal yet: it stays, to be written.
        if text.trim() == "/goal" && self.slash(&self.slash_seen).is_goal() {
            self.composer
                .update(cx, |input, cx| input.set_text("/goal ", cx));
            return true;
        }
        // A command still being typed runs the one selected.
        if let Slash::Menu(commands) = self.slash(text)
            && self.slash_dismissed.as_deref() != Some(text)
        {
            let command = commands
                .get(self.slash_selected)
                .copied()
                .unwrap_or(commands[0]);
            self.slash_selected = 0;
            return self.run_command(command, window, cx);
        }
        if self.run_plugin_command(text, cx) {
            return true;
        }
        if let Some(command) = tau_goal::Command::parse(text) {
            // Limits typed with the command win over the popover's.
            let typed =
                text.contains("--continuations") || text.contains("--budget");
            self.set_goal(command, typed, cx);
            return true;
        }
        false
    }

    fn run_command(
        &mut self,
        command: Command,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        let run = self
            .current()
            .filter(|_| self.route != Route::NewRun)
            .map(|run| run.id.clone());
        match (command.name, run) {
            ("/goal", _) => self
                .composer
                .update(cx, |input, cx| input.set_text("/goal ", cx)),
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

    /// `/goal ...`: sets the open conversation's goal, or a new run's,
    /// with the limits from the popover unless `typed` says the command
    /// gave its own. `/goal clear` clears it.
    fn set_goal(
        &mut self,
        command: tau_goal::Command,
        typed: bool,
        cx: &mut Context<Self>,
    ) {
        let run = self
            .current()
            .filter(|_| self.route != Route::NewRun)
            .map(|run| (run.id.clone(), run.status.is_live(), run.goal_checks));
        let tau_goal::Command::Set {
            condition,
            continuations,
            budget,
        } = command
        else {
            if let Some((run, _, _)) = run {
                self.goal_control(&run, tau_goal::Record::Cleared, cx);
            }
            return;
        };
        if !self.catalog.models.access.jev {
            self.show_alert(
                "Goals need Jev",
                "tau-goal checks goals with Jev. Add a TypeSafe key on the \
                 Models screen, then set the goal again.",
                cx,
            );
            self.composer.update(cx, |input, cx| {
                input.set_text(format!("/goal {condition}"), cx)
            });
            return;
        }
        let (continuations, budget) = if typed {
            (continuations, budget)
        } else {
            self.goal_limits(cx)
        };
        let text = format!(
            "/goal --continuations {continuations} --budget {budget:.2} \
             {condition}"
        );
        match run {
            // A run going on without tau-goal, started before the key or
            // as a sub-agent: the goal is kept for when it goes on, and
            // nothing tells the model it is checked now.
            Some((run, true, false)) => {
                self.goal_control(
                    &run,
                    tau_goal::Record::Set {
                        goal: condition.clone(),
                        continuations,
                        budget,
                    },
                    cx,
                );
                self.show_alert(
                    "The goal is checked from the next message",
                    "This run started without tau-goal, so nothing checks \
                     the goal while it goes on. It is kept, and checked once \
                     the conversation goes on.",
                    cx,
                );
            }
            // A run going on takes it at its next stop, and the model is
            // told.
            Some((run, true, true)) => {
                self.goal_control(
                    &run,
                    tau_goal::Record::Set {
                        goal: condition.clone(),
                        continuations,
                        budget,
                    },
                    cx,
                );
                cx.emit(WorkspaceEvent::Steer {
                    run,
                    text: tau_goal::set_input(&condition),
                });
            }
            _ => self.send(text, cx),
        }
    }

    /// The limits in the `/goal` popover, or the defaults where a field
    /// does not read as a number.
    pub fn goal_limits(&self, cx: &gpui::App) -> (u32, f64) {
        let continuations = self
            .goal_continuations
            .read(cx)
            .text()
            .trim()
            .parse()
            .unwrap_or(tau_goal::DEFAULT_CONTINUATIONS);
        let budget = self
            .goal_budget
            .read(cx)
            .text()
            .trim()
            .trim_start_matches('$')
            .parse()
            .unwrap_or(tau_goal::DEFAULT_BUDGET);
        (continuations, budget)
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
            Slash::Menu(commands) => {
                self.command_menu(&commands, compact, t, cx)
            }
            Slash::Goal(_) => self.goal_popover(compact, t),
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
            let command = *command;
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
                    ui::mono(command.name, Type::SMALL, t.text)
                        .w(px(64.))
                        .flex_shrink_0(),
                )
                .when(!compact, |row| {
                    row.child(
                        ui::mono(command.args, Type::CAPTION, t.dim)
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
                        .child(command.about),
                )
                .when(selected && !compact, |row| row.child(key("Tab", t)))
                .on_click(cx.listener(move |ws, _, window, cx| {
                    ws.composer.update(cx, |input, cx| input.clear(cx));
                    ws.run_command(command, Some(window), cx);
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
                menu.child(hints(
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

    fn goal_popover(&self, compact: bool, t: &Theme) -> Div {
        let jev = self.catalog.models.access.jev;
        let limit = |label: &'static str,
                     input: &gpui::Entity<crate::input::TextInput>,
                     unit: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .child(label)
                .child(
                    div()
                        .w(px(64.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .px(sp(2.))
                        .rounded(radius::CONTROL)
                        .well(t)
                        .typeset(Type::CAPTION.mono())
                        .text_color(t.text)
                        .child(input.clone()),
                )
                .when(!unit.is_empty(), |row| row.child(unit))
        };
        div()
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(1.5))
                    .px(sp(2.5))
                    .pt(sp(2.))
                    .pb(sp(2.5))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(2.))
                            .child(ui::icon(
                                Icon::Target,
                                IconSize::BASE,
                                t.accent,
                            ))
                            .child(ui::mono("/goal", Type::SMALL, t.accent))
                            .child(ui::mono(
                                "<condition>",
                                Type::CAPTION,
                                t.dim,
                            )),
                    )
                    .child(ui::text(
                        if jev {
                            "The agent keeps going until this holds. Each time \
                             it would stop, tau-goal asks Jev whether the goal \
                             is met; if not, the agent goes on."
                        } else {
                            "Goals are checked with Jev: add a TypeSafe key on \
                             the Models screen first."
                        },
                        Type::CAPTION,
                        if jev { t.muted } else { t.accent },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(4.))
                    .px(sp(2.5))
                    .py(sp(2.))
                    .border_t_1()
                    .border_b_1()
                    .border_color(t.border)
                    .child(limit(
                        "Stop after",
                        &self.goal_continuations,
                        "continuations",
                    ))
                    .child(limit("Budget $", &self.goal_budget, "")),
            )
            .when(!compact, |popover| {
                popover.child(hints(
                    &[("Enter", "set the goal and start"), ("Esc", "close")],
                    t,
                ))
            })
    }

    /// Stores a change to `run`'s goal, and shows it at once.
    pub fn goal_control(
        &mut self,
        run: &RunId,
        record: tau_goal::Record,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.runs.iter_mut().find(|view| &view.id == run) {
            view.apply_goal(&record);
        }
        cx.emit(WorkspaceEvent::Goal {
            run: run.clone(),
            record,
        });
        cx.notify();
    }
}

/// A key, as the popovers' hints show it.
fn key(label: &'static str, t: &Theme) -> Div {
    ui::mono(label, Type::MICRO, t.dim)
        .px(sp(1.25))
        .py(sp(0.25))
        .border_1()
        .border_color(t.border)
        .rounded(radius::SMALL)
}

/// The keys a popover takes, in a row at its foot.
fn hints(keys: &[(&'static str, &'static str)], t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(3.5))
        .px(sp(2.5))
        .pt(sp(2.))
        .pb(sp(1.))
        .mt(sp(1.))
        .border_t_1()
        .border_color(t.border)
        .children(keys.iter().map(|(label, does)| {
            div()
                .flex()
                .items_center()
                .gap(sp(1.5))
                .child(key(label, t))
                .child(ui::text(*does, Type::CAPTION, t.dim))
        }))
}
