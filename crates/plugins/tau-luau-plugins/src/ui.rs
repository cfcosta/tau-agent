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
        icon,
        mono,
        pill,
        tag,
        text_link,
        tile,
    },
    prose::rich,
    theme::{Design as _, IconSize, Theme, Tone, Type, radius, sp},
};
use tau_ui_plugin::{
    ENTRY,
    Fold,
    Handle,
    Link,
    Manifest,
    Page,
    RunCx,
    RunInfo,
    SCOPE,
    SlashCommand,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, CardView},
};

use crate::{
    Act,
    Declaration,
    Entry,
    LuauSettings,
    NAME,
    Overview,
    Record,
    SettingsPage,
    Standing,
    TestResult,
    Uses,
    pane::{Bind, SettingsUi},
};

/// The page of the person's own plugins, and, with an entry, of one.
pub const PAGE: &str = "luau-plugins";

/// The command that writes or changes a plugin.
const COMMAND: &str = "plugin";

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
            .page(Page::new(PAGE, page).title(|view| {
                view.entry()
                    .map_or_else(|| "Your plugins".to_owned(), str::to_owned)
            }))
            .settings(crate::pane::render)
            .command(
                SlashCommand::new(
                    COMMAND,
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

/// The page of the person's own plugins (ADR 0036): each plugin of the
/// plugins repository as a card, with where it stands, and the chats
/// writing them. Opened for one plugin, the page is that plugin's.
fn page(view: &mut ViewCx<'_, LuauPluginsUi>) -> AnyElement {
    match view.entry().map(str::to_owned) {
        Some(entry) => plugin_page(&entry, view),
        None => overview(view),
    }
}

/// Every plugin of the plugins repository, the way to write one, and the
/// chats there.
fn overview(view: &mut ViewCx<'_, LuauPluginsUi>) -> AnyElement {
    let t = view.theme().clone();
    let overview = view.data.clone();
    let handle = view.handle.clone();
    let write = {
        let handle = handle.clone();
        button("Write a plugin", ButtonKind::Primary, &t)
            .id("write-plugin")
            .on_click(move |_, _, cx| {
                handle.composer(format!("/{COMMAND} "), cx)
            })
    };
    let head = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(3.5))
        .child(tile(
            Icon::Plug,
            t.change,
            t.change.opacity(0.12),
            t.change.opacity(0.3),
        ))
        .child(
            div()
                .flex_1()
                .min_w(rems(12.))
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child(
                    div()
                        .typeset(Type::TITLE)
                        .text_color(t.text)
                        .child("Your plugins"),
                )
                .child(mono(crate::REPO, Type::CAPTION, t.dim)),
        )
        .child(write);
    let mut column = div()
        .flex()
        .flex_col()
        .gap(sp(5.))
        .p(sp(6.))
        .max_w(rems(66.))
        .child(head)
        .child(div().typeset(Type::SMALL).child(rich(
            &format!(
                "Type `/{COMMAND}` and what it should do in any chat, and \
                 tau writes one here. A version is active once it lands \
                 and its tests pass."
            ),
            t.muted,
            &t,
        )));
    if let Some(error) = &overview.error {
        column = column.child(
            div()
                .typeset(Type::SMALL)
                .text_color(t.red)
                .child(error.clone()),
        );
    }
    if overview.plugins.is_empty() {
        column = column.child(empty("No plugins yet.", &t));
    } else {
        column = column.child(counts(&overview, &t)).child(
            div().flex().flex_wrap().gap(sp(4.)).children(
                overview
                    .plugins
                    .iter()
                    .map(|entry| plugin_card(entry, &handle, &t)),
            ),
        );
    }
    column.child(chats(view, &t)).into_any_element()
}

/// How many plugins there are, and how many stand each way.
fn counts(overview: &Overview, t: &Theme) -> gpui::Div {
    let count = |of: fn(&Standing) -> bool| {
        overview
            .plugins
            .iter()
            .filter(|entry| of(&entry.standing))
            .count()
    };
    let all = overview.plugins.len();
    let tones = [
        (
            count(|standing| matches!(standing, Standing::Active)),
            "active",
            Tone::Good,
        ),
        (
            count(|standing| matches!(standing, Standing::Waiting { .. })),
            "waiting for you",
            Tone::Warn,
        ),
        (
            count(|standing| {
                matches!(standing, Standing::Failing | Standing::Broken { .. })
            }),
            "failing",
            Tone::Danger,
        ),
    ];
    div()
        .flex()
        .flex_wrap()
        .gap(sp(2.))
        .child(pill(
            if all == 1 {
                "1 plugin".to_owned()
            } else {
                format!("{all} plugins")
            },
            t.text_soft,
            t.raised,
        ))
        .children(tones.into_iter().filter(|(n, ..)| *n > 0).map(
            |(n, label, tone)| {
                let color = t.tone(tone);
                pill(format!("{n} {label}"), color, color.opacity(0.12))
            },
        ))
}

/// Where `standing` leaves a plugin, in words and a tone.
fn standing_label(standing: &Standing) -> (&'static str, Tone) {
    match standing {
        Standing::Active => ("active", Tone::Good),
        Standing::Waiting { .. } => ("waiting for you", Tone::Warn),
        Standing::Failing => ("tests failing", Tone::Danger),
        Standing::Broken { .. } => ("does not load", Tone::Danger),
    }
}

/// How many of `entry`'s tests pass, when it has any.
fn tests_line(entry: &Entry) -> Option<String> {
    (!entry.tests.is_empty()).then(|| {
        let passed = entry.tests.iter().filter(|test| test.passed).count();
        format!("{passed} of {} tests pass", entry.tests.len())
    })
}

/// Where a plugin's declaration says it steps into a run.
fn steps_in(declaration: &Declaration) -> Vec<String> {
    let hooks = &declaration.hooks;
    let mut seams: Vec<String> = declaration
        .tools
        .iter()
        .map(|tool| format!("the {} tool", tool.name))
        .collect();
    for (on, label) in [
        (hooks.before_tool, "before each call"),
        (hooks.before_stop, "before a run stops"),
        (hooks.turn_end, "as a turn ends"),
        (hooks.run_end, "as a run ends"),
        (hooks.view, "in the run's view"),
    ] {
        if on {
            seams.push(label.to_owned());
        }
    }
    seams
}

/// What a plugin may reach beyond its own hooks.
fn reaches(uses: &Uses) -> Vec<String> {
    let mut reach: Vec<String> = uses
        .tools
        .iter()
        .map(|tool| format!("the {tool} tool"))
        .collect();
    if uses.jev {
        reach.push("Jev".to_owned());
    }
    if uses.infer {
        reach.push("infer".to_owned());
    }
    reach
}

/// The link to `entry`'s own page.
fn entry_link(entry: &str) -> Link {
    Link::page(PAGE).param(ENTRY, entry)
}

/// The Allow that lets a waiting version of `plugin` reach what it asks.
fn allow(
    plugin: &str,
    handle: &Handle,
    t: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let (plugin, handle) = (plugin.to_owned(), handle.clone());
    button("Allow", ButtonKind::Primary, t)
        .id(gpui::SharedString::from(format!("allow-{plugin}")))
        .on_click(move |_, _, cx| {
            handle.act(
                Act::Allow {
                    plugin: plugin.clone(),
                },
                cx,
            )
        })
}

/// One plugin on the page of all of them: its name, which opens its own
/// page, where it stands, where it steps in, and what it needs.
fn plugin_card(entry: &Entry, handle: &Handle, t: &Theme) -> AnyElement {
    let (label, tone) = standing_label(&entry.standing);
    let color = t.tone(tone);
    let open = {
        let (link, handle) = (entry_link(&entry.name), handle.clone());
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut gpui::App| {
            handle.navigate(link.clone(), cx)
        }
    };
    let mut body = div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .child(
            div()
                .id(gpui::SharedString::from(format!("open-{}", entry.name)))
                .flex()
                .flex_col()
                .gap(sp(1.))
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .child(
                            mono(entry.name.clone(), Type::SMALL, t.text)
                                .min_w(rems(0.))
                                .truncate(),
                        )
                        .child(badge(label, color, color.opacity(0.4)))
                        .child(div().flex_1())
                        .child(icon(Icon::Chevron, IconSize::TINY, t.dim)),
                )
                .children(
                    tests_line(entry)
                        .map(|line| mono(line, Type::CAPTION, t.dim)),
                )
                .on_click(open),
        )
        .when(!entry.description.is_empty(), |body| {
            body.child(
                div()
                    .typeset(Type::SMALL)
                    .text_color(t.text_soft)
                    .child(entry.description.clone()),
            )
        });
    if let Some(declaration) = &entry.declaration {
        let seams = steps_in(declaration);
        if !seams.is_empty() {
            body = body.child(
                div().flex().flex_wrap().gap(sp(1.5)).children(
                    seams
                        .into_iter()
                        .map(|seam| tag(seam, Type::CAPTION, t.text_soft, t)),
                ),
            );
        }
    }
    match &entry.standing {
        Standing::Waiting { grown } => {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .p(sp(3.))
                    .rounded(radius::BOX)
                    .bg(color.opacity(0.08))
                    .child(div().flex_1().typeset(Type::SMALL).child(rich(
                        &format!("It wants to use {}.", grown.join(", ")),
                        t.text,
                        t,
                    )))
                    .child(allow(&entry.name, handle, t)),
            );
        }
        Standing::Broken { error } => {
            body = body.child(code_block(None, error, t));
        }
        _ => {}
    }
    let failing: Vec<_> =
        entry.tests.iter().filter(|test| !test.passed).collect();
    if !failing.is_empty() {
        body = body.child(failing_tests(&failing, t));
    }
    if entry.keeps_earlier {
        body =
            body.child(div().typeset(Type::CAPTION).text_color(t.dim).child(
                "Runs keep the version before until this one is fixed or \
                 allowed.",
            ));
    }
    card(t)
        .p(sp(4.))
        .flex_1()
        .min_w(rems(20.))
        .when(matches!(entry.standing, Standing::Waiting { .. }), |card| {
            card.border_color(color.opacity(0.4))
        })
        .child(body)
        .into_any_element()
}

/// Tests that fail, each with why.
fn failing_tests(failing: &[&TestResult], t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(heading("Failing tests", t))
        .children(failing.iter().map(|test| {
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
                })
        }))
}

/// One plugin's own page: where it stands, Allow when it waits, where it
/// steps in and what it reaches, its tests, and its settings, everywhere
/// or a repository's own.
fn plugin_page(
    entry: &str,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) -> AnyElement {
    let t = view.theme().clone();
    let handle = view.handle.clone();
    let back = {
        let handle = handle.clone();
        text_link("‹ Your plugins", Type::CAPTION, &t)
            .id("back-to-plugins")
            .on_click(move |_, _, cx| handle.navigate(Link::page(PAGE), cx))
    };
    let Some(found) = view
        .data
        .plugins
        .iter()
        .find(|plugin| plugin.name == entry)
        .cloned()
    else {
        return div()
            .flex()
            .flex_col()
            .gap(sp(4.))
            .p(sp(6.))
            .child(back)
            .child(empty(format!("No plugin {entry} here."), &t))
            .into_any_element();
    };
    let (label, tone) = standing_label(&found.standing);
    let color = t.tone(tone);
    let head = div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.5))
                .child(mono(found.name.clone(), Type::TITLE, t.text))
                .child(badge(label, color, color.opacity(0.4)))
                .children(
                    tests_line(&found)
                        .map(|line| mono(line, Type::CAPTION, t.dim)),
                ),
        )
        .when(!found.description.is_empty(), |head| {
            head.child(
                div()
                    .text_color(t.muted)
                    .leading(1.5)
                    .child(found.description.clone()),
            )
        });
    let mut column = div()
        .flex()
        .flex_col()
        .gap(sp(5.))
        .p(sp(6.))
        .max_w(rems(66.))
        .child(back)
        .child(head);
    match &found.standing {
        Standing::Waiting { grown } => {
            column = column.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(3.5))
                    .p(sp(3.5))
                    .rounded(radius::BOX)
                    .bg(color.opacity(0.08))
                    .border_1()
                    .border_color(color.opacity(0.35))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(14.))
                            .flex()
                            .flex_col()
                            .gap(sp(1.))
                            .child(div().typeset(Type::SMALL).child(rich(
                                &format!(
                                    "Its new version wants to use {}.",
                                    grown.join(", ")
                                ),
                                t.text,
                                &t,
                            )))
                            .when(found.keeps_earlier, |text| {
                                text.child(
                                    div()
                                        .typeset(Type::CAPTION)
                                        .text_color(t.muted)
                                        .child(
                                            "Runs keep the version before \
                                             until you allow it.",
                                        ),
                                )
                            }),
                    )
                    .child(allow(&found.name, &handle, &t)),
            );
        }
        Standing::Broken { error } => {
            column = column
                .child(heading("It does not load", &t))
                .child(code_block(None, error, &t));
        }
        Standing::Failing if found.keeps_earlier => {
            column = column.child(
                div().typeset(Type::CAPTION).text_color(t.dim).child(
                    "Runs keep the version before until its tests pass.",
                ),
            );
        }
        _ => {}
    }

    // What it does, on the left; its settings beside it.
    let mut about = div()
        .flex_grow(1.)
        .flex_basis(rems(26.))
        .min_w(rems(0.))
        .flex()
        .flex_col()
        .gap(sp(5.));
    if let Some(declaration) = &found.declaration {
        let seams = steps_in(declaration);
        about = about.child(section(
            "Where it steps in",
            if seams.is_empty() {
                div()
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child("Only in the interface.")
            } else {
                div().flex().flex_wrap().gap(sp(1.5)).children(
                    seams
                        .into_iter()
                        .map(|seam| tag(seam, Type::CAPTION, t.text_soft, &t)),
                )
            },
            &t,
        ));
        let reach = reaches(&declaration.uses);
        about = about.child(section(
            "What it may reach",
            if reach.is_empty() {
                div()
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child("Nothing beyond the run it is in.")
            } else {
                div().flex().flex_wrap().gap(sp(1.5)).children(
                    reach.into_iter().map(|reach| {
                        tag(reach, Type::CAPTION, t.text_soft, &t)
                    }),
                )
            },
            &t,
        ));
    }
    about = about.child(section(
        "Tests",
        if found.tests.is_empty() {
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child("It has none yet.")
        } else {
            card(&t).flex().flex_col().children(found.tests.iter().map(
                |test| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(1.))
                        .px(sp(3.5))
                        .py(sp(2.))
                        .border_b_1()
                        .border_color(t.border)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(2.5))
                                .child(if test.passed {
                                    icon(
                                        Icon::Check,
                                        IconSize::COMPACT,
                                        t.green,
                                    )
                                } else {
                                    icon(Icon::Close, IconSize::COMPACT, t.red)
                                })
                                .child(
                                    mono(
                                        format!(
                                            "{} · {}",
                                            test.file, test.name
                                        ),
                                        Type::CAPTION,
                                        t.text_soft,
                                    )
                                    .flex_1()
                                    .min_w(rems(0.)),
                                ),
                        )
                        .when_some(test.error.clone(), |row, error| {
                            row.child(
                                mono(error, Type::CAPTION, t.red).pl(sp(6.5)),
                            )
                        })
                },
            ))
        },
        &t,
    ));
    let has_settings = found
        .declaration
        .as_ref()
        .is_some_and(|declaration| declaration.settings.is_some());
    let settings = has_settings.then(|| {
        div()
            .flex_grow(1.)
            .flex_basis(rems(20.))
            .max_w(rems(28.))
            .min_w(rems(0.))
            .flex()
            .flex_col()
            .gap(sp(3.))
            .child(heading("Settings", &t))
            .child(scopes(entry, view, &t))
            .child(crate::pane::render(view))
    });
    column
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_start()
                .gap(sp(7.))
                .child(about)
                .children(settings),
        )
        .into_any_element()
}

/// A section of a plugin's page: its heading over `body`.
fn section(title: &str, body: gpui::Div, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(heading(title, t))
        .child(body)
}

/// Everywhere and each of the person's repositories, as a switch: the
/// scope `entry`'s settings show (ADR 0029), and what it means.
fn scopes(
    entry: &str,
    view: &ViewCx<'_, LuauPluginsUi>,
    t: &Theme,
) -> gpui::Div {
    let scope = view.scope().map(str::to_owned);
    let options = std::iter::once(None).chain(
        view.repos()
            .map(|(name, _)| name)
            .filter(|name| *name != crate::REPO)
            .map(|name| Some(name.to_owned())),
    );
    let switch = div()
        .flex()
        .flex_wrap()
        .gap(sp(0.5))
        .p(sp(0.75))
        .rounded(radius::CONTROL)
        .bg(t.panel)
        .border_1()
        .border_color(t.border)
        .children(options.map(|option| {
            let on = option == scope;
            let label = option.clone().unwrap_or_else(|| "Everywhere".into());
            let link = match &option {
                Some(repo) => entry_link(entry).param(SCOPE, repo.clone()),
                None => entry_link(entry),
            };
            let handle = view.handle.clone();
            div()
                .id(gpui::SharedString::from(format!("scope-{label}")))
                .px(sp(3.))
                .py(sp(1.5))
                .rounded(radius::SMALL)
                .cursor_pointer()
                .typeset(Type::CAPTION)
                .text_color(if on { t.text } else { t.dim })
                .when(on, |option| option.bg(t.raised))
                .child(label)
                .on_click(move |_, _, cx| handle.navigate(link.clone(), cx))
        }));
    let means = match &scope {
        Some(repo) => format!("Runs in {repo} use these."),
        None => "Every repository without its own copy uses these.".to_owned(),
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(switch)
        .child(div().typeset(Type::CAPTION).text_color(t.dim).child(means))
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
