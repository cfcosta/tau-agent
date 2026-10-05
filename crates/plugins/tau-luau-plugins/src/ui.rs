//! tau-luau-plugins' UI (ADR 0017, 0027): the records folded into each
//! run's state, the view trees Luau plugins return drawn with tau-ui-kit's
//! pieces, the run's status line, the cards of the plugins' own tools,
//! and a badge on calls a plugin flagged.
//!
//! The drawing reads JSON only, so the phone, which runs no Luau, draws
//! what the computer does.

use std::collections::BTreeMap;

use gpui::{
    AnyElement,
    IntoElement,
    ParentElement,
    Styled,
    div,
    prelude::*,
    relative,
    rems,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_kit::{
    assets::Icon,
    components::{
        ButtonKind,
        badge,
        button,
        card,
        code_block,
        dot,
        empty,
        heading,
        mono,
    },
    prose::rich,
    theme::{Design as _, Theme, Tone, Type, radius, sp},
};
use tau_ui_plugin::{
    Fold,
    Manifest,
    Page,
    RunCx,
    RunInfo,
    SlashCommand,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, CardView},
};

use crate::{
    Act,
    LuauSettings,
    NAME,
    Overview,
    Record,
    SettingsPage,
    Standing,
    pane::{Bind, SettingsUi},
};

/// The page that lists the plugins repository's plugins.
pub const PAGE: &str = "luau-plugins";

/// How deep a view tree may nest, and how many pieces a list or a row
/// may hold, before the rest is left out.
const MAX_DEPTH: usize = 12;
const MAX_CHILDREN: usize = 200;
/// Log lines a run keeps per plugin, the newest.
const MAX_LOGS: usize = 200;

/// tau-luau-plugins with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct LuauPluginsUi;

/// What a run's Luau plugins left, by plugin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub plugins: BTreeMap<String, PluginState>,
}

/// One plugin's part in a run, as its records leave it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginState {
    /// What its `view` drew last: `{ status?, note?, card?, page? }`.
    pub view: Value,
    pub state: Value,
    /// Hooks that failed, and why, oldest first.
    pub errors: Vec<(String, String)>,
    /// Why it is off for the rest of the run, once it is.
    pub off: Option<String>,
    /// Calls it flagged, and why, by call id.
    pub flags: BTreeMap<String, String>,
    pub logs: Vec<String>,
}

impl Fold for State {
    type Record = Record;

    fn apply(&mut self, record: Record, _run: &mut dyn RunCx) {
        match record {
            Record::State { plugin, state } => self.of(plugin).state = state,
            Record::View { plugin, view } => self.of(plugin).view = view,
            Record::Log { plugin, lines } => {
                let logs = &mut self.of(plugin).logs;
                logs.extend(lines);
                let over = logs.len().saturating_sub(MAX_LOGS);
                logs.drain(..over);
            }
            Record::Error {
                plugin,
                hook,
                error,
            } => self.of(plugin).errors.push((hook, error)),
            Record::Flag {
                plugin,
                call_id,
                reason,
            } => {
                self.of(plugin).flags.insert(call_id, reason);
            }
            Record::Off { plugin, why } => self.of(plugin).off = Some(why),
        }
    }
}

impl State {
    fn of(&mut self, plugin: String) -> &mut PluginState {
        self.plugins.entry(plugin).or_default()
    }

    /// The run's line in the plugin list: each plugin's status, or that
    /// it is off or failing.
    pub fn status(&self) -> Option<String> {
        let lines: Vec<String> = self
            .plugins
            .iter()
            .map(|(name, plugin)| {
                if let Some(why) = &plugin.off {
                    return format!("{name}: off · {why}");
                }
                let status = &plugin.view["status"];
                let text = status["text"].as_str().unwrap_or("on");
                let mut line = format!("{name}: {text}");
                if let Some(detail) = status["detail"].as_str() {
                    line.push_str(&format!(" · {detail}"));
                }
                if !plugin.errors.is_empty() {
                    line.push_str(&format!(
                        " · {} failed",
                        plugin.errors.len()
                    ));
                }
                line
            })
            .collect();
        (!lines.is_empty()).then(|| lines.join("; "))
    }
}

/// A piece's tone, as the theme has it.
fn tone(of: &Value) -> Tone {
    match of.as_str() {
        Some("good") => Tone::Good,
        Some("warn") => Tone::Warn,
        Some("bad") => Tone::Danger,
        Some("info") => Tone::Info,
        _ => Tone::Quiet,
    }
}

/// Draws a view tree a Luau plugin returned. A piece it does not know,
/// or past [`MAX_DEPTH`], draws as a red box that says so.
pub fn draw(piece: &Value, t: &Theme) -> AnyElement {
    draw_at(piece, t, 0, None)
}

/// Draws a settings page: its bound pieces read and save the settings
/// through `bind` (ADR 0029).
pub fn draw_bound(piece: &Value, t: &Theme, bind: &Bind) -> AnyElement {
    draw_at(piece, t, 0, Some(bind))
}

fn text(piece: &Value, key: &str) -> String {
    piece[key].as_str().unwrap_or_default().to_owned()
}

fn children(
    piece: &Value,
    key: &str,
    t: &Theme,
    depth: usize,
    bind: Option<&Bind>,
) -> Vec<AnyElement> {
    piece[key]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_CHILDREN)
        .map(|child| draw_at(child, t, depth + 1, bind))
        .collect()
}

fn unknown(what: String, t: &Theme) -> AnyElement {
    div()
        .px(sp(2.))
        .py(sp(1.))
        .rounded(radius::SMALL)
        .border_1()
        .border_color(t.red_border)
        .typeset(Type::CAPTION)
        .text_color(t.red)
        .child(what)
        .into_any_element()
}

fn draw_at(
    piece: &Value,
    t: &Theme,
    depth: usize,
    bind: Option<&Bind>,
) -> AnyElement {
    if depth > MAX_DEPTH {
        return unknown("the view nests too deep".into(), t);
    }
    if let Some(plain) = piece.as_str() {
        return div()
            .typeset(Type::SMALL)
            .text_color(t.text_soft)
            .child(plain.to_owned())
            .into_any_element();
    }
    match piece["piece"].as_str().unwrap_or_default() {
        "text" => div()
            .typeset(Type::SMALL)
            .text_color(t.text_soft)
            .child(text(piece, "text"))
            .into_any_element(),
        "rich" => div()
            .typeset(Type::SMALL)
            .child(rich(&text(piece, "text"), t.text_soft, t))
            .into_any_element(),
        "mono" => mono(text(piece, "text"), Type::CAPTION, t.text_soft)
            .into_any_element(),
        "badge" => {
            let color = t.tone(tone(&piece["tone"]));
            badge(text(piece, "text"), color, color.opacity(0.4))
                .into_any_element()
        }
        "status" => {
            let color = t.tone(tone(&piece["tone"]));
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(dot(color, 7.))
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.text)
                        .child(text(piece, "text")),
                )
                .when_some(piece["detail"].as_str(), |row, detail| {
                    row.child(mono(detail.to_owned(), Type::CAPTION, t.dim))
                })
                .into_any_element()
        }
        "rows" => div()
            .flex()
            .flex_col()
            .gap(sp(1.))
            .children(
                piece["rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(MAX_CHILDREN)
                    .map(|row| {
                        div()
                            .flex()
                            .gap(sp(3.))
                            .child(
                                mono(text(row, "key"), Type::CAPTION, t.dim)
                                    .w(rems(8.))
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .typeset(Type::SMALL)
                                    .text_color(t.text_soft)
                                    .child(text(row, "value")),
                            )
                    }),
            )
            .into_any_element(),
        "list" => div()
            .flex()
            .flex_col()
            .gap(sp(1.))
            .children(children(piece, "items", t, depth, bind).into_iter().map(
                |item| {
                    div()
                        .flex()
                        .gap(sp(2.))
                        .child(div().text_color(t.dim).child("•"))
                        .child(div().flex_1().min_w(rems(0.)).child(item))
                },
            ))
            .into_any_element(),
        "progress" => {
            let share =
                piece["share"].as_f64().unwrap_or(0.).clamp(0., 1.) as f32;
            div()
                .flex()
                .flex_col()
                .gap(sp(1.))
                .when_some(piece["label"].as_str(), |column, label| {
                    column.child(mono(label.to_owned(), Type::CAPTION, t.dim))
                })
                .child(
                    div()
                        .w_full()
                        .h(rems(0.375))
                        .rounded(radius::SMALL)
                        .bg(t.raised)
                        .child(
                            div()
                                .h_full()
                                .w(relative(share))
                                .rounded(radius::SMALL)
                                .bg(t.accent),
                        ),
                )
                .into_any_element()
        }
        "code" => code_block(piece["lang"].as_str(), &text(piece, "text"), t)
            .into_any_element(),
        "stack" => div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .children(children(piece, "children", t, depth, bind))
            .into_any_element(),
        "row" => div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(2.))
            .children(children(piece, "children", t, depth, bind))
            .into_any_element(),
        // Buttons send their action once actions arrive (ADR 0027, phase 2).
        "button" => badge(text(piece, "label"), t.muted, t.border_strong)
            .into_any_element(),
        "toggle" | "choice" | "field" => match bind {
            Some(bind) => crate::pane::bound(piece, t, bind),
            None => {
                unknown("a setting, drawn on a settings page only".into(), t)
            }
        },
        other => unknown(format!("unknown piece `{other}`"), t),
    }
}

impl UiPlugin for LuauPluginsUi {
    type State = State;
    type Data = Overview;
    type RepoData = ();
    type Settings = LuauSettings;
    type Ui = SettingsUi;

    fn name(&self) -> &'static str {
        NAME
    }

    /// A settings page the host drew.
    fn reply(
        &self,
        ui: &mut SettingsUi,
        reply: Value,
        cx: &mut gpui::Context<SettingsUi>,
    ) {
        if let Ok(page) = serde_json::from_value::<SettingsPage>(reply) {
            ui.take_page(page, cx);
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(Page::new(PAGE, page).title(|_| "Luau plugins".to_owned()))
            .settings(crate::pane::render)
            .command(
                SlashCommand::new(
                    "plugin",
                    "Write or change a plugin, in a chat of its own",
                    start_plugin_chat,
                )
                .args("<what it should do>")
                .icon(Icon::Plug),
            )
            .status(State::status)
            .contribute(points::CARD, |at: &AtCard, view| {
                let details = at.data.result.as_ref()?.details.as_ref()?;
                details.get("plugin")?.as_str()?;
                let card =
                    details.get("card").filter(|card| !card.is_null())?;
                let t = view.theme().clone();
                Some(CardView {
                    body: Some(
                        div()
                            .px(sp(3.5))
                            .py(sp(2.5))
                            .child(draw(card, &t))
                            .into_any_element(),
                    ),
                    ..CardView::default()
                })
            })
            .contribute(points::CARD_BADGE, |at: &AtCard, view| {
                let state: &State = view.state?;
                let (plugin, reason) =
                    state.plugins.iter().find_map(|(name, plugin)| {
                        plugin
                            .flags
                            .get(&at.call_id)
                            .map(|reason| (name.clone(), reason.clone()))
                    })?;
                let t = view.theme().clone();
                Some(
                    badge(
                        format!("flagged by {plugin}: {reason}"),
                        t.tone(Tone::Warn),
                        t.tone(Tone::Warn).opacity(0.4),
                    )
                    .into_any_element(),
                )
            })
    }
}

/// The plugins page: each plugin of the plugins repository, where it
/// stands, its tests, and Allow for a version waiting for the person.
fn page(view: &mut ViewCx<'_, LuauPluginsUi>) -> AnyElement {
    let t = view.theme().clone();
    let overview = view.data.clone();
    let handle = view.handle.clone();
    let mut column = div().flex().flex_col().gap(sp(4.)).p(sp(6.));
    column = column.child(div().typeset(Type::SMALL).child(rich(
        "Type `/plugin` and what it should do in any chat, and tau writes \
         one. A version is active once it lands and its tests pass.",
        t.muted,
        &t,
    )));
    // Where tau writes them: the chats of the plugins repository, which
    // the sidebar does not list.
    let only = view.entry().map(str::to_owned);
    if only.is_none() {
        column = column.child(chats(view, &t));
    }
    if let Some(error) = &overview.error {
        column = column.child(
            div()
                .typeset(Type::SMALL)
                .text_color(t.red)
                .child(error.clone()),
        );
    }
    if overview.plugins.is_empty() {
        return column
            .child(empty("No plugins yet.", &t))
            .into_any_element();
    }
    // Opened from one plugin's row, the page is that plugin's.
    for entry in overview
        .plugins
        .iter()
        .filter(|entry| only.as_ref().is_none_or(|only| &entry.name == only))
    {
        let (label, tone_of) = match &entry.standing {
            Standing::Active => ("active", Tone::Good),
            Standing::Waiting { .. } => ("waiting for you", Tone::Warn),
            Standing::Failing => ("tests failing", Tone::Danger),
            Standing::Broken { .. } => ("does not load", Tone::Danger),
        };
        let color = t.tone(tone_of);
        let passed = entry.tests.iter().filter(|test| test.passed).count();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(mono(entry.name.clone(), Type::SMALL, t.text))
                    .child(badge(label, color, color.opacity(0.4)))
                    .when(!entry.tests.is_empty(), |row| {
                        row.child(mono(
                            format!(
                                "{passed} of {} tests pass",
                                entry.tests.len()
                            ),
                            Type::CAPTION,
                            t.dim,
                        ))
                    }),
            )
            .when(!entry.description.is_empty(), |body| {
                body.child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.text_soft)
                        .child(entry.description.clone()),
                )
            });
        if entry.keeps_earlier {
            body = body.child(
                div().typeset(Type::CAPTION).text_color(t.dim).child(
                    "Runs keep the version before until this one is fixed \
                     or allowed.",
                ),
            );
        }
        match &entry.standing {
            Standing::Waiting { grown } => {
                let plugin = entry.name.clone();
                let handle = handle.clone();
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(3.))
                        .child(div().flex_1().typeset(Type::SMALL).child(rich(
                            &format!("It wants to use {}.", grown.join(", ")),
                            t.text_soft,
                            &t,
                        )))
                        .child(
                            button("Allow", ButtonKind::Primary, &t)
                                .id(gpui::SharedString::from(format!(
                                    "allow-{plugin}"
                                )))
                                .on_click(move |_, _, cx| {
                                    handle.act(
                                        Act::Allow {
                                            plugin: plugin.clone(),
                                        },
                                        cx,
                                    )
                                }),
                        ),
                );
            }
            Standing::Broken { error } => {
                body = body.child(code_block(None, error, &t));
            }
            _ => {}
        }
        let failing: Vec<_> =
            entry.tests.iter().filter(|test| !test.passed).collect();
        if !failing.is_empty() {
            body = body.child(heading("Failing tests", &t));
            for test in failing {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(1.))
                        .child(mono(
                            format!("{} · {}", test.file, test.name),
                            Type::CAPTION,
                            t.text_soft,
                        ))
                        .when_some(test.error.clone(), |column, error| {
                            column.child(mono(error, Type::CAPTION, t.red))
                        }),
                );
            }
        }
        column = column.child(card(&t).p(sp(4.)).child(body));
    }
    column.into_any_element()
}

/// The chats writing plugins, newest first, each opening its run.
fn chats(view: &ViewCx<'_, LuauPluginsUi>, t: &Theme) -> AnyElement {
    let runs: Vec<RunInfo> = view
        .runs()
        .into_iter()
        .map(|(run, _)| run)
        .filter(|run| run.repo == crate::REPO && !run.title.is_empty())
        .collect();
    let mut list = div().flex().flex_col().child(heading("Chats", t));
    if runs.is_empty() {
        return list
            .child(
                div()
                    .typeset(Type::SMALL)
                    .text_color(t.dim)
                    .child("None yet."),
            )
            .into_any_element();
    }
    for run in runs {
        let handle = view.handle.clone();
        let color = if run.live { t.tone(Tone::Good) } else { t.dim };
        list = list.child(
            div()
                .id(gpui::SharedString::from(format!("chat-{}", run.id.0)))
                .flex()
                .items_center()
                .gap(sp(2.))
                .px(sp(2.))
                .py(sp(1.5))
                .rounded(radius::SMALL)
                .cursor_pointer()
                .hover(|row| row.bg(t.selected))
                .child(dot(color, 7.))
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .typeset(Type::SMALL)
                        .text_color(t.text)
                        .child(run.title.clone()),
                )
                .on_click(move |_, _, cx| handle.open_run(&run.id, cx)),
        );
    }
    list.into_any_element()
}

/// `/plugin <what>`: a chat in the plugins repository, with the skill
/// loaded and the person's words as its task.
fn start_plugin_chat(args: &str, view: &mut ViewCx<'_, LuauPluginsUi>) {
    if args.is_empty() {
        view.handle.composer("/plugin ", view.cx);
        return;
    }
    view.handle.start(
        crate::REPO,
        format!("/{} {args}", crate::SKILL),
        view.cx,
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Records fold into each plugin's part: its state and view, its log
    /// (the newest lines), its errors, its flags, and that it is off.
    #[test]
    fn records_fold_by_plugin() {
        let mut state = State::default();
        let p = || "p".to_owned();
        for record in [
            Record::State {
                plugin: p(),
                state: json!({ "turns": 2 }),
            },
            Record::View {
                plugin: p(),
                view: json!({ "status": { "text": "on", "detail": "2 turns" } }),
            },
            Record::Log {
                plugin: p(),
                lines: (0..250).map(|n| n.to_string()).collect(),
            },
            Record::Error {
                plugin: p(),
                hook: "turn_end".into(),
                error: "no luck".into(),
            },
            Record::Flag {
                plugin: p(),
                call_id: "c1".into(),
                reason: "big".into(),
            },
        ] {
            state
                .apply(record, &mut tau_ui_plugin::testing::FakeRun::default());
        }
        let part = &state.plugins["p"];
        assert_eq!(part.state, json!({ "turns": 2 }));
        assert_eq!(part.logs.len(), MAX_LOGS);
        assert_eq!(part.logs.first().map(String::as_str), Some("50"));
        assert_eq!(part.flags["c1"], "big");
        assert_eq!(
            state.status().as_deref(),
            Some("p: on · 2 turns · 1 failed")
        );
        state.apply(
            Record::Off {
                plugin: p(),
                why: "3 hooks failed".into(),
            },
            &mut tau_ui_plugin::testing::FakeRun::default(),
        );
        assert_eq!(state.status().as_deref(), Some("p: off · 3 hooks failed"));
    }
}
