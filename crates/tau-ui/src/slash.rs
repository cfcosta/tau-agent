//! Slash commands in the composer. Typing `/` lists what the app can do
//! from there, filtered as the user types: ↑↓ move, Tab completes, Enter
//! runs, Esc closes. `/goal` opens its own popover, with the goal's
//! limits and suggestions for the condition.

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
    view::{Item, ToolState},
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

/// A condition the `/goal` popover suggests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub condition: String,
    /// Where it comes from: `last command`, `mutants-triage · met`.
    pub from: String,
}

/// How many suggestions the `/goal` popover lists.
const SUGGESTIONS: usize = 5;

impl Workspace {
    /// The commands that work here: the ones about a conversation only
    /// when one is open.
    fn commands(&self) -> impl Iterator<Item = Command> + '_ {
        let in_run = self.route != Route::NewRun && self.current().is_some();
        COMMANDS
            .into_iter()
            .filter(move |command| in_run || !command.needs_run)
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

    /// Conditions for a goal on the open conversation: what its last
    /// command proves, the repository's checks, the pull request's
    /// checks, and goals set elsewhere. Only those matching `typed`.
    pub fn goal_suggestions(&self, typed: &str) -> Vec<Suggestion> {
        let mut found = Vec::new();
        let run = self.current().filter(|_| self.route != Route::NewRun);
        if let Some(run) = run {
            let commands: Vec<&str> = run
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Tool(card)
                        if card.tool == "bash"
                            && matches!(card.state, ToolState::Done { .. }) =>
                    {
                        Some(card.summary.as_str())
                    }
                    _ => None,
                })
                .collect();
            if let Some(last) = commands.last() {
                found.push(Suggestion {
                    condition: format!("{last} passes"),
                    from: "last command".into(),
                });
            }
            if commands.iter().any(|command| command.starts_with("cargo")) {
                found.push(Suggestion {
                    condition: "cargo clippy --all-targets has no warnings"
                        .into(),
                    from: "repository check".into(),
                });
            }
            if self.pull_request(&run.id).is_some() {
                found.push(Suggestion {
                    condition: "The pull request's checks are green".into(),
                    from: "GitHub".into(),
                });
            }
        }
        for other in &self.runs {
            let Some(goal) = &other.goal else { continue };
            if run.is_some_and(|run| run.id == other.id)
                || found.iter().any(|seen| seen.condition == goal.condition)
            {
                continue;
            }
            let state = match goal.status {
                tau_goal::Status::Met => "met",
                tau_goal::Status::Stopped(_) => "stopped",
                tau_goal::Status::Paused => "paused",
                tau_goal::Status::Active => "working",
            };
            found.push(Suggestion {
                condition: goal.condition.clone(),
                from: format!("{} · {state}", other.title),
            });
        }
        let typed = typed.trim().to_lowercase();
        found.retain(|suggestion| {
            typed
                .split_whitespace()
                .all(|word| suggestion.condition.to_lowercase().contains(word))
        });
        found.truncate(SUGGESTIONS);
        found
    }

    /// How many entries the open popover lists.
    fn slash_len(&self, cx: &gpui::App) -> usize {
        match self.composer_slash(cx) {
            Slash::None => 0,
            Slash::Menu(commands) => commands.len(),
            Slash::Goal(typed) => self.goal_suggestions(&typed).len(),
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

    /// Tab in the composer: complete the selected command, or take the
    /// selected suggestion as the goal.
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
            Slash::Goal(typed) => {
                let suggestions = self.goal_suggestions(&typed);
                let Some(pick) = suggestions.get(self.slash_selected) else {
                    return false;
                };
                format!("/goal {}", pick.condition)
            }
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
        // `/goal ` with nothing after it takes the selected suggestion
        // (the composer trims what it sends, so look at what it showed).
        let shown = self.slash_seen.clone();
        if let Slash::Goal(typed) = self.slash(&shown)
            && typed.trim().is_empty()
            && text.trim() == "/goal"
        {
            let picked = self
                .goal_suggestions("")
                .get(self.slash_selected)
                .map(|pick| pick.condition.clone());
            self.slash_selected = 0;
            match picked {
                Some(condition) => self.set_goal(
                    tau_goal::Command::Set {
                        condition,
                        continuations: tau_goal::DEFAULT_CONTINUATIONS,
                        budget: tau_goal::DEFAULT_BUDGET,
                    },
                    false,
                    cx,
                ),
                None => self
                    .composer
                    .update(cx, |input, cx| input.set_text("/goal ", cx)),
            }
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
            .map(|run| (run.id.clone(), run.status.is_live()));
        let tau_goal::Command::Set {
            condition,
            continuations,
            budget,
        } = command
        else {
            if let Some((run, _)) = run {
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
            // A run going on takes it at its next stop, and the model is
            // told.
            Some((run, true)) => {
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
            Slash::Goal(typed) => self.goal_popover(&typed, compact, t, cx),
        };
        let popover = div()
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .p(sp(1.5))
            .bg(t.panel)
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
                .when(selected, |row| row.bg(t.selected))
                .hover(|style| style.bg(t.selected))
                .child(
                    div()
                        .size(px(if compact { 30. } else { 24. }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::CONTROL)
                        .bg(t.raised)
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

    fn goal_popover(
        &self,
        typed: &str,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let suggestions = self.goal_suggestions(typed);
        let jev = self.catalog.models.access.jev;
        let rows = suggestions.into_iter().enumerate().map(|(n, pick)| {
            let selected = n == self.slash_selected;
            let condition = pick.condition.clone();
            div()
                .id(("goal-suggestion", n))
                .flex()
                .items_center()
                .gap(sp(2.5))
                .min_h(px(if compact { 44. } else { 34. }))
                .px(sp(2.5))
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .when(selected, |row| row.bg(t.selected))
                .hover(|style| style.bg(t.selected))
                .child(ui::icon(
                    Icon::Target,
                    IconSize::COMPACT,
                    if selected { t.accent } else { t.dim },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(if selected { t.text } else { t.text_soft })
                        .child(pick.condition),
                )
                .when(!compact, |row| {
                    row.child(ui::mono(pick.from, Type::MICRO, t.dim))
                })
                .on_click(cx.listener(move |ws, _, _, cx| {
                    let text = format!("/goal {condition}");
                    ws.composer
                        .update(cx, |input, cx| input.set_text(text, cx));
                }))
        });
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
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.bg)
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
            .when(rows.len() > 0, |popover| {
                popover
                    .child(
                        div()
                            .px(sp(2.5))
                            .pt(sp(2.))
                            .pb(sp(1.))
                            .child(ui::heading("Suggestions", t)),
                    )
                    .children(rows)
            })
            .when(!compact, |popover| {
                popover.child(hints(
                    &[
                        ("Tab", "use suggestion"),
                        ("Enter", "set the goal and start"),
                    ],
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
