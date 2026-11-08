//! tau-luau-plugins' UI (ADR 0017, 0027): the records folded into each
//! run's state, the view trees Luau plugins return drawn with the shared
//! tree (ADR 0045), the run's status line, the cards of the plugins' own
//! tools, and a badge on calls a plugin flagged.
//!
//! The drawing reads JSON only, so the phone, which runs no Luau, draws
//! what the computer does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_ui_kit::assets::Icon;
use tau_ui_plugin::{
    ENTRY,
    Fold,
    Handle,
    KindPlace,
    Link,
    ListedCommand,
    Manifest,
    NavEntry,
    Page,
    RepoKind,
    RowNote,
    RunCx,
    RunInfo,
    SCOPE,
    SlashCommand,
    UiPlugin,
    ViewCx,
    points::{self, AtAnchor, AtApp, AtCard, AtRepo, AtRun, CardView},
    tree::{Kind, Node, Tab, Tone},
};

use crate::{
    Act,
    ActionDone,
    Declaration,
    Entry,
    KIND,
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
    /// What calls asked the person to allow, by call id.
    #[serde(default)]
    pub asks: BTreeMap<String, crate::Ask>,
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
                // Keyed notes: each goes where the run is when its key
                // first comes, and is drawn again there as it changes.
                for (key, note) in view
                    .get("notes")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    let anchor = format!("note-{plugin}-{key}");
                    let placed = self.notes.contains_key(&anchor);
                    if note.is_null() && !placed {
                        continue;
                    }
                    self.notes
                        .insert(anchor.clone(), (plugin.clone(), note.clone()));
                    if !placed {
                        run.transcript(&anchor);
                    }
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
            Record::Asked {
                plugin,
                call_id,
                title,
                text,
            } => {
                self.asks.insert(
                    call_id,
                    crate::Ask {
                        plugin,
                        title,
                        text,
                        outcome: crate::Outcome::Waiting,
                    },
                );
            }
            Record::Answered {
                call_id, outcome, ..
            } => {
                if let Some(ask) = self.asks.get_mut(&call_id) {
                    ask.outcome = outcome;
                }
            }
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
/// one of tau-luau-plugins' own: they are the plugin's, not the page's.
/// After it come the plugin's name, `/`, and its own action. A tree on a
/// card or in the transcript acts on the run it is drawn in.
const LUAU: &str = "luau/";

/// [`LUAU`] for the tree on a plugin's own page, which acts on the newest
/// run that drew one ([`page_run`]).
const LUAU_PAGE: &str = "luau-page/";

/// What the actions of a Luau plugin's page of a repository start with
/// ([`embed_repo`]): carried out in the repository the page shows.
const LUAU_REPO: &str = "luau-repo/";

/// The action of an Allow card's buttons: the person's answer to what a
/// call asked them to allow, with `{ run, call, allowed }`.
const ANSWER: &str = "luau-answer";

/// The page parameter naming which of a plugin's pages it shows.
pub const PAGE_PARAM: &str = "page";

/// The page parameter naming the repository a plugin's page shows.
pub const REPO_PARAM: &str = "repo";

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
        Node::Board { action, columns } => Node::Board {
            action,
            columns: columns
                .into_iter()
                .map(|column| tau_ui_plugin::tree::BoardColumn {
                    cards: column
                        .cards
                        .into_iter()
                        .map(|card| tau_ui_plugin::tree::BoardCard {
                            id: card.id,
                            children: each(card.children, &mut *f),
                        })
                        .collect(),
                    ..column
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
    embed_as(piece, plugin, LUAU)
}

/// `piece`, a Luau plugin's page of `repo`, as a tree whose actions are
/// carried out in that repository, and whose links to the plugin's own
/// pages open them of that repository.
pub fn embed_repo(piece: &Value, plugin: &str, repo: &str) -> Node {
    rewrite(embed_as(piece, plugin, LUAU_REPO), &mut |node| match node {
        Node::Link { label, to } if to.plugin.is_none() => {
            let mut link = repo_page_link(plugin, &to.page, repo);
            for (name, value) in to.params {
                if name != PAGE_PARAM && name != REPO_PARAM {
                    link = link.param(name, value);
                }
            }
            Node::Link { label, to: link }
        }
        other => other,
    })
}

/// [`embed`], its actions starting with `mark`.
fn embed_as(piece: &Value, plugin: &str, mark: &str) -> Node {
    let own = |id: String| format!("{plugin}/{id}");
    let act = |action: String| format!("{mark}{plugin}/{action}");
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
        Node::Board { action, columns } => Node::Board {
            action: act(action),
            columns,
        },
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
        Node::Ago { id, at } => Node::Ago { id: own(id), at },
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

/// Carries out `plugin`'s own `action` with `args`, on its state in
/// `run`; outside a run there is no state to act on.
/// What a Luau tree's action starting with `tau.` asks of the interface,
/// on `run`, the run the tree is drawn in or its page shows: whether it
/// was one.
fn kernel_action(
    action: &str,
    args: &Value,
    run: Option<&RunId>,
    handle: &Handle,
) -> bool {
    let Some(kernel) = action.strip_prefix("tau.") else {
        return false;
    };
    let text = |key: &str| args[key].as_str().unwrap_or_default().to_owned();
    match kernel {
        // To the run going on, cutting in.
        "steer" => {
            if let Some(run) = run {
                handle.steer(run, text("text"));
            }
        }
        // The next message: to the run, or as a new one.
        "send" => handle.send(run, text("text")),
        "composer" => handle.composer(text("text")),
        "start" => handle.start(text("repo"), text("text")),
        "alert" => handle.alert(text("title"), text("message")),
        "details" => handle.run_details(),
        other => {
            handle.note(format!("`tau.{other}` is not one of tau's actions"))
        }
    }
    true
}

/// What a call asked the person to allow: what it would do, and while
/// it waits and its run goes on, Allow and Deny; then what came of it.
fn allow_card(ask: &crate::Ask, at: &AtCard) -> Node {
    use crate::Outcome;
    let mut children = vec![Node::row([
        Node::Icon {
            name: "shield".into(),
            tone: Tone::Warn,
        },
        Node::Title {
            text: ask.title.clone(),
            subtitle: Some(ask.plugin.clone()),
        },
    ])];
    if !ask.text.is_empty() {
        children.push(Node::Rich {
            text: ask.text.clone(),
        });
    }
    let answer = |allowed: bool| json!({ "run": at.run.id.0.as_ref(), "call": at.call_id, "allowed": allowed });
    let outcome = match ask.outcome {
        Outcome::Waiting if at.run.live => {
            children.push(Node::row([
                Node::Button {
                    label: "Allow".into(),
                    action: ANSWER.into(),
                    args: answer(true),
                    kind: Kind::Primary,
                    icon: None,
                },
                Node::Button {
                    label: "Deny".into(),
                    action: ANSWER.into(),
                    args: answer(false),
                    kind: Kind::Secondary,
                    icon: None,
                },
            ]));
            None
        }
        Outcome::Waiting | Outcome::Withdrawn => {
            Some(("Withdrawn: the run ended first", Tone::Neutral))
        }
        Outcome::Allowed => Some(("Allowed", Tone::Good)),
        Outcome::Denied => Some(("Denied", Tone::Bad)),
    };
    if let Some((text, tone)) = outcome {
        children.push(Node::Status {
            text: text.into(),
            detail: None,
            tone,
        });
    }
    Node::stack(children)
}

fn luau_action(
    plugin: &str,
    action: &str,
    args: &Value,
    run: Option<(String, Option<String>, State)>,
    view: &ViewCx<'_, LuauPluginsUi>,
) {
    let Some((run, repo, state)) = run else {
        return;
    };
    let state = state
        .plugins
        .get(plugin)
        .map_or_else(|| json!({}), |part| part.state.clone());
    view.handle.act(Act::Action {
        plugin: plugin.to_owned(),
        action: action.to_owned(),
        args: args.clone(),
        run,
        repo,
        state,
        settings: view.settings.plugins.get(plugin).cloned(),
    });
}

/// The newest run that drew a page for `plugin`, its state there, and
/// the page.
fn page_run(
    plugin: &str,
    view: &ViewCx<'_, LuauPluginsUi>,
) -> Option<(RunInfo, State, Value)> {
    view.runs().into_iter().find_map(|(run, state)| {
        let page = state.plugins.get(plugin)?.view.get("page")?.clone();
        (!page.is_null()).then_some((run, state, page))
    })
}

impl UiPlugin for LuauPluginsUi {
    type State = State;
    type Data = Overview;
    type RepoData = crate::RepoValues;
    type Settings = LuauSettings;

    fn name(&self) -> &'static str {
        NAME
    }

    /// What an action left.
    fn reply(&self, reply: Value, handle: &Handle) {
        if let Ok(done) = serde_json::from_value::<ActionDone>(reply.clone()) {
            // What an action left, with the run it was carried out on.
            let run = RunId(done.run.as_str().into());
            for record in done.records() {
                handle.record(&run, record);
            }
            // What it asked the interface to do, on its run.
            if let Some(asked) = &done.interface {
                let action = asked["action"].as_str().unwrap_or_default();
                kernel_action(
                    &format!("tau.{action}"),
                    &asked["args"],
                    Some(&run),
                    handle,
                );
            }
        }
    }

    /// The actions of every tree it draws: a Luau plugin's own, on the run
    /// its tree is drawn in or the one its page shows; a settings pane's,
    /// which save; and the page's.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        if action == ANSWER {
            let text = |key: &str| args[key].as_str().unwrap_or_default();
            view.handle.act(Act::Answer {
                run: text("run").to_owned(),
                call: text("call").to_owned(),
                allowed: args["allowed"].as_bool().unwrap_or(false),
                reason: text("reason").to_owned(),
            });
            return;
        }
        if let Some((plugin, own)) = action
            .strip_prefix(LUAU)
            .and_then(|rest| rest.split_once('/'))
        {
            let run = view.run.map(|run| run.id.clone());
            if kernel_action(own, args, run.as_ref(), &view.handle) {
                return;
            }
            let repo = view.run.map(|run| run.repo.clone());
            let run = run
                .map(|run| run.0.to_string())
                .zip(view.state.cloned())
                .map(|(run, state)| (run, repo, state));
            luau_action(plugin, own, args, run, view);
            return;
        }
        if let Some((plugin, own)) = action
            .strip_prefix(LUAU_PAGE)
            .and_then(|rest| rest.split_once('/'))
        {
            let run = page_run(plugin, view);
            let id = run.as_ref().map(|(run, _, _)| run.id.clone());
            if kernel_action(own, args, id.as_ref(), &view.handle) {
                return;
            }
            let run = run.map(|(run, state, _)| {
                (run.id.0.to_string(), Some(run.repo.clone()), state)
            });
            luau_action(plugin, own, args, run, view);
            return;
        }
        if let Some((plugin, own)) = action
            .strip_prefix(LUAU_REPO)
            .and_then(|rest| rest.split_once('/'))
        {
            if kernel_action(own, args, None, &view.handle) {
                return;
            }
            if let Some(repo) = view.params.get(REPO_PARAM) {
                // With the page's own params, as an input's text comes
                // alone: which issue a page shows, say.
                let mut args = args.clone();
                if let Value::Object(fields) = &mut args {
                    let params: serde_json::Map<String, Value> = view
                        .params
                        .iter()
                        .filter(|(name, _)| {
                            *name != PAGE_PARAM && *name != REPO_PARAM
                        })
                        .map(|(name, value)| (name.clone(), json!(value)))
                        .collect();
                    fields.entry("params").or_insert(Value::Object(params));
                }
                view.handle.act(Act::RepoAction {
                    plugin: plugin.to_owned(),
                    repo: repo.clone(),
                    action: own.to_owned(),
                    args,
                });
            }
            return;
        }
        if crate::pane::act(action, args, view) {
            return;
        }
        page_action(&view.handle, action, args);
    }

    fn declaration(&self) -> tau_ui_plugin::Declaration {
        // The person's plugins come after the rules and the goal, so
        // theirs hold first.
        tau_ui_plugin::Declaration::new()
            .after(&["tau-constitution", "tau-goal"])
            .seams(&[
                tau_ui_plugin::Seam::Tools,
                tau_ui_plugin::Seam::Start,
                tau_ui_plugin::Seam::BeforeRequest,
                tau_ui_plugin::Seam::BeforeTool,
                tau_ui_plugin::Seam::AfterTool,
                tau_ui_plugin::Seam::Rewrite,
                tau_ui_plugin::Seam::Rewritten,
                tau_ui_plugin::Seam::StartIdle,
                tau_ui_plugin::Seam::OnEvent,
                tau_ui_plugin::Seam::BeforeStop,
                tau_ui_plugin::Seam::Finish,
            ])
            .fills(&crate::SLOTS)
            .services(&[
                tau_ui_plugin::services::names::WORKSPACE_DIR,
                tau_ui_plugin::services::names::IDENTITY,
                tau_ui_plugin::services::names::MODEL,
                tau_ui_plugin::services::names::CONVERSATIONS,
                tau_jev::SERVICE,
            ])
    }

    /// The plugins repository (ADR 0027): one, made the first time tau
    /// starts.
    fn repo_kinds(&self) -> Vec<tau_ui_plugin::kinds::KindHost> {
        vec![tau_ui_plugin::kinds::KindHost {
            kind: crate::KIND,
            root: crate::repository::ROOT,
            list: tau_ui_plugin::kinds::KindList::One { name: crate::REPO },
            first_files: crate::repository::first_files,
            sandboxed: false,
            without: &[],
        }]
    }

    fn manifest(&self) -> Manifest<Self> {
        let manifest = Manifest::new()
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
            .listed_commands(|overview, _| commands(overview), run_command)
            .status(State::status)
            // The plugins repository, whose chats the sidebar lists under
            // Plugins (ADR 0027).
            .contribute(points::REPO_KIND, |_: &AtApp, _| {
                Some(RepoKind {
                    kind: KIND.into(),
                    title: "Plugins".into(),
                    page: Link::page(PAGE),
                    new: None,
                    empty: String::new(),
                    place: KindPlace::Plugins,
                })
            })
            // What each plugin's `view` draws at the run's places.
            .contribute(points::RUN_BANNER, |_: &AtRun, view| {
                drawn(view.state?, "banner")
            })
            .contribute(points::INSPECTOR, |_: &AtRun, view| {
                drawn(view.state?, "inspector")
            })
            .contribute(points::COMPOSER_BAND, |_: &AtRun, view| {
                drawn(view.state?, "band")
            })
            .contribute(points::PLAN_STEPS, |_: &AtRun, view| {
                drawn(view.state?, "plan_steps")
            })
            .contribute(points::PLAN, |_: &AtRun, view| plan_field(view.state?))
            .contribute(points::PICKER_AUTO, |_: &AtApp, view| {
                let keyed = tau_jev::keyed(view);
                view.data.plugins.iter().find_map(|entry| {
                    let auto =
                        entry.declaration.as_ref()?.picker_auto.as_ref()?;
                    Some(if keyed {
                        auto.jev.clone()
                    } else {
                        auto.no_jev.clone()
                    })
                })
            })
            .contribute(points::RUN_ROW, |_: &AtRun, view| row(view.state?))
            .contribute(points::ASKS, |_: &AtRun, view| {
                let asks: Vec<&str> = view
                    .state?
                    .plugins
                    .values()
                    .filter(|plugin| plugin.off.is_none())
                    .filter_map(|plugin| plugin.view["asks"].as_str())
                    .collect();
                (!asks.is_empty()).then(|| asks.join(" "))
            })
            .contribute(points::CARD, |at: &AtCard, view| {
                if let Some(ask) =
                    view.state.and_then(|state| state.asks.get(&at.call_id))
                {
                    return Some(CardView {
                        body: Some(allow_card(ask, at)),
                        ..CardView::default()
                    });
                }
                let details = at.data.result.as_ref()?.details.as_ref()?;
                let plugin = details.get("plugin")?.as_str()?;
                let card =
                    details.get("card").filter(|card| !card.is_null())?;
                Some(CardView {
                    body: Some(embed(card, plugin)),
                    ..CardView::default()
                })
            })
            .contribute(points::TRANSCRIPT, |at: &AtAnchor, view| {
                let (plugin, note) = view.state?.notes.get(&at.key)?.clone();
                Some(Node::Card {
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
                Some(Node::Badge {
                    text: format!("flagged by {plugin}: {reason}"),
                    tone: Tone::Warn,
                })
            });
        // The plugins that ask for one, in the sidebar: the first few.
        let manifest = (0..NAV_ENTRIES).fold(manifest, |manifest, at| {
            manifest.contribute(points::SIDEBAR, move |_: &AtApp, view| {
                nav_entries(view.data).nth(at)
            })
        });
        // Under each repository, the plugins' entries for it.
        (0..NAV_ENTRIES).fold(manifest, |manifest, at| {
            manifest.contribute(
                points::SIDEBAR_REPO,
                move |at_repo: &AtRepo, view| {
                    let kept = view.repo(&at_repo.repo);
                    repo_nav_entries(view.data, kept, &at_repo.repo).nth(at)
                },
            )
        })
    }
}

/// The active plugins' slash commands, for the composer's menu.
fn commands(overview: &Overview) -> Vec<ListedCommand> {
    overview
        .plugins
        .iter()
        .filter(|entry| matches!(entry.standing, Standing::Active))
        .flat_map(|entry| {
            let declared = entry.declaration.iter().flat_map(|d| &d.commands);
            declared.map(|command| ListedCommand {
                name: command.name.clone(),
                hint: if command.description.is_empty() {
                    entry.name.clone()
                } else {
                    command.description.clone()
                },
                args: command.args.clone(),
                icon: Icon::Plug,
            })
        })
        .collect()
}

/// `/command args`: the plugin whose command it is carries it out on the
/// run it was typed in, as its action `/command`.
fn run_command(
    command: &str,
    args: &str,
    _repo: Option<&str>,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) {
    let Some(plugin) = view
        .data
        .plugins
        .iter()
        .filter(|entry| matches!(entry.standing, Standing::Active))
        .find(|entry| {
            entry
                .declaration
                .iter()
                .flat_map(|d| &d.commands)
                .any(|c| c.name == command)
        })
        .map(|entry| entry.name.clone())
    else {
        return;
    };
    let Some(run) = view.run.map(|run| run.id.0.to_string()) else {
        view.handle
            .note(format!("/{command} runs in a chat: open one first"));
        return;
    };
    // A run no plugin has drawn in yet starts from no state.
    let state = view.state.cloned().unwrap_or_default();
    luau_action(
        &plugin,
        &format!("/{command}"),
        &json!({ "text": args }),
        Some((run, view.run.map(|run| run.repo.clone()), state)),
        view,
    );
}

/// The active plugins' entries under the repository `repo`, each opening
/// its page there, with what it keeps of the repository: its `nav`'s
/// `detail`, and its `badge` with a `tone`.
fn repo_nav_entries<'a>(
    overview: &'a Overview,
    kept: Option<&'a crate::RepoValues>,
    repo: &'a str,
) -> impl Iterator<Item = NavEntry> + 'a {
    overview
        .plugins
        .iter()
        .filter(|entry| matches!(entry.standing, Standing::Active))
        .filter_map(move |entry| {
            let nav = entry.declaration.as_ref()?.repo_nav.as_ref()?;
            let icon = Icon::named(&nav.icon).unwrap_or(Icon::Plug);
            let said = kept
                .and_then(|values| values.0.get(&entry.name))
                .map(|value| &value["nav"]);
            let mut item = NavEntry::new(
                nav.label.clone(),
                icon,
                repo_page_link(&entry.name, &nav.page, repo),
            );
            item.detail = said
                .and_then(|nav| nav["detail"].as_str())
                .map(str::to_owned);
            item.badge =
                said.and_then(|nav| nav["badge"].as_str()).map(|badge| {
                    let tone = match nav_tone(said) {
                        Some(tone) => tone,
                        None => tau_ui_kit::theme::Tone::Quiet,
                    };
                    (badge.to_owned(), tone)
                });
            Some(item)
        })
}

fn nav_tone(nav: Option<&Value>) -> Option<tau_ui_kit::theme::Tone> {
    Some(match nav?["tone"].as_str()? {
        "good" => tau_ui_kit::theme::Tone::Good,
        "warn" => tau_ui_kit::theme::Tone::Warn,
        "bad" => tau_ui_kit::theme::Tone::Danger,
        "info" => tau_ui_kit::theme::Tone::Info,
        _ => return None,
    })
}

/// The most of the person's plugins the sidebar lists.
const NAV_ENTRIES: usize = 8;

/// The active plugins' sidebar entries, by name, each opening its page.
fn nav_entries(overview: &Overview) -> impl Iterator<Item = NavEntry> + '_ {
    overview
        .plugins
        .iter()
        .filter(|entry| matches!(entry.standing, Standing::Active))
        .filter_map(|entry| {
            let nav = entry.declaration.as_ref()?.nav.as_ref()?;
            let icon = Icon::named(&nav.icon).unwrap_or(Icon::Plug);
            Some(NavEntry::new(
                nav.label.clone(),
                icon,
                entry_link(&entry.name),
            ))
        })
}

/// The page of the person's own plugins (ADR 0036): each plugin of the
/// plugins repository as a card, with where it stands, and the chats
/// writing them. Opened for one plugin, the page is that plugin's. A
/// view tree in the frame the interface gives a page (ADR 0045).
fn page(view: &mut ViewCx<'_, LuauPluginsUi>) -> Node {
    if let (Some(entry), Some(page), Some(repo)) = (
        view.entry().map(str::to_owned),
        view.params.get(PAGE_PARAM).cloned(),
        view.params.get(REPO_PARAM).cloned(),
    ) {
        return repo_page(&entry, &page, &repo, view);
    }
    match view.entry().map(str::to_owned) {
        Some(entry) => plugin_page(&entry, view),
        None => overview(view),
    }
}

/// `plugin`'s page `page` of `repo`, drawn by its host half: fetched
/// again whenever what it keeps of the repository changes.
fn repo_page(
    plugin: &str,
    page: &str,
    repo: &str,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) -> Node {
    let kept = view
        .repo(repo)
        .and_then(|values| values.0.get(plugin))
        .map(Value::to_string)
        .unwrap_or_default();
    let params = view.params.clone();
    Node::Fetched {
        key: format!("luau-repo-page/{plugin}/{}/{kept}", json!(params)),
        fetch: json!(Act::Page {
            plugin: plugin.to_owned(),
            page: page.to_owned(),
            params,
        }),
    }
}

/// The link to `plugin`'s page `page` of `repo`.
pub fn repo_page_link(plugin: &str, page: &str, repo: &str) -> Link {
    entry_link(plugin)
        .param(PAGE_PARAM, page)
        .param(REPO_PARAM, repo)
}

/// Carries out one of the page's own actions.
fn page_action(handle: &Handle, action: &str, args: &Value) {
    match action {
        WRITE => handle.composer(format!("/{COMMAND} ")),
        ALLOW => {
            if let Some(plugin) = args["plugin"].as_str() {
                handle.act(Act::Allow {
                    plugin: plugin.to_owned(),
                });
            }
        }
        OPEN_RUN => {
            if let Some(run) = args["run"].as_str() {
                handle.open_run(&RunId(run.into()));
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

/// What every plugin on in the run drew as `key` of its view, each in
/// its own actions; one under the other when several did.
/// The plan's field a plugin's view sets (`plan = { name, value }`):
/// the first plugin's that sets one, as its own.
fn plan_field(state: &State) -> Option<tau_ui_plugin::PlanField> {
    state
        .plugins
        .iter()
        .filter(|(_, plugin)| plugin.off.is_none())
        .find_map(|(name, plugin)| {
            let field = plugin.view.get("plan")?;
            Some(tau_ui_plugin::PlanField {
                name: field["name"].as_str()?.to_owned(),
                value: field["value"].as_str()?.to_owned(),
                set_by: Some(name.clone()),
            })
        })
}

fn drawn(state: &State, key: &str) -> Option<Node> {
    let mut trees: Vec<Node> = state
        .plugins
        .iter()
        .filter(|(_, plugin)| plugin.off.is_none())
        .filter_map(|(name, plugin)| {
            let piece =
                plugin.view.get(key).filter(|piece| !piece.is_null())?;
            Some(embed(piece, name))
        })
        .collect();
    match trees.len() {
        0 => None,
        1 => trees.pop(),
        _ => Some(Node::stack(trees)),
    }
}

/// The run's row in the sidebar, from the first plugin whose view says
/// `row = { line, count?, tone? }`.
fn row(state: &State) -> Option<RowNote> {
    state
        .plugins
        .values()
        .filter(|plugin| plugin.off.is_none())
        .find_map(|plugin| {
            let row = plugin.view.get("row")?;
            let line = row["line"].as_str()?.to_owned();
            Some(RowNote {
                phone_line: Some(line.clone()),
                line: Some(line),
                count: row["count"].as_str().map(str::to_owned),
                icon: Icon::Plug,
                tone: match row["tone"].as_str() {
                    Some("good") => tau_ui_kit::theme::Tone::Good,
                    Some("warn") => tau_ui_kit::theme::Tone::Warn,
                    Some("bad") => tau_ui_kit::theme::Tone::Danger,
                    _ => tau_ui_kit::theme::Tone::Quiet,
                },
            })
        })
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
/// or a repository's own.
fn plugin_page(entry: &str, view: &mut ViewCx<'_, LuauPluginsUi>) -> Node {
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
        return Node::stack([back, empty]);
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
    if let Some((run, _, drawn)) = page_run(entry, view) {
        let title = if run.title.is_empty() {
            "In its last run".to_owned()
        } else {
            format!("In {}", run.title)
        };
        page.extend(section(&title, embed_as(&drawn, entry, LUAU_PAGE)));
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
    if has_settings {
        page.push(Node::Heading {
            text: "Settings".into(),
        });
        page.extend(scopes(entry, view));
        page.push(crate::pane::render(view));
    }
    Node::stack(page)
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
        view.handle.composer("/plugin ");
        return;
    }
    view.handle
        .start(crate::REPO, format!("/{} {args}", crate::SKILL));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A call that asks the person to allow shows Allow and Deny while
    /// it waits and its run goes on, each sending the person's answer;
    /// then what came of it, and a run that ended withdraws it.
    #[test]
    fn an_ask_draws_its_buttons_while_it_waits() {
        use crate::Outcome;
        let mut state = State::default();
        let mut run_cx = tau_ui_plugin::testing::FakeRun::default();
        state.apply(
            Record::Asked {
                plugin: "todo".into(),
                call_id: "c1".into(),
                title: "Start a chat on td-1".into(),
                text: String::new(),
            },
            &mut run_cx,
        );
        let at = |live: bool| AtCard {
            run: RunInfo {
                id: RunId("r1".into()),
                repo: "home".into(),
                live,
                title: String::new(),
                answer: None,
                context: 0,
                window: None,
            },
            call_id: "c1".into(),
            tool: "todo_dispatch".into(),
            keys: Vec::new(),
            data: Default::default(),
            summary: String::new(),
            cut: None,
        };
        let buttons = |node: &Node| -> Vec<(String, Value)> {
            let text = serde_json::to_value(node).unwrap();
            let mut found = Vec::new();
            fn walk(value: &Value, found: &mut Vec<(String, Value)>) {
                if value["piece"] == "button" {
                    found.push((
                        value["label"].as_str().unwrap().to_owned(),
                        value["args"].clone(),
                    ));
                }
                for child in value["children"].as_array().into_iter().flatten()
                {
                    walk(child, found);
                }
            }
            walk(&text, &mut found);
            found
        };
        let ask = &state.asks["c1"];
        assert_eq!(
            buttons(&allow_card(ask, &at(true))),
            [
                (
                    "Allow".to_owned(),
                    json!({ "run": "r1", "call": "c1", "allowed": true })
                ),
                (
                    "Deny".to_owned(),
                    json!({ "run": "r1", "call": "c1", "allowed": false })
                ),
            ]
        );
        // The run ended: no one can answer any more.
        assert!(buttons(&allow_card(ask, &at(false))).is_empty());
        state.apply(
            Record::Answered {
                plugin: "todo".into(),
                call_id: "c1".into(),
                outcome: Outcome::Allowed,
            },
            &mut run_cx,
        );
        let drawn = allow_card(&state.asks["c1"], &at(true));
        assert!(buttons(&drawn).is_empty());
        assert!(
            serde_json::to_string(&drawn).unwrap().contains("Allowed"),
            "{drawn:?}"
        );
    }

    /// What an action asked the interface to do once done
    /// (`tau.interface`) is done after its records are kept, on its run.
    #[test]
    fn an_action_may_ask_the_interface_when_done() {
        use std::{cell::RefCell, rc::Rc};

        use tau_ui_plugin::Request;
        let asked: Rc<RefCell<Vec<Request>>> = Rc::default();
        let sink = asked.clone();
        let handle = Handle::new(
            NAME,
            Rc::new(move |_, request| sink.borrow_mut().push(request)),
        );
        let done = ActionDone {
            plugin: "p".into(),
            run: "r1".into(),
            state: json!({ "answered": true }),
            view: None,
            logs: Vec::new(),
            error: None,
            interface: Some(json!({
                "action": "composer",
                "args": { "text": "about the note" },
            })),
        };
        LuauPluginsUi.reply(serde_json::to_value(&done).unwrap(), &handle);
        let asked = asked.borrow();
        assert!(
            matches!(asked.first(), Some(Request::Record { .. })),
            "{asked:?}"
        );
        assert_eq!(
            asked.last(),
            Some(&Request::Composer("about the note".into()))
        );
    }

    /// A tree's `tau.` actions ask the interface, on the run the tree is
    /// drawn in; the plugin's own are not, and an unknown one says so.
    #[test]
    fn a_trees_tau_actions_ask_the_interface() {
        use std::{cell::RefCell, rc::Rc};

        use tau_ui_plugin::Request;
        let asked: Rc<RefCell<Vec<Request>>> = Rc::default();
        let sink = asked.clone();
        let handle = Handle::new(
            NAME,
            Rc::new(move |_, request| sink.borrow_mut().push(request)),
        );
        let run = RunId("r1".into());
        assert!(kernel_action(
            "tau.steer",
            &json!({ "text": "stop" }),
            Some(&run),
            &handle
        ));
        assert!(kernel_action(
            "tau.send",
            &json!({ "text": "go on" }),
            Some(&run),
            &handle
        ));
        assert!(kernel_action(
            "tau.composer",
            &json!({ "text": "draft" }),
            None,
            &handle
        ));
        // Steering needs a run going on.
        assert!(kernel_action(
            "tau.steer",
            &json!({ "text": "lost" }),
            None,
            &handle
        ));
        assert!(!kernel_action("reset", &json!({}), Some(&run), &handle));
        assert!(kernel_action("tau.nope", &json!({}), None, &handle));
        let asked = asked.borrow();
        assert_eq!(
            asked[..3],
            [
                Request::Steer {
                    run: run.clone(),
                    text: "stop".into()
                },
                Request::Send {
                    run: Some(run.clone()),
                    text: "go on".into()
                },
                Request::Composer("draft".into()),
            ]
        );
        assert_eq!(asked.len(), 4, "{asked:?}");
    }

    fn with_views(views: &[(&str, Value)]) -> State {
        State {
            plugins: views
                .iter()
                .map(|(name, view)| {
                    let part = PluginState {
                        view: view.clone(),
                        ..PluginState::default()
                    };
                    ((*name).to_owned(), part)
                })
                .collect(),
            ..State::default()
        }
    }

    /// What plugins' views draw at a run's place is each plugin's, in its
    /// own actions, one under the other; a plugin that is off draws
    /// nothing, and none drawn is nothing.
    #[test]
    fn a_runs_places_hold_what_its_plugins_drew() {
        let banner = |text: &str| json!({ "piece": "button", "label": text, "action": "go" });
        let mut state = with_views(&[
            ("a", json!({ "banner": banner("A") })),
            (
                "b",
                json!({ "banner": banner("B"), "row": { "line": "3 left", "tone": "warn" } }),
            ),
        ]);
        let Some(Node::Stack { children }) = drawn(&state, "banner") else {
            panic!("{:?}", drawn(&state, "banner"));
        };
        let actions: Vec<&str> = children
            .iter()
            .map(|child| match child {
                Node::Button { action, .. } => action.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(actions, [format!("{LUAU}a/go"), format!("{LUAU}b/go")]);
        assert_eq!(drawn(&state, "inspector"), None);
        let drawn_row = row(&state).unwrap();
        assert_eq!(drawn_row.line.as_deref(), Some("3 left"));
        assert_eq!(drawn_row.tone, tau_ui_kit::theme::Tone::Warn);

        state.plugins.get_mut("b").unwrap().off = Some("failed".into());
        assert!(matches!(drawn(&state, "banner"), Some(Node::Button { .. })));
        assert_eq!(row(&state), None);
    }

    /// Each active plugin's commands are in the composer's menu, with
    /// what they do and take; a waiting plugin's are not.
    #[test]
    fn active_plugins_list_their_commands() {
        let entry = |name: &str, standing: Standing| Entry {
            name: name.into(),
            description: String::new(),
            standing,
            declaration: Some(Declaration {
                commands: vec![crate::CommandSpec {
                    name: format!("{name}-go"),
                    description: String::new(),
                    args: "<what>".into(),
                }],
                ..Declaration::named(name)
            }),
            tests: Vec::new(),
            keeps_earlier: false,
            built_in: false,
        };
        let overview = Overview {
            plugins: vec![
                entry("ships", Standing::Active),
                entry("new", Standing::Waiting { grown: Vec::new() }),
            ],
            ..Overview::default()
        };
        let listed = commands(&overview);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "ships-go");
        assert_eq!(listed[0].hint, "ships");
        assert_eq!(listed[0].args, "<what>");
    }

    /// Each active plugin that asks for a sidebar entry has one, opening
    /// its page; a waiting or broken one has none.
    #[test]
    fn active_plugins_with_nav_are_in_the_sidebar() {
        let entry =
            |name: &str, standing: Standing, nav: Option<crate::Nav>| Entry {
                name: name.into(),
                description: String::new(),
                standing,
                declaration: Some(Declaration {
                    nav,
                    ..Declaration::named(name)
                }),
                tests: Vec::new(),
                keeps_earlier: false,
                built_in: false,
            };
        let nav = |label: &str| {
            Some(crate::Nav {
                label: label.into(),
                icon: "not-an-icon".into(),
            })
        };
        let overview = Overview {
            plugins: vec![
                entry("deploys", Standing::Active, nav("Deploys")),
                entry("quiet", Standing::Active, None),
                entry("new", Standing::Failing, nav("New")),
            ],
            ..Overview::default()
        };
        let entries: Vec<NavEntry> = nav_entries(&overview).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Deploys");
        assert_eq!(entries[0].icon, Icon::Plug);
        assert_eq!(entries[0].to, entry_link("deploys"));
    }

    /// A view's `plan` is a field of the run's plan, set by the plugin
    /// whose view it is; a view without one sets none.
    #[test]
    fn a_views_plan_is_the_plans_field() {
        let mut state = State::default();
        let mut run = tau_ui_plugin::testing::FakeRun::default();
        state.apply(
            Record::View {
                plugin: "quiet".into(),
                view: json!({}),
            },
            &mut run,
        );
        assert_eq!(plan_field(&state), None);
        state.apply(
            Record::View {
                plugin: "picks".into(),
                view: json!({ "plan": { "name": "reasoning", "value": "high" } }),
            },
            &mut run,
        );
        assert_eq!(
            plan_field(&state),
            Some(tau_ui_plugin::PlanField {
                name: "reasoning".into(),
                value: "high".into(),
                set_by: Some("picks".into()),
            })
        );
    }

    /// A keyed note is placed where the run is when its key first
    /// comes, and drawn again in place as it changes: a later key is
    /// placed later, and the earlier stays where it was.
    #[test]
    fn keyed_notes_stay_where_they_were_placed() {
        let mut state = State::default();
        let mut run = tau_ui_plugin::testing::FakeRun::default();
        let view = |notes: Value| Record::View {
            plugin: "w".into(),
            view: json!({ "notes": notes }),
        };
        for record in [
            view(json!({})),
            view(json!({ "n0": "new" })),
            view(json!({ "n0": "answered" })),
            view(json!({ "n0": "answered", "n1": "second" })),
            view(json!({ "n0": "answered", "n1": "second", "n2": null })),
        ] {
            state.apply(record, &mut run);
        }
        assert_eq!(run.anchors, ["note-w-n0", "note-w-n1"]);
        assert_eq!(state.notes["note-w-n0"].1, json!("answered"));
        assert_eq!(state.notes["note-w-n1"].1, json!("second"));
        assert!(!state.notes.contains_key("note-w-n2"));
    }

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

    /// A Luau plugin's tree keeps its state under the plugin's name, and
    /// its actions name the plugin, so the plugin's `on_action` knows
    /// whose they are.
    #[test]
    fn an_embedded_tree_is_the_plugin_s() {
        let piece = json!({ "piece": "stack", "children": [
            { "piece": "button", "label": "Add", "action": "add" },
            { "piece": "section", "id": "more", "title": "More", "children": [] },
        ] });
        let Node::Stack { children } = embed(&piece, "p") else {
            panic!("not a stack");
        };
        assert!(matches!(
            &children[0],
            Node::Button { action, .. } if action == "luau/p/add"
        ));
        assert!(matches!(
            &children[1],
            Node::Section { id, .. } if id == "p/more"
        ));
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
            interface: None,
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
