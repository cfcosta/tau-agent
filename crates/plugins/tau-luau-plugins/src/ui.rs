//! tau-luau-plugins' UI (ADR 0017, 0027): the records folded into each
//! run's state, the view trees Luau plugins return drawn with the shared
//! tree (ADR 0045), the run's status line, the cards of the plugins' own
//! tools, and a badge on calls a plugin flagged.
//!
//! The drawing reads JSON only, so the phone, which runs no Luau, draws
//! what the computer does.

use std::{collections::BTreeMap, rc::Rc};

use gpui::{AnyElement, App};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_ui_kit::assets::Icon;
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
    points::{self, AtAnchor, AtCard, CardView},
    tree::{Kind, Node, Tab, Tone},
};

use crate::{
    Act,
    ActionDone,
    Declaration,
    Entry,
    LuauSettings,
    NAME,
    Overview,
    Record,
    Standing,
    TestResult,
    Uses,
};

/// The page of the person's own plugins, and, with an entry, of one.
pub const PAGE: &str = "luau-plugins";

/// The command that writes or changes a plugin.
const COMMAND: &str = "plugin";

/// Log lines a run keeps per plugin, the newest.
const MAX_LOGS: usize = 200;

/// tau-luau-plugins with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct LuauPluginsUi;

/// What a run's Luau plugins left, by plugin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub plugins: BTreeMap<String, PluginState>,
    /// The notes plugins' views drew, by the anchor each was placed at
    /// in the transcript: the plugin, and the note's tree.
    #[serde(default)]
    pub notes: BTreeMap<String, (String, Value)>,
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

    fn apply(&mut self, record: Record, run: &mut dyn RunCx) {
        match record {
            Record::State { plugin, state } => self.of(plugin).state = state,
            Record::View { plugin, view } => {
                // A note goes where the run is when it first draws, and
                // stays there as later ones come.
                let note = view.get("note").filter(|note| !note.is_null());
                let last = self
                    .plugins
                    .get(&plugin)
                    .and_then(|part| part.view.get("note"));
                if let Some(note) = note.filter(|note| Some(*note) != last) {
                    let key = format!("note-{}", self.notes.len());
                    self.notes
                        .insert(key.clone(), (plugin.clone(), note.clone()));
                    run.transcript(&key);
                }
                self.of(plugin).view = view;
            }
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

/// What the actions of a Luau plugin's tree start with, once it sits in
/// one of the page's own: they are the plugin's, not the page's.
const LUAU: &str = "luau/";

/// The page's own actions: write a plugin, allow a waiting version
/// (`{ plugin }`), open a chat (`{ run }`).
const WRITE: &str = "write";
const ALLOW: &str = "allow";
const OPEN_RUN: &str = "open-run";

/// `node`, with each child rewritten first, then itself by `f`.
pub(crate) fn rewrite(node: Node, f: &mut dyn FnMut(Node) -> Node) -> Node {
    fn each(children: Vec<Node>, f: &mut dyn FnMut(Node) -> Node) -> Vec<Node> {
        children
            .into_iter()
            .map(|child| rewrite(child, &mut *f))
            .collect()
    }
    let node = match node {
        Node::Stack { children } => Node::Stack {
            children: each(children, f),
        },
        Node::Row { children } => Node::Row {
            children: each(children, f),
        },
        Node::Scroll { height, children } => Node::Scroll {
            height,
            children: each(children, f),
        },
        Node::Card { children } => Node::Card {
            children: each(children, f),
        },
        Node::Keys {
            id,
            keys,
            through,
            action,
            focus,
            children,
        } => Node::Keys {
            id,
            keys,
            through,
            action,
            focus,
            children: each(children, f),
        },
        Node::List { items } => Node::List {
            items: each(items, f),
        },
        Node::Section {
            id,
            title,
            open,
            children,
        } => Node::Section {
            id,
            title,
            open,
            children: each(children, f),
        },
        Node::Modal {
            title,
            children,
            dismiss,
        } => Node::Modal {
            title,
            children: each(children, f),
            dismiss,
        },
        Node::Table { head, rows, widths } => Node::Table {
            head,
            rows: rows.into_iter().map(|row| each(row, &mut *f)).collect(),
            widths,
        },
        Node::Tabs { id, tabs } => Node::Tabs {
            id,
            tabs: tabs
                .into_iter()
                .map(|tab| Tab {
                    label: tab.label,
                    children: each(tab.children, &mut *f),
                })
                .collect(),
        },
        other => other,
    };
    f(node)
}

/// A view tree a Luau plugin returned, read to sit inside one of
/// tau-luau-plugins' own (ADR 0045): its sections, tabs, inputs and
/// widgets keep their state under `plugin`'s name, and its actions are
/// marked as the plugin's. A piece it does not know draws as a red box
/// that says so.
pub fn embed(piece: &Value, plugin: &str) -> Node {
    let own = |id: String| format!("{plugin}/{id}");
    let act = |action: String| format!("{LUAU}{action}");
    rewrite(Node::read(piece), &mut |node| match node {
        Node::Section {
            id,
            title,
            open,
            children,
        } => Node::Section {
            id: own(id),
            title,
            open,
            children,
        },
        Node::Tabs { id, tabs } => Node::Tabs { id: own(id), tabs },
        Node::Input {
            id,
            placeholder,
            multiline,
            submit,
            keep,
            secret,
            focus,
        } => Node::Input {
            id: own(id),
            placeholder,
            multiline,
            submit: submit.map(act),
            keep,
            secret,
            focus,
        },
        Node::Keys {
            id,
            keys,
            through,
            action,
            focus,
            children,
        } => Node::Keys {
            id: own(id),
            keys,
            through,
            action: act(action),
            focus,
            children,
        },
        Node::Terminal {
            id,
            text,
            cols,
            rows,
            live,
            fold,
        } => Node::Terminal {
            id: own(id),
            text,
            cols,
            rows,
            live,
            fold,
        },
        Node::Ticker { id, since, until } => Node::Ticker {
            id: own(id),
            since,
            until,
        },
        Node::Due { id, at } => Node::Due { id: own(id), at },
        Node::Button {
            label,
            action,
            args,
            kind,
            icon,
        } => Node::Button {
            label,
            action: act(action),
            args,
            kind,
            icon,
        },
        Node::Switch {
            label,
            detail,
            on,
            action,
            args,
        } => Node::Switch {
            label,
            detail,
            on,
            action: act(action),
            args,
        },
        Node::Modal {
            title,
            children,
            dismiss,
        } => Node::Modal {
            title,
            children,
            dismiss: dismiss.map(act),
        },
        other => other,
    })
}

/// Carries out the actions of `plugin`'s embedded tree, on its state in
/// `run`; outside a run there is no state to act on. Says whether the
/// action was one of the plugin's.
pub type Acting = Rc<dyn Fn(&str, &Value, &mut App) -> bool>;

/// [`Acting`] for `plugin` in `run`, with its state there.
fn acting(
    plugin: &str,
    run: Option<(String, State)>,
    view: &ViewCx<'_, LuauPluginsUi>,
) -> Acting {
    let handle = view.handle.clone();
    let plugin = plugin.to_owned();
    let on = run.map(|(run, state)| {
        let state = state
            .plugins
            .get(&plugin)
            .map_or_else(|| json!({}), |part| part.state.clone());
        (run, state, view.settings.plugins.get(&plugin).cloned())
    });
    Rc::new(move |action: &str, args: &Value, cx: &mut App| {
        let Some(action) = action.strip_prefix(LUAU) else {
            return false;
        };
        if let Some((run, state, settings)) = &on {
            handle.act(
                Act::Action {
                    plugin: plugin.clone(),
                    action: action.to_owned(),
                    args: args.clone(),
                    run: run.clone(),
                    state: state.clone(),
                    settings: settings.clone(),
                },
                cx,
            );
        }
        true
    })
}

/// [`acting`] in the run the view is of, if any.
fn acting_here(plugin: &str, view: &ViewCx<'_, LuauPluginsUi>) -> Acting {
    let run = view.run.map(|run| run.id.0.to_string());
    let state = view.state.cloned();
    acting(plugin, run.zip(state), view)
}

/// Draws a view tree a Luau plugin returned, in the run the view is of:
/// where its buttons act.
pub fn draw(
    piece: &Value,
    id: &str,
    plugin: &str,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) -> AnyElement {
    let tree = embed(piece, plugin);
    let on = acting_here(plugin, view);
    view.tree(id, &tree, move |action, args, cx| {
        on(action, args, cx);
    })
}

impl UiPlugin for LuauPluginsUi {
    type State = State;
    type Data = Overview;
    type RepoData = ();
    type Settings = LuauSettings;

    fn name(&self) -> &'static str {
        NAME
    }

    /// What an action left.
    fn reply(&self, reply: Value, handle: &Handle, cx: &mut App) {
        if let Ok(done) = serde_json::from_value::<ActionDone>(reply.clone()) {
            // What an action left, with the run it was carried out on.
            let run = RunId(done.run.as_str().into());
            for record in done.records() {
                handle.record(&run, record, cx);
            }
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(
                Page::new(PAGE, page)
                    .title(|view| {
                        view.entry().map_or_else(
                            || "Your plugins".to_owned(),
                            str::to_owned,
                        )
                    })
                    .framed(),
            )
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
                let plugin = details.get("plugin")?.as_str()?;
                let card =
                    details.get("card").filter(|card| !card.is_null())?;
                Some(CardView {
                    body: Some(draw(
                        card,
                        &format!("luau-card-{}", at.call_id),
                        plugin,
                        view,
                    )),
                    ..CardView::default()
                })
            })
            .contribute(points::TRANSCRIPT, |at: &AtAnchor, view| {
                let (plugin, note) = view.state?.notes.get(&at.key)?.clone();
                let id = format!("luau-note-{}-{}", at.run.id.0, at.key);
                let tree = Node::Card {
                    children: vec![
                        Node::row([
                            Node::Icon {
                                name: "plug".into(),
                                tone: Tone::Info,
                            },
                            Node::Mono {
                                text: plugin.clone(),
                            },
                        ]),
                        embed(&note, &plugin),
                    ],
                };
                let on = acting_here(&plugin, view);
                Some(view.tree(&id, &tree, move |action, args, cx| {
                    on(action, args, cx);
                }))
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
                let tree = Node::Badge {
                    text: format!("flagged by {plugin}: {reason}"),
                    tone: Tone::Warn,
                };
                Some(view.tree(
                    &format!("luau-flag-{}", at.call_id),
                    &tree,
                    |_, _, _| {},
                ))
            })
    }
}

/// The page of the person's own plugins (ADR 0036): each plugin of the
/// plugins repository as a card, with where it stands, and the chats
/// writing them. Opened for one plugin, the page is that plugin's. A
/// view tree in the frame the interface gives a page (ADR 0045).
fn page(view: &mut ViewCx<'_, LuauPluginsUi>) -> AnyElement {
    let handle = view.handle.clone();
    match view.entry().map(str::to_owned) {
        Some(entry) => {
            let (tree, luau, saver) = plugin_page(&entry, view);
            view.tree(
                &format!("luau-plugin-{entry}"),
                &tree,
                move |action, args, cx| {
                    if luau.as_ref().is_some_and(|on| on(action, args, cx)) {
                        return;
                    }
                    if saver
                        .as_ref()
                        .is_some_and(|saver| saver.act(action, args, cx))
                    {
                        return;
                    }
                    page_action(&handle, action, args, cx);
                },
            )
        }
        None => {
            let tree = overview(view);
            view.tree("luau-plugins", &tree, move |action, args, cx| {
                page_action(&handle, action, args, cx)
            })
        }
    }
}

/// Carries out one of the page's own actions.
fn page_action(handle: &Handle, action: &str, args: &Value, cx: &mut App) {
    match action {
        WRITE => handle.composer(format!("/{COMMAND} "), cx),
        ALLOW => {
            if let Some(plugin) = args["plugin"].as_str() {
                handle.act(
                    Act::Allow {
                        plugin: plugin.to_owned(),
                    },
                    cx,
                );
            }
        }
        OPEN_RUN => {
            if let Some(run) = args["run"].as_str() {
                handle.open_run(&RunId(run.into()), cx);
            }
        }
        _ => {}
    }
}

/// Every plugin of the plugins repository, the way to write one, and the
/// chats there.
fn overview(view: &ViewCx<'_, LuauPluginsUi>) -> Node {
    let overview = view.data;
    let mut page = vec![
        Node::row([
            Node::Icon {
                name: "plug".into(),
                tone: Tone::Info,
            },
            Node::Title {
                text: "Your plugins".into(),
                subtitle: Some(crate::REPO.into()),
            },
            Node::Spacer,
            Node::Button {
                label: "Write a plugin".into(),
                action: WRITE.into(),
                args: Value::Null,
                kind: Kind::Primary,
                icon: None,
            },
        ]),
        Node::Rich {
            text: format!(
                "Type `/{COMMAND}` and what it should do in any chat, and \
                 tau writes one here. A version is active once it lands \
                 and its tests pass."
            ),
        },
    ];
    if let Some(error) = &overview.error {
        page.push(Node::Status {
            text: error.clone(),
            detail: None,
            tone: Tone::Bad,
        });
    }
    if overview.plugins.is_empty() {
        page.push(Node::Empty {
            text: "No plugins yet.".into(),
        });
    } else {
        page.push(counts(overview));
        page.extend(overview.plugins.iter().map(plugin_card));
    }
    page.push(chats(view));
    Node::stack(page)
}

/// How many plugins there are, and how many stand each way.
fn counts(overview: &Overview) -> Node {
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
            Tone::Bad,
        ),
    ];
    let mut pills = vec![Node::Pill {
        text: if all == 1 {
            "1 plugin".to_owned()
        } else {
            format!("{all} plugins")
        },
        tone: Tone::Neutral,
    }];
    pills.extend(tones.into_iter().filter(|(n, ..)| *n > 0).map(
        |(n, label, tone)| Node::Pill {
            text: format!("{n} {label}"),
            tone,
        },
    ));
    Node::row(pills)
}

/// Where `standing` leaves a plugin, in words and a tone.
fn standing_label(standing: &Standing) -> (&'static str, Tone) {
    match standing {
        Standing::Active => ("active", Tone::Good),
        Standing::Waiting { .. } => ("waiting for you", Tone::Warn),
        Standing::Failing => ("tests failing", Tone::Bad),
        Standing::Broken { .. } => ("does not load", Tone::Bad),
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
fn allow(plugin: &str) -> Node {
    Node::Button {
        label: "Allow".into(),
        action: ALLOW.into(),
        args: json!({ "plugin": plugin }),
        kind: Kind::Primary,
        icon: None,
    }
}

/// Tags, one per thing said, side by side.
fn tags(words: Vec<String>) -> Node {
    Node::row(words.into_iter().map(|text| Node::Pill {
        text,
        tone: Tone::Neutral,
    }))
}

/// One plugin on the page of all of them: its name, which opens its own
/// page, where it stands, where it steps in, and what it needs.
fn plugin_card(entry: &Entry) -> Node {
    let (label, tone) = standing_label(&entry.standing);
    let mut head = vec![
        Node::Link {
            label: entry.name.clone(),
            to: entry_link(&entry.name),
        },
        Node::Badge {
            text: label.into(),
            tone,
        },
    ];
    if let Some(line) = tests_line(entry) {
        head.push(Node::Spacer);
        head.push(Node::Mono { text: line });
    }
    let mut body = vec![Node::row(head)];
    if !entry.description.is_empty() {
        body.push(Node::text(entry.description.clone()));
    }
    if let Some(declaration) = &entry.declaration {
        let seams = steps_in(declaration);
        if !seams.is_empty() {
            body.push(tags(seams));
        }
    }
    match &entry.standing {
        Standing::Waiting { grown } => body.push(Node::row([
            Node::Rich {
                text: format!("It wants to use {}.", grown.join(", ")),
            },
            Node::Spacer,
            allow(&entry.name),
        ])),
        Standing::Broken { error } => body.push(Node::Code {
            lang: None,
            text: error.clone(),
            first_line: None,
        }),
        _ => {}
    }
    let failing: Vec<_> =
        entry.tests.iter().filter(|test| !test.passed).collect();
    if !failing.is_empty() {
        body.push(failing_tests(&failing));
    }
    if entry.keeps_earlier {
        body.push(Node::text(
            "Runs keep the version before until this one is fixed or \
             allowed.",
        ));
    }
    Node::Card { children: body }
}

/// A test's line: its file and name, and why it failed, if it did.
fn test_lines(test: &TestResult) -> Vec<Node> {
    let mut lines = vec![Node::Mono {
        text: format!("{} · {}", test.file, test.name),
    }];
    if let Some(error) = &test.error {
        lines.push(Node::Status {
            text: error.clone(),
            detail: None,
            tone: Tone::Bad,
        });
    }
    lines
}

/// Tests that fail, each with why.
fn failing_tests(failing: &[&TestResult]) -> Node {
    let mut lines = vec![Node::Heading {
        text: "Failing tests".into(),
    }];
    lines.extend(failing.iter().flat_map(|test| test_lines(test)));
    Node::stack(lines)
}

/// One plugin's own page: where it stands, Allow when it waits, where it
/// steps in and what it reaches, its tests, and its settings, everywhere
/// or a repository's own. With it, what its own page's actions do, and
/// how its settings save.
fn plugin_page(
    entry: &str,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) -> (Node, Option<Acting>, Option<crate::pane::Saver>) {
    let back = Node::Link {
        label: "‹ Your plugins".into(),
        to: Link::page(PAGE),
    };
    let Some(found) = view
        .data
        .plugins
        .iter()
        .find(|plugin| plugin.name == entry)
        .cloned()
    else {
        let empty = Node::Empty {
            text: format!("No plugin {entry} here."),
        };
        return (Node::stack([back, empty]), None, None);
    };
    let (label, tone) = standing_label(&found.standing);
    let mut standing = vec![Node::Badge {
        text: label.into(),
        tone,
    }];
    if let Some(line) = tests_line(&found) {
        standing.push(Node::Mono { text: line });
    }
    let mut page = vec![
        back,
        Node::Title {
            text: found.name.clone(),
            subtitle: (!found.description.is_empty())
                .then(|| found.description.clone()),
        },
        Node::row(standing),
    ];
    match &found.standing {
        Standing::Waiting { grown } => {
            let mut words = vec![Node::Rich {
                text: format!(
                    "Its new version wants to use {}.",
                    grown.join(", ")
                ),
            }];
            if found.keeps_earlier {
                words.push(Node::text(
                    "Runs keep the version before until you allow it.",
                ));
            }
            page.push(Node::Card {
                children: vec![Node::row([
                    Node::stack(words),
                    Node::Spacer,
                    allow(&found.name),
                ])],
            });
        }
        Standing::Broken { error } => {
            page.push(Node::Heading {
                text: "It does not load".into(),
            });
            page.push(Node::Code {
                lang: None,
                text: error.clone(),
                first_line: None,
            });
        }
        Standing::Failing if found.keeps_earlier => {
            page.push(Node::text(
                "Runs keep the version before until its tests pass.",
            ));
        }
        _ => {}
    }

    if let Some(declaration) = &found.declaration {
        let seams = steps_in(declaration);
        page.extend(section(
            "Where it steps in",
            if seams.is_empty() {
                Node::text("Only in the interface.")
            } else {
                tags(seams)
            },
        ));
        let reach = reaches(&declaration.uses);
        page.extend(section(
            "What it may reach",
            if reach.is_empty() {
                Node::text("Nothing beyond the run it is in.")
            } else {
                tags(reach)
            },
        ));
    }
    // What its view puts on its page, from the newest run that drew one;
    // its buttons act on that run.
    let drawn = view.runs().into_iter().find_map(|(run, state)| {
        let page = state.plugins.get(entry)?.view.get("page")?.clone();
        (!page.is_null()).then_some((run, state, page))
    });
    let mut luau = None;
    if let Some((run, state, drawn)) = drawn {
        let title = if run.title.is_empty() {
            "In its last run".to_owned()
        } else {
            format!("In {}", run.title)
        };
        page.extend(section(&title, embed(&drawn, entry)));
        luau = Some(acting(entry, Some((run.id.0.to_string(), state)), view));
    }
    page.extend(section(
        "Tests",
        if found.tests.is_empty() {
            Node::text("It has none yet.")
        } else {
            Node::Card {
                children: found
                    .tests
                    .iter()
                    .map(|test| {
                        let mut line = vec![Node::Icon {
                            name: if test.passed { "check" } else { "close" }
                                .into(),
                            tone: if test.passed {
                                Tone::Good
                            } else {
                                Tone::Bad
                            },
                        }];
                        line.push(Node::stack(test_lines(test)));
                        Node::row(line)
                    })
                    .collect(),
            }
        },
    ));
    let has_settings = found
        .declaration
        .as_ref()
        .is_some_and(|declaration| declaration.settings.is_some());
    let mut saver = None;
    if has_settings {
        page.push(Node::Heading {
            text: "Settings".into(),
        });
        page.extend(scopes(entry, view));
        let (pane, save) = crate::pane::pane(view);
        page.push(pane);
        saver = save;
    }
    (Node::stack(page), luau, saver)
}

/// A section of a plugin's page: its heading over `body`.
fn section(title: &str, body: Node) -> [Node; 2] {
    [
        Node::Heading {
            text: title.to_owned(),
        },
        body,
    ]
}

/// Everywhere and each of the person's repositories, the one shown
/// marked: the scope `entry`'s settings show (ADR 0029), and what it
/// means.
fn scopes(entry: &str, view: &ViewCx<'_, LuauPluginsUi>) -> [Node; 2] {
    let scope = view.scope().map(str::to_owned);
    let options = std::iter::once(None).chain(
        view.repos()
            .map(|(name, _)| name)
            .filter(|name| *name != crate::REPO)
            .map(|name| Some(name.to_owned())),
    );
    let switch = Node::row(options.map(|option| {
        let label = option.clone().unwrap_or_else(|| "Everywhere".into());
        if option == scope {
            return Node::Pill {
                text: label,
                tone: Tone::Info,
            };
        }
        let to = match &option {
            Some(repo) => entry_link(entry).param(SCOPE, repo.clone()),
            None => entry_link(entry),
        };
        Node::Link { label, to }
    }));
    let means = match &scope {
        Some(repo) => format!("Runs in {repo} use these."),
        None => "Every repository without its own copy uses these.".to_owned(),
    };
    [switch, Node::text(means)]
}

/// The chats writing plugins, newest first, each opening its run.
fn chats(view: &ViewCx<'_, LuauPluginsUi>) -> Node {
    let runs: Vec<RunInfo> = view
        .runs()
        .into_iter()
        .map(|(run, _)| run)
        .filter(|run| run.repo == crate::REPO && !run.title.is_empty())
        .collect();
    let mut list = vec![Node::Heading {
        text: "Chats".into(),
    }];
    if runs.is_empty() {
        list.push(Node::text("None yet."));
    }
    list.extend(runs.into_iter().map(|run| {
        Node::row([
            Node::Status {
                text: run.title.clone(),
                detail: None,
                tone: if run.live { Tone::Good } else { Tone::Neutral },
            },
            Node::Spacer,
            Node::Button {
                label: "Open".into(),
                action: OPEN_RUN.into(),
                args: json!({ "run": run.id.0.to_string() }),
                kind: Kind::Secondary,
                icon: None,
            },
        ])
    }));
    Node::stack(list)
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

    /// A view's note is placed in the transcript where the run is when
    /// it first draws, and again only when it changes; each keeps the
    /// note it was placed with.
    #[test]
    fn notes_are_placed_where_they_change() {
        let mut state = State::default();
        let mut run = tau_ui_plugin::testing::FakeRun::default();
        let view = |note: Value| Record::View {
            plugin: "p".into(),
            view: json!({ "status": { "text": "on" }, "note": note }),
        };
        for record in [
            view(Value::Null),
            view(json!("first")),
            view(json!("first")),
            view(json!({ "piece": "text", "text": "second" })),
        ] {
            state.apply(record, &mut run);
        }
        assert_eq!(run.anchors, ["note-0", "note-1"]);
        assert_eq!(state.notes["note-0"], ("p".to_owned(), json!("first")));
        assert_eq!(
            state.notes["note-1"].1,
            json!({ "piece": "text", "text": "second" })
        );
    }

    /// What an action left folds into the run as its records do: the
    /// state, the log, the error, and the view drawn from the state.
    #[test]
    fn what_an_action_left_folds_into_the_run() {
        let done = ActionDone {
            plugin: "p".into(),
            run: "r1".into(),
            state: json!({ "n": 2 }),
            view: Some(json!({ "status": { "text": "2" } })),
            logs: vec!["added".into()],
            error: Some(("view".into(), "late".into())),
        };
        let mut state = State::default();
        for record in done.records() {
            state
                .apply(record, &mut tau_ui_plugin::testing::FakeRun::default());
        }
        let part = &state.plugins["p"];
        assert_eq!(part.state, json!({ "n": 2 }));
        assert_eq!(part.view, json!({ "status": { "text": "2" } }));
        assert_eq!(part.logs, ["added"]);
        assert_eq!(part.errors, [("view".to_owned(), "late".to_owned())]);
    }

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
