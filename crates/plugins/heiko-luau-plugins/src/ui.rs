//! heiko-luau-plugins' UI (ADR 0017, 0027): the records folded into each
//! run's state, the view trees Luau plugins return drawn with the shared
//! tree (ADR 0045), the run's status line, the cards of the plugins' own
//! tools, and a badge on calls a plugin flagged.
//!
//! The drawing reads JSON only, so the phone, which runs no Luau, draws
//! what the computer does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use heiko_agent::tool::RunId;
use heiko_ui_kit::assets::Icon;
use heiko_ui_plugin::{
    CardMark,
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
    points::{
        self,
        AtAnchor,
        AtApp,
        AtCard,
        AtMessage,
        AtRepo,
        AtRun,
        AtSetup,
        CardView,
        HomeAction,
        HomeList,
        HomeRow,
        SetupAt,
        SetupRepo,
        SetupView,
    },
    tree::{Ink, Kind, MarkState, Node, SquareLook, Tab, Tone, draft_key},
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

/// heiko-luau-plugins with its UI.
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
    /// Calls its view marked on their cards, by call id: how, as the
    /// cards were last marked.
    #[serde(default)]
    pub marked: BTreeMap<String, CardMark>,
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
                // A call its view marks shows so on its card, refused or
                // flagged, as a Rust plugin's verdict does.
                for (call_id, drawn) in view
                    .get("calls")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    // What a rewrite left of the call, and its result cut
                    // as it arrived, as a Rust plugin's fold says them.
                    match drawn.get("dropped").and_then(Value::as_str) {
                        Some("result") => {
                            run.dropped(
                                call_id,
                                heiko_ui_plugin::Dropped::Result,
                            );
                        }
                        Some("call") => {
                            run.dropped(call_id, heiko_ui_plugin::Dropped::Call);
                        }
                        _ => {}
                    }
                    if let Some(cut) = drawn
                        .get("cut")
                        .cloned()
                        .and_then(|cut| serde_json::from_value(cut).ok())
                    {
                        run.cut(call_id, cut);
                    }
                    let mark = match drawn.get("mark").and_then(Value::as_str) {
                        Some("blocked") => CardMark::Blocked {
                            reason: drawn["reason"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                            by: Some(plugin.clone()),
                        },
                        Some("flagged") => CardMark::Flagged,
                        _ => continue,
                    };
                    let part = self.of(plugin.clone());
                    if part.marked.get(call_id) != Some(&mark)
                        && run.mark(call_id, mark.clone())
                    {
                        part.marked.insert(call_id.clone(), mark);
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
            // The mailbox's journal is the run's to take again after a
            // restart; an interface shows the state.
            Record::Checkpoint { .. }
            | Record::Taken { .. }
            | Record::Quiet { .. } => {}
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
/// one of heiko-luau-plugins' own: they are the plugin's, not the page's.
/// After it come the plugin's name, `/`, and its own action. A tree on a
/// card or in the transcript acts on the run it is drawn in.
const LUAU: &str = "luau/";

/// [`LUAU`] for the tree on a plugin's own page, which acts on the newest
/// run that drew one ([`page_run`]).
const LUAU_PAGE: &str = "luau-page/";

/// What the actions of a Luau plugin's page of a repository start with
/// ([`embed_repo`]): carried out in the repository the page shows.
const LUAU_REPO: &str = "luau-repo/";

/// The mark of an action of a plugin's page of the app (ADR 0050, gap K).
const LUAU_APP: &str = "luau-app/";

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
        Node::Band { children } => Node::Band {
            children: each(children, f),
        },
        Node::Rail { children } => Node::Rail {
            children: each(children, f),
        },
        Node::Header {
            title,
            meta,
            actions,
        } => Node::Header {
            title,
            meta,
            actions: each(actions, f),
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
                .map(|column| heiko_ui_plugin::tree::BoardColumn {
                    cards: column
                        .cards
                        .into_iter()
                        .map(|card| heiko_ui_plugin::tree::BoardCard {
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
/// heiko-luau-plugins' own (ADR 0045): its sections, tabs, inputs and
/// widgets keep their state under `plugin`'s name, and its actions are
/// marked as the plugin's. A piece it does not know draws as a red box
/// that says so.
pub fn embed(piece: &Value, plugin: &str) -> Node {
    own_links(embed_as(piece, plugin, LUAU), plugin)
}

/// `node`'s links to `plugin`'s own pages, opening them: of the
/// repository a link names, or of the app when it names none (ADR 0050,
/// gap L).
fn own_links(node: Node, plugin: &str) -> Node {
    let own = |to: Link| {
        if to.plugin.is_some() {
            return to;
        }
        let mut link = match to.params.get(REPO_PARAM) {
            Some(repo) => repo_page_link(plugin, &to.page, repo),
            None => app_page_link(plugin, &to.page),
        };
        for (name, value) in to.params {
            if name != PAGE_PARAM && name != REPO_PARAM {
                link = link.param(name, value);
            }
        }
        link
    };
    rewrite(node, &mut |node| match node {
        Node::Link { label, to } => Node::Link { label, to: own(to) },
        Node::Slip {
            text,
            tone,
            dashed,
            to,
        } => Node::Slip {
            text,
            tone,
            dashed,
            to: to.map(own),
        },
        Node::Graph {
            height,
            nodes,
            edges,
        } => Node::Graph {
            height,
            nodes: nodes
                .into_iter()
                .map(|mut node| {
                    node.to = node.to.map(own);
                    node
                })
                .collect(),
            edges,
        },
        other => other,
    })
}

/// `piece`, a Luau plugin's page of the app (ADR 0050, gap L), as a tree
/// whose actions are the plugin's outside any run and repository, and
/// whose links open its pages: of a repository a link names, or of the
/// app.
pub fn embed_app(piece: &Value, plugin: &str) -> Node {
    own_links(embed_as(piece, plugin, LUAU_APP), plugin)
}

/// `piece`, a Luau plugin's page of `repo`, as a tree whose actions are
/// carried out in that repository, and whose links to the plugin's own
/// pages open them of that repository.
pub fn embed_repo(piece: &Value, plugin: &str, repo: &str) -> Node {
    let own = |to: Link| {
        if to.plugin.is_some() {
            return to;
        }
        let mut link = repo_page_link(plugin, &to.page, repo);
        for (name, value) in to.params {
            if name != PAGE_PARAM && name != REPO_PARAM {
                link = link.param(name, value);
            }
        }
        link
    };
    rewrite(embed_as(piece, plugin, LUAU_REPO), &mut |node| match node {
        Node::Link { label, to } => Node::Link { label, to: own(to) },
        Node::Slip {
            text,
            tone,
            dashed,
            to,
        } => Node::Slip {
            text,
            tone,
            dashed,
            to: to.map(own),
        },
        Node::Graph {
            height,
            nodes,
            edges,
        } => Node::Graph {
            height,
            nodes: nodes
                .into_iter()
                .map(|mut node| {
                    node.to = node.to.map(own);
                    node
                })
                .collect(),
            edges,
        },
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
        } => {
            let id = own(id);
            // Enter in it sends the composer, keeping what it says.
            let submit = submit.map(|submit| match submit.as_str() {
                SUBMIT => act(format!("{SUBMIT}/{id}")),
                _ => act(submit),
            });
            Node::Input {
                id,
                placeholder,
                multiline,
                submit,
                keep,
                secret,
                focus,
            }
        }
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
        // A blob of the plugin's, which its host half reads.
        Node::Image {
            key,
            mime,
            fetch,
            height,
        } => Node::Image {
            key: own(key),
            mime,
            fetch: serde_json::to_value(crate::Act::Blob {
                plugin: plugin.to_owned(),
                repo: fetch["repo"].as_str().unwrap_or_default().to_owned(),
                blob: fetch["blob"].clone(),
            })
            .unwrap_or_default(),
            height,
        },
        Node::Button {
            label,
            action,
            args,
            kind,
            icon,
            size,
            wide,
            detail,
        } => Node::Button {
            label,
            action: act(action),
            args,
            kind,
            icon,
            size,
            wide,
            detail,
        },
        Node::Square {
            label,
            style,
            tone,
            action,
            args,
        } => Node::Square {
            label,
            style,
            tone,
            action: action.map(act),
            args,
        },
        Node::Segments {
            choices,
            picked,
            action,
            args,
        } => Node::Segments {
            choices,
            picked,
            action: act(action),
            args,
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
/// An input's `submit` that sends the composer, as Enter there would.
const SUBMIT: &str = "heiko.submit";

/// What a Luau tree's action starting with `heiko.` asks of the interface,
/// on `run`, the run the tree is drawn in or its page shows: whether it
/// was one.
fn kernel_action(
    action: &str,
    args: &Value,
    run: Option<&RunId>,
    handle: &Handle,
) -> bool {
    let Some(kernel) = action.strip_prefix("heiko.") else {
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
        "pane" => handle.open_pane(),
        // A sign-in's code to paste, the page to paste it on, and setup's
        // step done (ADR 0050, gap E).
        "copy" => {
            handle.copy(text("text"));
            // And the page to paste it on, when it says one.
            let url = text("url");
            if url.starts_with("https://") {
                handle.open_url(url);
            }
        }
        "open_url" => {
            let url = text("url");
            if url.starts_with("https://") {
                handle.open_url(url);
            }
        }
        "setup_done" => handle.setup_done(),
        "open_run" => handle.open_run(&RunId(text("run").into())),
        // Stops the run, as its Stop button does.
        "cancel" => {
            if let Some(run) = run {
                handle.cancel(run);
            }
        }
        // Enter in an input of a command's popover: what it says stays
        // its draft, which Enter would clear, and the composer is sent.
        submit if submit.starts_with("submit/") => {
            let id = &submit["submit/".len()..];
            let said = text("text");
            handle
                .set_ui(draft_key(id), (!said.is_empty()).then(|| said.into()));
            handle.request(heiko_ui_plugin::Request::Submit);
        }
        other => {
            handle.note(format!("`heiko.{other}` is not one of heiko's actions"))
        }
    }
    true
}

/// `heiko.open`, which opens one of `plugin`'s own pages: `{ page, params,
/// repo }`, of the repository `repo` names or of the app without one
/// (ADR 0050, gap L). Whether `action` was it.
fn open_own(plugin: &str, action: &str, args: &Value, handle: &Handle) -> bool {
    if action != "heiko.open" {
        return false;
    }
    let page = args["page"].as_str().unwrap_or_default();
    let mut link = match args["repo"].as_str().filter(|repo| !repo.is_empty()) {
        Some(repo) => repo_page_link(plugin, page, repo),
        None => app_page_link(plugin, page),
    };
    if let Some(params) = args["params"].as_object() {
        for (name, value) in params {
            if name == PAGE_PARAM || name == REPO_PARAM {
                continue;
            }
            let value = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            link = link.param(name.clone(), value);
        }
    }
    handle.navigate(link);
    true
}

/// What a call asked the person to allow: what it would do, and while
/// it waits and its run goes on, Allow and Deny; then what came of it.
fn allow_card(ask: &crate::Ask, at: &AtCard) -> Node {
    use crate::Outcome;
    let mut children = vec![Node::row([
        mark(MarkState::Asks),
        Node::Title {
            text: ask.title.clone(),
            subtitle: None,
        },
        small(ask.plugin.clone()),
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
                    size: Default::default(),
                    wide: false,
                    detail: None,
                },
                Node::Button {
                    label: "Deny".into(),
                    action: ANSWER.into(),
                    args: answer(false),
                    kind: Kind::Secondary,
                    icon: None,
                    size: Default::default(),
                    wide: false,
                    detail: None,
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
    let live = view
        .run
        .filter(|info| info.id.0.as_ref() == run)
        .map_or_else(
            || {
                view.runs()
                    .iter()
                    .any(|(info, _)| info.id.0.as_ref() == run && info.live)
            },
            |info| info.live,
        );
    view.handle.act(Act::Action {
        plugin: plugin.to_owned(),
        action: action.to_owned(),
        args: args.clone(),
        run,
        repo,
        state,
        settings: view.settings.plugins.get(plugin).cloned(),
        live,
        jev: heiko_jev::keyed(view),
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

    fn read_prompt(&self, prompt: &str) -> Option<String> {
        read_command(prompt)
    }

    /// What an action left.
    fn reply(&self, reply: Value, handle: &Handle) {
        if let Ok(done) = serde_json::from_value::<ActionDone>(reply.clone()) {
            // What an action left, with the run it was carried out on;
            // a command typed outside a chat has none to keep it.
            let run =
                (!done.run.is_empty()).then(|| RunId(done.run.as_str().into()));
            if let Some(run) = &run {
                for record in done.records() {
                    handle.record(run, record);
                }
            }
            // What it asked the interface to do, on its run.
            if let Some(asked) = &done.interface {
                let action = asked["action"].as_str().unwrap_or_default();
                if open_own(
                    &done.plugin,
                    &format!("heiko.{action}"),
                    &asked["args"],
                    handle,
                ) {
                    return;
                }
                kernel_action(
                    &format!("heiko.{action}"),
                    &asked["args"],
                    run.as_ref(),
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
            if open_own(plugin, own, args, &view.handle) {
                return;
            }
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
            if open_own(plugin, own, args, &view.handle) {
                return;
            }
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
            .strip_prefix(LUAU_APP)
            .and_then(|rest| rest.split_once('/'))
        {
            if own == "heiko.reveal" {
                reveal(plugin, args, view);
                return;
            }
            // `ui.secret`'s Enter: the text to the store, past the plugin.
            if let Some(name) = own.strip_prefix("heiko.secret/") {
                if let Some(value) =
                    args["text"].as_str().filter(|v| !v.is_empty())
                {
                    view.handle.act(Act::SetSecret {
                        plugin: plugin.to_owned(),
                        name: name.to_owned(),
                        value: value.to_owned(),
                    });
                }
                return;
            }
            if open_own(plugin, own, args, &view.handle) {
                return;
            }
            if kernel_action(own, args, None, &view.handle) {
                return;
            }
            view.handle.act(Act::AppAction {
                plugin: plugin.to_owned(),
                action: own.to_owned(),
                args: args.clone(),
            });
            return;
        }
        if let Some((plugin, own)) = action
            .strip_prefix(LUAU_REPO)
            .and_then(|rest| rest.split_once('/'))
        {
            if open_own(plugin, own, args, &view.handle) {
                return;
            }
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

    fn declaration(&self) -> heiko_ui_plugin::Declaration {
        heiko_ui_plugin::Declaration::new()
            .seams(&[
                heiko_ui_plugin::Seam::Tools,
                heiko_ui_plugin::Seam::Start,
                heiko_ui_plugin::Seam::BeforeRequest,
                heiko_ui_plugin::Seam::BeforeTool,
                heiko_ui_plugin::Seam::AfterTool,
                heiko_ui_plugin::Seam::Rewrite,
                heiko_ui_plugin::Seam::Rewritten,
                heiko_ui_plugin::Seam::StartIdle,
                heiko_ui_plugin::Seam::OnEvent,
                heiko_ui_plugin::Seam::BeforeStop,
                heiko_ui_plugin::Seam::Finish,
            ])
            .fills(&crate::SLOTS)
            .services(&[
                heiko_ui_plugin::services::names::WORKSPACE_DIR,
                heiko_ui_plugin::services::names::IDENTITY,
                heiko_ui_plugin::services::names::MODEL,
                heiko_ui_plugin::services::names::CONVERSATIONS,
                // Each turn's commit, for `turn_end` (ADR 0050, gap L).
                heiko_ui_plugin::services::names::TURN_HOOKS,
                heiko_jev::SERVICE,
            ])
    }

    /// The plugins repository (ADR 0027): one, made the first time heiko
    /// starts.
    fn repo_kinds(&self) -> Vec<heiko_ui_plugin::kinds::KindHost> {
        vec![heiko_ui_plugin::kinds::KindHost {
            kind: crate::KIND,
            root: crate::repository::ROOT,
            list: heiko_ui_plugin::kinds::KindList::One { name: crate::REPO },
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
            .listed_popover(command_popover)
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
            .contribute(points::PANE, |_: &AtRun, view| pane(view.state?))
            .contribute(points::RUN_BANNER, |_: &AtRun, view| {
                drawn(view.state?, "banner")
            })
            .contribute(points::INSPECTOR, |_: &AtRun, view| {
                drawn(view.state?, "inspector")
            })
            .contribute(points::COMPOSER_BAND, |_: &AtRun, view| {
                drawn(view.state?, "band")
            })
            // In the composer's place while the run goes on: the first
            // plugin's, by name, that draws there, as only one may.
            .contribute(points::COMPOSER, |at: &AtRun, view| {
                if !at.run.live {
                    return None;
                }
                first_drawn(view.state?, "composer")
            })
            .contribute(points::PLAN_STEPS, |_: &AtRun, view| {
                drawn(view.state?, "plan_steps")
            })
            .contribute(points::PLAN, |_: &AtRun, view| plan_field(view.state?))
            .contribute(points::PICKER_AUTO, |_: &AtApp, view| {
                let keyed = heiko_jev::keyed(view);
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
                if let Some(header) =
                    view.state.and_then(|state| card_header(state, &at.call_id))
                {
                    return Some(header);
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
                    children: vec![embed(&note, &plugin)],
                })
            })
            .contribute(points::USER_MESSAGE, |at: &AtMessage, view| {
                user_message(view.state?, &at.text)
            })
            .contribute(points::CARD_BADGE, |at: &AtCard, view| {
                let state: &State = view.state?;
                let flag = state.plugins.iter().find_map(|(name, plugin)| {
                    let reason = plugin.flags.get(&at.call_id)?;
                    Some(Node::Badge {
                        text: format!("flagged by {name}: {reason}"),
                        tone: Tone::Warn,
                    })
                });
                let drawn = on_card(state, &at.call_id, "badge");
                match (flag, drawn) {
                    (Some(flag), Some(drawn)) => Some(Node::row([flag, drawn])),
                    (flag, drawn) => flag.or(drawn),
                }
            })
            .contribute(points::CARD_BODY, |at: &AtCard, view| {
                on_card(view.state?, &at.call_id, "body")
            })
            // A built-in's step of first-run setup and its section of the
            // Models screen, drawn of what its `app` keeps (gap E).
            .contribute(points::MODELS, |_: &AtApp, view| {
                models_section(view.data)
            });
        // Each of heiko's own plugins' steps of first-run setup.
        let manifest = (0..NAV_ENTRIES).fold(manifest, |manifest, nth| {
            manifest.contribute(points::SETUP, move |at: &AtSetup, view| {
                setup_step(view.data, nth, &at.repos)
            })
        });
        // The plugins that ask for one, in the sidebar: the first few.
        let manifest = (0..NAV_ENTRIES).fold(manifest, |manifest, at| {
            manifest.contribute(points::SIDEBAR, move |_: &AtApp, view| {
                nav_entries(view.data).nth(at)
            })
        });
        // In each repository's menu, the plugins' entries for it, and
        // what their `app` adds there (ADR 0050, gap L).
        let manifest = (0..NAV_ENTRIES).fold(manifest, |manifest, nth| {
            manifest.contribute_at(
                points::REPO_MENU,
                -10,
                move |at: &AtRepo, view| {
                    repo_menu(view.data, &at.repo).nth(nth)
                },
            )
        });
        // Each key heiko leaves to Luau plugins, to the first that binds it.
        let manifest = crate::KEYS.iter().fold(manifest, |manifest, key| {
            manifest.key(key, move |at, view| {
                let Some(repo) = at.repo.clone() else {
                    return;
                };
                let bound = view.data.plugins.iter().find_map(|entry| {
                    if !matches!(entry.standing, Standing::Active) {
                        return None;
                    }
                    let page = entry.declaration.as_ref()?.keys.get(*key)?;
                    Some(repo_page_link(&entry.name, page, &repo))
                });
                if let Some(link) = bound {
                    view.handle.navigate(link);
                }
            })
        });
        // On Home, what each plugin lists of the selected repository.
        let manifest = (0..NAV_ENTRIES).fold(manifest, |manifest, nth| {
            manifest.contribute(points::HOME, move |at: &AtRepo, view| {
                home_lists(view.data, view.repo(&at.repo)).nth(nth)
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

/// The step of first-run setup one of heiko's own plugins draws (ADR 0050,
/// gap E): `{ step, at, title, heading, detail, body }`, `at` one of
/// `model`, `before_repositories` and `after_repositories`. A person's
/// plugin draws none: setup runs before theirs load.
///
/// The `nth` of them: each plugin draws its own. With repositories
/// picked, the body is drawn again of them, as `ctx.run.picked` (ADR
/// 0050, gap L).
fn setup_step(
    overview: &Overview,
    nth: usize,
    picked: &[SetupRepo],
) -> Option<SetupView> {
    let (plugin, setup, app) = overview
        .plugins
        .iter()
        .filter(|entry| {
            entry.built_in && matches!(entry.standing, Standing::Active)
        })
        .filter_map(|entry| {
            let app = entry.app.as_ref()?;
            let setup = app.setup.as_ref()?;
            (!setup.is_null()).then_some((
                entry.name.as_str(),
                setup,
                &app.value,
            ))
        })
        .nth(nth)?;
    let text = |key: &str| setup[key].as_str().unwrap_or_default().to_owned();
    let body = if picked.is_empty() {
        embed_as(&setup["body"], plugin, LUAU_APP)
    } else {
        let picked: Vec<Value> = picked
            .iter()
            .map(|repo| {
                json!({
                    "name": repo.name,
                    "id": repo.id.as_ref().map(|id| id.0.to_string()),
                    "folder": repo.folder.as_ref().map(|f| f.display().to_string()),
                    "remote": repo.remote,
                })
            })
            .collect();
        Node::Fetched {
            key: format!("luau-setup/{plugin}/{}/{app}", json!(picked)),
            fetch: json!(Act::Setup {
                plugin: plugin.to_owned(),
                picked,
            }),
        }
    };
    Some(SetupView {
        step: crate::interned(&format!("{plugin}/{}", text("step"))),
        at: match setup["at"].as_str() {
            Some("model") => SetupAt::Model,
            Some("before_repositories") => SetupAt::BeforeRepositories,
            _ => SetupAt::AfterRepositories,
        },
        title: text("title"),
        heading: text("heading"),
        detail: text("detail"),
        body,
        required: false,
        link: None,
    })
}

/// The sections of the Models screen the Luau plugins draw of what their
/// `app` keeps.
fn models_section(overview: &Overview) -> Option<Node> {
    let sections: Vec<Node> = overview
        .plugins
        .iter()
        .filter(|entry| matches!(entry.standing, Standing::Active))
        .filter_map(|entry| {
            let models = entry.app.as_ref()?.models.as_ref()?;
            (!models.is_null()).then(|| embed_as(models, &entry.name, LUAU_APP))
        })
        .collect();
    (!sections.is_empty()).then(|| Node::stack(sections))
}

/// `heiko.reveal` of `plugin`'s page of the app: shows `args.path` in the
/// file manager, when it is in a folder the plugin's `uses.files` names.
fn reveal(plugin: &str, args: &Value, view: &mut ViewCx<'_, LuauPluginsUi>) {
    let Some(path) = args["path"].as_str() else {
        return;
    };
    let path = std::path::Path::new(path);
    let inside = view
        .data
        .plugins
        .iter()
        .find(|entry| entry.name == plugin)
        .and_then(|entry| entry.app.as_ref())
        .is_some_and(|app| app.roots.iter().any(|root| path.starts_with(root)));
    if inside
        && !path
            .components()
            .any(|part| part == std::path::Component::ParentDir)
    {
        view.handle.reveal(path);
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
            let listed = entry.app.iter().flat_map(|app| &app.commands);
            declared.chain(listed).map(|command| ListedCommand {
                name: command.name.clone(),
                hint: if command.description.is_empty() {
                    entry.name.clone()
                } else {
                    command.description.clone()
                },
                args: command.args.clone(),
                icon: Icon::named(&command.icon).unwrap_or(Icon::Plug),
                popover: command.popover.is_some(),
            })
        })
        .collect()
}

/// What a prompt that is a plugin's command is about, for the run's
/// title: `/name args` reads as its `args`, its leading `--flag value`
/// pairs dropped, and one quoted string as what it says. Only its first
/// line counts, and a name is a command's: lower case letters, digits,
/// `-` and `_`, a letter first.
pub fn read_command(prompt: &str) -> Option<String> {
    let line = prompt.lines().next()?.trim();
    let rest = line.strip_prefix('/')?;
    let (name, mut rest) = rest.split_once(char::is_whitespace)?;
    let named = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
        });
    if !named {
        return None;
    }
    rest = rest.trim_start();
    while rest.starts_with("--") {
        let mut words = rest.splitn(3, char::is_whitespace);
        words.next();
        words.next();
        rest = words.next().unwrap_or_default().trim_start();
    }
    let said = unquote(rest).unwrap_or_else(|| {
        rest.split_whitespace().collect::<Vec<_>>().join(" ")
    });
    (!said.trim().is_empty()).then_some(said)
}

/// What a quoted string says, when `text` is exactly one: `"`, then
/// characters with `\"`, `\\`, `\n` and `\r` escaped, then `"`.
fn unquote(text: &str) -> Option<String> {
    let mut chars = text.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return chars.as_str().is_empty().then_some(out),
            '\\' => out.push(match chars.next()? {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                _ => return None,
            }),
            c => out.push(c),
        }
    }
}

/// The active plugin whose command `command` is, and the command.
fn command_of<'a>(
    overview: &'a Overview,
    command: &str,
) -> Option<(&'a str, &'a crate::CommandSpec)> {
    overview
        .plugins
        .iter()
        .filter(|entry| matches!(entry.standing, Standing::Active))
        .find_map(|entry| {
            let spec = entry
                .declaration
                .iter()
                .flat_map(|d| &d.commands)
                .chain(entry.app.iter().flat_map(|app| &app.commands))
                .find(|c| c.name == command)?;
            Some((entry.name.as_str(), spec))
        })
}

/// The popover of `command` while it is written: its pieces, as there
/// is a Jev key or not.
fn command_popover(
    command: &str,
    _args: &str,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) -> Option<Node> {
    let keyed = heiko_jev::keyed(view);
    let (plugin, spec) = command_of(view.data, command)?;
    let popover = spec.popover.as_ref()?;
    let pieces = if keyed { &popover.jev } else { &popover.no_jev };
    Some(embed(pieces, plugin))
}

/// The ids of the inputs among `pieces`, at any depth.
fn input_ids(pieces: &Value, ids: &mut Vec<String>) {
    match pieces {
        Value::Object(piece) => {
            if piece.get("piece").and_then(Value::as_str) == Some("input")
                && let Some(id) = piece.get("id").and_then(Value::as_str)
            {
                ids.push(id.to_owned());
            }
            piece.values().for_each(|value| input_ids(value, ids));
        }
        Value::Array(pieces) => {
            pieces.iter().for_each(|value| input_ids(value, ids));
        }
        _ => {}
    }
}

/// What the inputs of `command`'s popover say, by id: their drafts, as
/// the interface keeps them.
fn drafts(
    plugin: &str,
    spec: &crate::CommandSpec,
    view: &ViewCx<'_, LuauPluginsUi>,
) -> serde_json::Map<String, Value> {
    let mut ids = Vec::new();
    if let Some(popover) = &spec.popover {
        input_ids(&popover.jev, &mut ids);
        input_ids(&popover.no_jev, &mut ids);
    }
    ids.into_iter()
        .filter_map(|id| {
            let draft = view.ui_value(&draft_key(&format!("{plugin}/{id}")))?;
            Some((id, draft.as_str()?.into()))
        })
        .collect()
}

/// `/command args`: the plugin whose command it is carries it out on the
/// run it was typed in, as its action `/command`, with what its
/// popover's inputs say.
fn run_command(
    command: &str,
    args: &str,
    _repo: Option<&str>,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) {
    let Some((plugin, drafts)) = command_of(view.data, command)
        .map(|(plugin, spec)| (plugin.to_owned(), drafts(plugin, spec, view)))
    else {
        return;
    };
    // Outside a chat, on no run and no state: what it keeps is dropped,
    // and what it asks the interface, such as sending a message, starts
    // a chat.
    let run = view.run.map(|run| run.id.0.to_string()).unwrap_or_default();
    // A run no plugin has drawn in yet starts from no state.
    let state = view.state.cloned().unwrap_or_default();
    luau_action(
        &plugin,
        &format!("/{command}"),
        &json!({ "text": args, "drafts": drafts }),
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
                        None => heiko_ui_kit::theme::Tone::Quiet,
                    };
                    (badge.to_owned(), tone)
                });
            Some(item)
        })
}

/// What the active plugins list on Home of a repository, of what each
/// keeps of it: its `repo_data`'s `home`, `{ title, meta, rows }`, each
/// row `{ id, title, detail }` with what clicking it does: `write`, the
/// words it puts in the new run's composer, or `action` and `args`, one
/// of the plugin's actions carried out in the repository. A list with
/// no rows is left out.
fn home_lists<'a>(
    overview: &'a Overview,
    kept: Option<&'a crate::RepoValues>,
) -> impl Iterator<Item = HomeList> + 'a {
    overview
        .plugins
        .iter()
        .filter(|entry| matches!(entry.standing, Standing::Active))
        .filter_map(move |entry| {
            let home = &kept?.0.get(&entry.name)?["home"];
            let text =
                |value: &Value| value.as_str().unwrap_or_default().to_owned();
            let rows: Vec<HomeRow> = home["rows"]
                .as_array()?
                .iter()
                .filter_map(|row| {
                    let action = match (&row["write"], &row["action"]) {
                        (Value::String(write), _) => {
                            HomeAction::Write(write.clone())
                        }
                        (_, Value::String(action)) => HomeAction::Act {
                            action: format!(
                                "{LUAU_REPO}{}/{action}",
                                entry.name
                            ),
                            args: row["args"].clone(),
                        },
                        _ => return None,
                    };
                    Some(HomeRow {
                        id: text(&row["id"]),
                        title: text(&row["title"]),
                        detail: text(&row["detail"]),
                        action,
                    })
                })
                .collect();
            (!rows.is_empty()).then(|| HomeList {
                title: text(&home["title"]),
                meta: home["meta"].as_str().map(str::to_owned),
                rows,
            })
        })
}

/// The active plugins' entries in `repo`'s menu: a link to the page its
/// `repo_menu` names, then what its `app` keeps as `menu`, a tree of the
/// app's (ADR 0050, gap L).
fn repo_menu<'a>(
    overview: &'a Overview,
    repo: &'a str,
) -> impl Iterator<Item = Node> + 'a {
    overview
        .plugins
        .iter()
        .filter(|entry| matches!(entry.standing, Standing::Active))
        .flat_map(move |entry| {
            let link = entry
                .declaration
                .as_ref()
                .and_then(|declaration| declaration.repo_menu.as_ref())
                .map(|menu| {
                    Node::row([
                        Node::Icon {
                            name: if menu.icon.is_empty() {
                                "plug".into()
                            } else {
                                menu.icon.clone()
                            },
                            tone: Tone::Neutral,
                        },
                        Node::Link {
                            label: menu.label.clone(),
                            to: repo_page_link(&entry.name, &menu.page, repo),
                        },
                    ])
                });
            let kept = entry
                .app
                .as_ref()
                .map(|app| &app.value["menu"])
                .filter(|menu| !menu.is_null())
                .map(|menu| embed_app(menu, &entry.name));
            link.into_iter().chain(kept)
        })
}

fn nav_tone(nav: Option<&Value>) -> Option<heiko_ui_kit::theme::Tone> {
    Some(match nav?["tone"].as_str()? {
        "good" => heiko_ui_kit::theme::Tone::Good,
        "warn" => heiko_ui_kit::theme::Tone::Warn,
        "bad" => heiko_ui_kit::theme::Tone::Danger,
        "info" => heiko_ui_kit::theme::Tone::Info,
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
            // Its page of the app, when it names one; else its card's.
            let to = match &nav.page {
                Some(page) => app_page_link(&entry.name, page),
                None => entry_link(&entry.name),
            };
            let mut item = NavEntry::new(nav.label.clone(), icon, to);
            // What its `app` keeps says the entry's detail and badge.
            let said = entry.app.as_ref().map(|app| &app.value["nav"]);
            item.detail = said
                .and_then(|nav| nav["detail"].as_str())
                .map(str::to_owned);
            item.badge =
                said.and_then(|nav| nav["badge"].as_str()).map(|badge| {
                    let tone = nav_tone(said)
                        .unwrap_or(heiko_ui_kit::theme::Tone::Quiet);
                    (badge.to_owned(), tone)
                });
            Some(item)
        })
}

/// The page of the person's own plugins (ADR 0036): each plugin of the
/// plugins repository as a card, with where it stands, and the chats
/// writing them. Opened for one plugin, the page is that plugin's. A
/// view tree in the frame the interface gives a page (ADR 0045).
fn page(view: &mut ViewCx<'_, LuauPluginsUi>) -> Node {
    if let (Some(entry), Some(page)) = (
        view.entry().map(str::to_owned),
        view.params.get(PAGE_PARAM).cloned(),
    ) {
        return match view.params.get(REPO_PARAM).cloned() {
            Some(repo) => repo_page(&entry, &page, &repo, view),
            None => app_page(&entry, &page, view),
        };
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
    // Whether a Jev key is saved, as the page's `ctx.run.jev`.
    let jev = heiko_jev::keyed(view);
    let drafts = page_drafts(plugin, view);
    Node::Fetched {
        key: format!(
            "luau-repo-page/{plugin}/{}/{kept}/{jev}/{}",
            json!(params),
            json!(drafts)
        ),
        fetch: json!(Act::Page {
            plugin: plugin.to_owned(),
            page: page.to_owned(),
            params,
            jev,
            drafts,
        }),
    }
}

/// `plugin`'s page `page` of the app (ADR 0050, gap L), drawn by its host
/// half: fetched again whenever what its `app` keeps changes.
fn app_page(
    plugin: &str,
    page: &str,
    view: &mut ViewCx<'_, LuauPluginsUi>,
) -> Node {
    let kept = view
        .data
        .plugins
        .iter()
        .find(|entry| entry.name == plugin)
        .and_then(|entry| entry.app.as_ref())
        .map(|app| app.value.to_string())
        .unwrap_or_default();
    let params = view.params.clone();
    let jev = heiko_jev::keyed(view);
    let drafts = page_drafts(plugin, view);
    Node::Fetched {
        key: format!(
            "luau-app-page/{plugin}/{}/{kept}/{jev}/{}",
            json!(params),
            json!(drafts)
        ),
        fetch: json!(Act::Page {
            plugin: plugin.to_owned(),
            page: page.to_owned(),
            params,
            jev,
            drafts,
        }),
    }
}

/// What the inputs `plugin`'s `drafts` names hold, by id: its pages are
/// drawn again as they change.
fn page_drafts(
    plugin: &str,
    view: &ViewCx<'_, LuauPluginsUi>,
) -> BTreeMap<String, String> {
    let Some(declaration) = view
        .data
        .plugins
        .iter()
        .find(|entry| entry.name == plugin)
        .and_then(|entry| entry.declaration.as_ref())
    else {
        return BTreeMap::new();
    };
    declaration
        .drafts
        .iter()
        .filter_map(|id| {
            let said = view
                .ui_value(&draft_key(&format!("{plugin}/{id}")))?
                .as_str()?
                .to_owned();
            (!said.is_empty()).then(|| (id.clone(), said))
        })
        .collect()
}

/// The link to `plugin`'s page `page` of the app (ADR 0050, gap L).
pub fn app_page_link(plugin: &str, page: &str) -> Link {
    entry_link(plugin).param(PAGE_PARAM, page)
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
                let grown = args["grown"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|word| word.as_str().map(str::to_owned))
                    .collect();
                handle.act(Act::Allow {
                    plugin: plugin.to_owned(),
                    grown,
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

/// A state's mark.
fn mark(state: MarkState) -> Node {
    Node::Mark { state, count: None }
}

/// Small print in mono.
fn small(text: impl Into<String>) -> Node {
    toned(text, Tone::Neutral)
}

/// A line in mono, in `tone`.
fn toned(text: impl Into<String>, tone: Tone) -> Node {
    Node::Toned {
        text: text.into(),
        tone,
        mono: true,
    }
}

/// `rows` with a hairline between each two.
fn ruled(rows: impl IntoIterator<Item = Node>) -> Vec<Node> {
    let mut ruled = Vec::new();
    for (at, row) in rows.into_iter().enumerate() {
        if at > 0 {
            ruled.push(Node::Rule);
        }
        ruled.push(row);
    }
    ruled
}

/// Every plugin of the plugins repository as a ruled row, how many stand
/// each way, the way to write one, and the chats there.
fn overview(view: &ViewCx<'_, LuauPluginsUi>) -> Node {
    let overview = view.data;
    let write = Node::Button {
        label: "Write a plugin".into(),
        action: WRITE.into(),
        args: Value::Null,
        kind: Kind::Primary,
        icon: None,
        size: Default::default(),
        wide: false,
        detail: None,
    };
    let counts = if overview.plugins.is_empty() {
        Vec::new()
    } else {
        counts(overview)
    };
    // On a computer the counts are on the head; a phone's is too narrow
    // for them, so they go under it.
    let head = |actions| Node::Header {
        title: "Your plugins".into(),
        meta: Some(crate::REPO.into()),
        actions,
    };
    let mut plugins = Vec::new();
    if view.compact {
        plugins.push(head(vec![write]));
        if !counts.is_empty() {
            plugins.push(Node::Row { children: counts });
        }
    } else {
        let mut actions = counts;
        actions.push(write);
        plugins.push(head(actions));
    }
    if let Some(error) = &overview.error {
        plugins.push(Node::Status {
            text: error.clone(),
            detail: None,
            tone: Tone::Bad,
        });
    }
    if overview.plugins.is_empty() {
        plugins.push(Node::Empty {
            text: "No plugins yet.".into(),
        });
    } else if view.compact {
        plugins.push(Node::Rule);
        plugins.extend(ruled(overview.plugins.iter().map(|entry| {
            let [name, does, needs] = plugin_row(entry);
            Node::stack([name, does, needs])
        })));
    } else {
        // Three columns: the plugin and where it stands, what it does and
        // when it runs, and what it waits on.
        plugins.push(Node::Table {
            head: vec![],
            rows: overview
                .plugins
                .iter()
                .map(|entry| plugin_row(entry).to_vec())
                .collect(),
            widths: vec![1., 1.5, 1.5],
        });
    }
    plugins.push(gap());
    plugins.extend(chats(view));
    Node::stack(plugins)
}

/// An empty line: the room between two sections.
fn gap() -> Node {
    Node::text("\u{a0}")
}

/// How many plugins there are, and how many stand each way, each count
/// in its tone.
fn counts(overview: &Overview) -> Vec<Node> {
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
    let mut counts = vec![small(if all == 1 {
        "1 plugin".to_owned()
    } else {
        format!("{all} plugins")
    })];
    counts.extend(
        tones
            .into_iter()
            .filter(|(n, ..)| *n > 0)
            .map(|(n, label, tone)| toned(format!("{n} {label}"), tone)),
    );
    counts
}

/// Where `standing` leaves a plugin: its mark, and in words and a tone.
fn standing_label(standing: &Standing) -> (MarkState, &'static str, Tone) {
    match standing {
        Standing::Active => (MarkState::Met, "active", Tone::Good),
        Standing::Waiting { .. } => {
            (MarkState::Asks, "waiting for you", Tone::Warn)
        }
        Standing::Failing => (MarkState::Failing, "tests failing", Tone::Bad),
        Standing::Broken { .. } => {
            (MarkState::Failing, "does not load", Tone::Bad)
        }
    }
}

/// Where `entry` stands: its state's mark, and the word for it.
fn standing_row(entry: &Entry) -> Node {
    let (state, label, tone) = standing_label(&entry.standing);
    Node::row([mark(state), toned(label, tone)])
}

/// The most tests drawn as a square each; more are only counted.
const TEST_SQUARES: usize = 12;

/// `entry`'s tests, when it has any: a small square each, green for one
/// that passes and red for one that fails, then how many pass.
fn tests_row(entry: &Entry) -> Option<Node> {
    if entry.tests.is_empty() {
        return None;
    }
    let passed = entry.tests.iter().filter(|test| test.passed).count();
    let mut row: Vec<Node> = if entry.tests.len() <= TEST_SQUARES {
        entry
            .tests
            .iter()
            .map(|test| Node::Square {
                label: if test.passed { "✓" } else { "×" }.into(),
                style: if test.passed {
                    SquareLook::Filled
                } else {
                    SquareLook::Outlined
                },
                tone: if test.passed { Ink::Good } else { Ink::Bad },
                action: None,
                args: Default::default(),
            })
            .collect()
    } else {
        Vec::new()
    };
    row.push(small(format!("{passed} of {}", entry.tests.len())));
    Some(Node::Row { children: row })
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
fn plan_field(state: &State) -> Option<heiko_ui_plugin::PlanField> {
    state
        .plugins
        .iter()
        .filter(|(_, plugin)| plugin.off.is_none())
        .find_map(|(name, plugin)| {
            let field = plugin.view.get("plan")?;
            Some(heiko_ui_plugin::PlanField {
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

/// What the first plugin by name whose view has `key` drew there: for a
/// place only one plugin may fill.
fn first_drawn(state: &State, key: &str) -> Option<Node> {
    state
        .plugins
        .iter()
        .filter(|(_, plugin)| plugin.off.is_none())
        .find_map(|(name, plugin)| {
            let piece =
                plugin.view.get(key).filter(|piece| !piece.is_null())?;
            Some(embed(piece, name))
        })
}

/// What the plugins' views draw on another tool's card, the call
/// `call_id`: `place` (`badge`, beside its title, or `body`, under it) of
/// each view's `calls[call_id]`, together.
/// The run's pane, from the first plugin by name whose view has one:
/// `pane = { title, fresh, body }`, `fresh` growing with each new thing
/// it shows.
fn pane(state: &State) -> Option<heiko_ui_plugin::points::PaneView> {
    state
        .plugins
        .iter()
        .filter(|(_, plugin)| plugin.off.is_none())
        .find_map(|(name, plugin)| {
            let pane =
                plugin.view.get("pane").filter(|pane| !pane.is_null())?;
            Some(heiko_ui_plugin::points::PaneView {
                title: pane["title"].as_str().unwrap_or(name).to_owned(),
                fresh: pane["fresh"].as_u64().unwrap_or_default(),
                body: embed(&pane["body"], name),
            })
        })
}

/// What the first plugin by name whose view draws on the card of
/// `call_id` puts in place of its head and at its header's end:
/// `calls = { [call_id] = { head, label } }`.
fn card_header(state: &State, call_id: &str) -> Option<CardView> {
    state
        .plugins
        .iter()
        .filter(|(_, plugin)| plugin.off.is_none())
        .find_map(|(name, plugin)| {
            let call = plugin.view.get("calls")?.get(call_id)?;
            let head = call.get("head").filter(|head| !head.is_null());
            let label = call.get("label").and_then(Value::as_str);
            if head.is_none() && label.is_none() {
                return None;
            }
            Some(CardView {
                head: head.map(|head| embed(head, name)),
                label: label.map(str::to_owned),
                ..CardView::default()
            })
        })
}

fn on_card(state: &State, call_id: &str, place: &str) -> Option<Node> {
    let mut trees: Vec<Node> = state
        .plugins
        .iter()
        .filter(|(_, plugin)| plugin.off.is_none())
        .filter_map(|(name, plugin)| {
            let piece = plugin.view.get("calls")?.get(call_id)?.get(place)?;
            (!piece.is_null()).then(|| embed(piece, name))
        })
        .collect();
    match trees.len() {
        0 => None,
        1 => trees.pop(),
        _ => Some(Node::stack(trees)),
    }
}

/// How a person's message `text` draws, from the first plugin whose
/// view has it among its `messages`: the piece it gave, which an empty
/// stack makes nothing at all.
fn user_message(state: &State, text: &str) -> Option<Node> {
    state
        .plugins
        .iter()
        .filter(|(_, plugin)| plugin.off.is_none())
        .find_map(|(name, plugin)| {
            let piece = plugin.view.get("messages")?.get(text)?;
            (!piece.is_null()).then(|| embed(piece, name))
        })
}

/// The run's row in the sidebar, from the first plugin whose view says
/// `row = { line, count?, tone?, icon?, phone_line? }`.
fn row(state: &State) -> Option<RowNote> {
    state
        .plugins
        .values()
        .filter(|plugin| plugin.off.is_none())
        .find_map(|plugin| {
            let row = plugin.view.get("row")?;
            let line = row["line"].as_str()?.to_owned();
            Some(RowNote {
                phone_line: Some(
                    row["phone_line"].as_str().unwrap_or(&line).to_owned(),
                ),
                line: Some(line),
                count: row["count"].as_str().map(str::to_owned),
                icon: row["icon"]
                    .as_str()
                    .and_then(Icon::named)
                    .unwrap_or(Icon::Plug),
                tone: match row["tone"].as_str() {
                    Some("good") => heiko_ui_kit::theme::Tone::Good,
                    Some("warn") => heiko_ui_kit::theme::Tone::Warn,
                    Some("bad") => heiko_ui_kit::theme::Tone::Danger,
                    _ => heiko_ui_kit::theme::Tone::Quiet,
                },
            })
        })
}

/// The link to `entry`'s own page.
/// The page of `entry`, one of the Luau plugins: its card, or the page it
/// draws of the app.
pub fn entry_link(entry: &str) -> Link {
    Link::page(PAGE).param(ENTRY, entry)
}

/// The Allow that lets a waiting version of `plugin` reach `grown`, as
/// the page shows it.
fn allow(plugin: &str, grown: &[String]) -> Node {
    Node::Button {
        label: "Allow".into(),
        action: ALLOW.into(),
        args: json!({ "plugin": plugin, "grown": grown }),
        kind: Kind::Primary,
        icon: None,
        size: Default::default(),
        wide: false,
        detail: None,
    }
}

/// Stamps, one per thing said, side by side.
fn stamps(words: Vec<String>) -> Node {
    Node::row(words.into_iter().map(|label| Node::Stamp {
        label,
        tone: Ink::Neutral,
    }))
}

/// One plugin on the page of all of them, as a row's three cells: its
/// name, where it stands and its tests; what it does and when it runs;
/// and what it waits on, with Allow for one that waits on the person,
/// and the way to its own page.
fn plugin_row(entry: &Entry) -> [Node; 3] {
    let mut name = vec![
        Node::Title {
            text: entry.name.clone(),
            subtitle: None,
        },
        standing_row(entry),
    ];
    name.extend(tests_row(entry));

    let mut does = Vec::new();
    if !entry.description.is_empty() {
        does.push(Node::text(entry.description.clone()));
    }
    if let Some(declaration) = &entry.declaration {
        let seams = steps_in(declaration);
        if !seams.is_empty() {
            does.push(stamps(seams));
        }
    }
    if !entry.version.is_empty() {
        does.push(small(format!("version {}", entry.version)));
    }

    let mut needs = vec![Node::row([
        Node::Spacer,
        Node::Link {
            label: "open".into(),
            to: entry_link(&entry.name),
        },
    ])];
    match &entry.standing {
        Standing::Waiting { grown } => {
            needs.push(Node::Rich {
                text: format!("Wants {}.", grown.join(", ")),
            });
            needs.push(allow(&entry.name, grown));
        }
        Standing::Broken { error } => needs.push(Node::Code {
            lang: None,
            text: error.clone(),
            first_line: None,
        }),
        _ => {}
    }
    needs.extend(
        entry
            .tests
            .iter()
            .filter(|test| !test.passed)
            .flat_map(test_lines),
    );
    if entry.keeps_earlier {
        needs.push(Node::Toned {
            text: "runs keep the version before".into(),
            tone: Tone::Neutral,
            mono: false,
        });
    }
    [
        Node::Stack { children: name },
        Node::Stack { children: does },
        Node::Stack { children: needs },
    ]
}

/// A test's line: its file and name, and why it failed, if it did.
fn test_lines(test: &TestResult) -> Vec<Node> {
    let mut lines = vec![Node::Mono {
        text: format!("{} · {}", test.file, test.name),
    }];
    if let Some(error) = &test.error {
        lines.push(toned(error.clone(), Tone::Bad));
    }
    lines
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
    // A page of the app it draws itself (ADR 0050, gap K), in place of
    // its card.
    if let Some(page) = found.app.as_ref().and_then(|app| app.page.as_ref()) {
        return embed_as(page, &found.name, LUAU_APP);
    }
    let mut page = vec![
        back,
        Node::Title {
            text: found.name.clone(),
            subtitle: (!found.description.is_empty())
                .then(|| found.description.clone()),
        },
        standing_row(&found),
    ];
    page.extend(tests_row(&found));
    match &found.standing {
        Standing::Waiting { grown } => {
            let mut words = vec![Node::Rich {
                text: format!("Its new version wants {}.", grown.join(", ")),
            }];
            if found.keeps_earlier {
                words.push(Node::Toned {
                    text: "runs keep the version before".into(),
                    tone: Tone::Neutral,
                    mono: false,
                });
            }
            words.push(allow(&found.name, grown));
            page.push(Node::Card { children: words });
        }
        Standing::Broken { error } => {
            page.push(Node::Code {
                lang: None,
                text: error.clone(),
                first_line: None,
            });
        }
        Standing::Failing if found.keeps_earlier => {
            page.push(Node::Toned {
                text: "runs keep the version before".into(),
                tone: Tone::Neutral,
                mono: false,
            });
        }
        _ => {}
    }

    if let Some(declaration) = &found.declaration {
        let seams = steps_in(declaration);
        page.push(section(
            "Where it steps in",
            vec![if seams.is_empty() {
                small("only in the interface")
            } else {
                stamps(seams)
            }],
        ));
        let reach = reaches(&declaration.uses);
        page.push(section(
            "What it may reach",
            vec![if reach.is_empty() {
                small("nothing beyond the run it is in")
            } else {
                Node::row(reach.into_iter().map(|text| Node::Mono { text }))
            }],
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
        page.push(section(
            &title,
            vec![embed_as(&drawn, entry, LUAU_PAGE)],
        ));
    }
    page.push(section(
        "Tests",
        if found.tests.is_empty() {
            vec![Node::Empty {
                text: "It has none yet.".into(),
            }]
        } else {
            ruled(found.tests.iter().map(|test| {
                Node::row([
                    mark(if test.passed {
                        MarkState::Met
                    } else {
                        MarkState::Failing
                    }),
                    Node::stack(test_lines(test)),
                ])
            }))
        },
    ));
    let has_settings = found
        .declaration
        .as_ref()
        .is_some_and(|declaration| declaration.settings.is_some());
    if has_settings {
        page.push(section(
            "Settings",
            vec![scopes(entry, view), Node::Rule, crate::pane::render(view)],
        ));
    }
    Node::stack(page)
}

/// A section of a plugin's page: its head, which does not fold, over
/// `children`.
fn section(title: &str, children: Vec<Node>) -> Node {
    let mut section = vec![Node::Header {
        title: title.to_owned(),
        meta: None,
        actions: vec![],
    }];
    section.extend(children);
    Node::stack(section)
}

/// "Applies to", then everywhere and each of the person's repositories,
/// the one shown outlined: the scope `entry`'s settings show (ADR 0029).
fn scopes(entry: &str, view: &ViewCx<'_, LuauPluginsUi>) -> Node {
    let scope = view.scope().map(str::to_owned);
    let options = std::iter::once(None).chain(
        view.repos()
            .map(|(name, _)| name)
            .filter(|name| *name != crate::REPO)
            .map(|name| Some(name.to_owned())),
    );
    let mut row = vec![Node::text("Applies to"), Node::Spacer];
    row.extend(options.map(|option| {
        let label = option.clone().unwrap_or_else(|| "everywhere".into());
        if option == scope {
            return Node::Stamp {
                label,
                tone: Ink::Bright,
            };
        }
        let to = match &option {
            Some(repo) => entry_link(entry).param(SCOPE, repo.clone()),
            None => entry_link(entry),
        };
        Node::Link { label, to }
    }));
    Node::Row { children: row }
}

/// The chats writing plugins under their head, newest first, a ruled
/// row each, opening its run.
fn chats(view: &ViewCx<'_, LuauPluginsUi>) -> Vec<Node> {
    let runs: Vec<RunInfo> = view
        .runs()
        .into_iter()
        .map(|(run, _)| run)
        .filter(|run| run.repo == crate::REPO && !run.title.is_empty())
        .collect();
    let mut list = vec![Node::Header {
        title: "Chats".into(),
        meta: (!runs.is_empty()).then(|| runs.len().to_string()),
        actions: vec![],
    }];
    if runs.is_empty() {
        list.push(Node::Empty {
            text: "None yet.".into(),
        });
        return list;
    }
    list.extend(ruled(runs.into_iter().map(|run| {
        Node::row([
            mark(if run.live {
                MarkState::Working
            } else {
                MarkState::Idle
            }),
            Node::Rich {
                text: format!("**{}**", run.title),
            },
            Node::Spacer,
            Node::Button {
                label: "open".into(),
                action: OPEN_RUN.into(),
                args: json!({ "run": run.id.0.to_string() }),
                kind: Kind::Secondary,
                icon: None,
                size: Default::default(),
                wide: false,
                detail: None,
            },
        ])
    })));
    list.push(Node::Rule);
    list
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
        let mut run_cx = heiko_ui_plugin::testing::FakeRun::default();
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
    /// (`heiko.interface`) is done after its records are kept, on its run.
    #[test]
    fn an_action_may_ask_the_interface_when_done() {
        use std::{cell::RefCell, rc::Rc};

        use heiko_ui_plugin::Request;
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
            changed: true,
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

    /// The composer's place is one plugin's: the first by name whose
    /// view draws there and that is not off.
    #[test]
    fn only_one_plugin_fills_the_composers_place() {
        let part = |view: Value, off: bool| PluginState {
            view,
            off: off.then(|| "failed".to_owned()),
            ..PluginState::default()
        };
        let form = |text: &str| json!({ "composer": { "piece": "text", "text": text } });
        let mut state = State::default();
        assert_eq!(first_drawn(&state, "composer"), None);
        state.plugins.insert("a".into(), part(json!({}), false));
        state.plugins.insert("b".into(), part(form("off"), true));
        state.plugins.insert("c".into(), part(form("first"), false));
        state
            .plugins
            .insert("d".into(), part(form("second"), false));
        assert_eq!(first_drawn(&state, "composer"), Some(Node::text("first")));
    }

    /// A tree's `heiko.` actions ask the interface, on the run the tree is
    /// drawn in; the plugin's own are not, and an unknown one says so.
    #[test]
    fn a_trees_heiko_actions_ask_the_interface() {
        use std::{cell::RefCell, rc::Rc};

        use heiko_ui_plugin::Request;
        let asked: Rc<RefCell<Vec<Request>>> = Rc::default();
        let sink = asked.clone();
        let handle = Handle::new(
            NAME,
            Rc::new(move |_, request| sink.borrow_mut().push(request)),
        );
        let run = RunId("r1".into());
        assert!(kernel_action(
            "heiko.steer",
            &json!({ "text": "stop" }),
            Some(&run),
            &handle
        ));
        assert!(kernel_action(
            "heiko.send",
            &json!({ "text": "go on" }),
            Some(&run),
            &handle
        ));
        assert!(kernel_action(
            "heiko.composer",
            &json!({ "text": "draft" }),
            None,
            &handle
        ));
        // Steering needs a run going on.
        assert!(kernel_action(
            "heiko.steer",
            &json!({ "text": "lost" }),
            None,
            &handle
        ));
        assert!(kernel_action("heiko.cancel", &json!({}), Some(&run), &handle));
        assert!(!kernel_action("reset", &json!({}), Some(&run), &handle));
        assert!(kernel_action("heiko.nope", &json!({}), None, &handle));
        let asked = asked.borrow();
        assert_eq!(asked[3], Request::Cancel(run.clone()));
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
        assert_eq!(asked.len(), 5, "{asked:?}");
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
        assert_eq!(drawn_row.tone, heiko_ui_kit::theme::Tone::Warn);

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
            version: String::new(),
            standing,
            declaration: Some(Declaration {
                commands: vec![crate::CommandSpec {
                    name: format!("{name}-go"),
                    description: String::new(),
                    args: "<what>".into(),
                    ..Default::default()
                }],
                ..Declaration::named(name)
            }),
            tests: Vec::new(),
            keeps_earlier: false,
            built_in: false,
            app: None,
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

    /// What a plugin's `app` keeps (ADR 0050, gap K): its listed commands
    /// join the menu and are found by name to run, and its `nav` gives
    /// the sidebar entry's detail and badge.
    #[test]
    fn what_app_keeps_lists_commands_and_marks_the_sidebar() {
        let app = crate::AppView {
            value: json!({ "nav": { "detail": "2 skills", "badge": "1", "tone": "bad" } }),
            commands: vec![crate::CommandSpec {
                name: "release-notes".into(),
                description: "Writes release notes".into(),
                args: "what to do".into(),
                icon: "skill".into(),
                ..Default::default()
            }],
            roots: vec!["/home/me/.agents/skills".into()],
            ..crate::AppView::default()
        };
        let overview = Overview {
            plugins: vec![Entry {
                name: "heiko-skills".into(),
                description: String::new(),
                version: String::new(),
                standing: Standing::Active,
                declaration: Some(Declaration {
                    nav: Some(crate::Nav {
                        label: "Skills".into(),
                        icon: "skill".into(),
                        page: None,
                    }),
                    listed_commands: true,
                    ..Declaration::named("heiko-skills")
                }),
                tests: Vec::new(),
                keeps_earlier: false,
                built_in: true,
                app: Some(app),
            }],
            ..Overview::default()
        };
        let listed = commands(&overview);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "release-notes");
        assert_eq!(listed[0].hint, "Writes release notes");
        assert_eq!(
            command_of(&overview, "release-notes").map(|(plugin, _)| plugin),
            Some("heiko-skills")
        );
        let nav: Vec<NavEntry> = nav_entries(&overview).collect();
        assert_eq!(nav.len(), 1);
        assert_eq!(nav[0].detail.as_deref(), Some("2 skills"));
        assert_eq!(
            nav[0].badge,
            Some(("1".to_owned(), heiko_ui_kit::theme::Tone::Danger))
        );
    }

    /// Each active plugin that asks for a sidebar entry has one, opening
    /// its page; a waiting or broken one has none.
    #[test]
    fn active_plugins_with_nav_are_in_the_sidebar() {
        let entry =
            |name: &str, standing: Standing, nav: Option<crate::Nav>| Entry {
                name: name.into(),
                description: String::new(),
                version: String::new(),
                standing,
                declaration: Some(Declaration {
                    nav,
                    ..Declaration::named(name)
                }),
                tests: Vec::new(),
                keeps_earlier: false,
                built_in: false,
                app: None,
            };
        let nav = |label: &str| {
            Some(crate::Nav {
                label: label.into(),
                icon: "not-an-icon".into(),
                page: None,
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

    /// An active plugin's `home`, of what it keeps of a repository, is
    /// its list on Home: each row writes in the composer or carries out
    /// the plugin's action in the repository. A list with no rows, and a
    /// plugin that is not active, give none.
    #[test]
    fn what_a_plugin_keeps_as_home_is_its_list_there() {
        let entry = |name: &str, standing: Standing| Entry {
            name: name.into(),
            description: String::new(),
            version: String::new(),
            standing,
            declaration: Some(Declaration::named(name)),
            tests: Vec::new(),
            keeps_earlier: false,
            built_in: false,
            app: None,
        };
        let overview = Overview {
            plugins: vec![
                entry("todo", Standing::Active),
                entry("empty", Standing::Active),
                entry("new", Standing::Failing),
            ],
            ..Overview::default()
        };
        let home = json!({
            "title": "Ready to pick up",
            "meta": "this repository's issues",
            "rows": [
                { "id": "td-1", "title": "Log once", "detail": "P3",
                  "write": "Work on td-1: Log once." },
                { "id": "td-2", "title": "Jitter", "detail": "P2",
                  "action": "start", "args": { "issue": "td-2" } },
                { "id": "td-3", "title": "Does nothing", "detail": "P2" },
            ],
        });
        let kept = crate::RepoValues(
            [
                ("todo".to_owned(), json!({ "home": home })),
                ("empty".to_owned(), json!({ "home": { "rows": [] } })),
                ("new".to_owned(), json!({ "home": home })),
            ]
            .into(),
        );
        let lists: Vec<HomeList> = home_lists(&overview, Some(&kept)).collect();
        assert_eq!(
            lists,
            [HomeList {
                title: "Ready to pick up".into(),
                meta: Some("this repository's issues".into()),
                rows: vec![
                    HomeRow {
                        id: "td-1".into(),
                        title: "Log once".into(),
                        detail: "P3".into(),
                        action: HomeAction::Write(
                            "Work on td-1: Log once.".into()
                        ),
                    },
                    HomeRow {
                        id: "td-2".into(),
                        title: "Jitter".into(),
                        detail: "P2".into(),
                        action: HomeAction::Act {
                            action: "luau-repo/todo/start".into(),
                            args: json!({ "issue": "td-2" }),
                        },
                    },
                ],
            }]
        );
        assert_eq!(home_lists(&overview, None).count(), 0);
    }

    /// A view's `plan` is a field of the run's plan, set by the plugin
    /// whose view it is; a view without one sets none.
    #[test]
    fn a_views_plan_is_the_plans_field() {
        let mut state = State::default();
        let mut run = heiko_ui_plugin::testing::FakeRun::default();
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
            Some(heiko_ui_plugin::PlanField {
                name: "reasoning".into(),
                value: "high".into(),
                set_by: Some("picks".into()),
            })
        );
    }

    /// A person's message draws as the piece a view gives its exact
    /// text, the first plugin's that has one; an off plugin's draws
    /// nothing, and another message stays plain.
    #[test]
    fn a_views_messages_draw_the_persons_messages() {
        let mut state = State::default();
        let mut run = heiko_ui_plugin::testing::FakeRun::default();
        for (plugin, piece) in [("goal", "the goal"), ("echo", "an echo")] {
            state.apply(
                Record::View {
                    plugin: plugin.into(),
                    view: json!({ "messages": {
                        "/goal pass": { "piece": "text", "text": piece },
                    } }),
                },
                &mut run,
            );
        }
        let drawn = user_message(&state, "/goal pass").unwrap();
        assert!(format!("{drawn:?}").contains("an echo"), "{drawn:?}");
        assert_eq!(user_message(&state, "hello"), None);
        state.apply(
            Record::Off {
                plugin: "echo".into(),
                why: "it failed".into(),
            },
            &mut run,
        );
        let drawn = user_message(&state, "/goal pass").unwrap();
        assert!(format!("{drawn:?}").contains("the goal"), "{drawn:?}");
    }

    /// A command's prompt reads as what follows it, its flags dropped
    /// and a quoted string unquoted; anything else is not one.
    #[test]
    fn a_command_prompt_reads_as_what_it_is_about() {
        for (prompt, about) in [
            ("/goal the tests pass", Some("the tests pass")),
            (
                "/goal --continuations 3 --budget 1.5 the   tests pass",
                Some("the tests pass"),
            ),
            ("/goal \"clear\"", Some("clear")),
            ("/goal \"two\\nlines\"", Some("two\nlines")),
            (
                "/goal the tests pass\n\nThis is your goal.",
                Some("the tests pass"),
            ),
            ("/goal", None),
            ("/goal --budget 2", None),
            ("/etc/hosts is broken", None),
            ("/Goal the tests pass", None),
            ("fix the tests", None),
        ] {
            assert_eq!(read_command(prompt).as_deref(), about, "{prompt}");
        }
    }

    /// A view's `calls` draw on that call's card, beside its title and
    /// under it, every plugin's together; another call's card has none.
    #[test]
    fn a_views_calls_draw_on_their_cards() {
        let mut state = State::default();
        let mut run = heiko_ui_plugin::testing::FakeRun::default();
        for (plugin, text) in [("rules", "R1 0.82"), ("audit", "seen")] {
            state.apply(
                Record::View {
                    plugin: plugin.into(),
                    view: json!({ "calls": { "c1": {
                        "badge": { "piece": "mono", "text": text },
                        "body": { "piece": "text", "text": format!("{plugin} says") },
                    } } }),
                },
                &mut run,
            );
        }
        let badge = format!("{:?}", on_card(&state, "c1", "badge").unwrap());
        assert!(
            badge.contains("R1 0.82") && badge.contains("seen"),
            "{badge}"
        );
        let body = format!("{:?}", on_card(&state, "c1", "body").unwrap());
        assert!(body.contains("rules says"), "{body}");
        assert_eq!(on_card(&state, "c2", "badge"), None);
    }

    /// A view's `calls` that say `mark` mark their cards, refused with
    /// the reason or flagged, as a Rust plugin's verdict does; a card not
    /// shown yet is marked once it is, and a mark is not made twice.
    #[test]
    fn a_views_calls_mark_their_cards() {
        let card = |id: &str| heiko_ui_plugin::CardInfo {
            call_id: id.into(),
            tool: "write".into(),
            args: json!({}),
            summary: String::new(),
            size: 0,
            turn: 1,
        };
        let mut state = State::default();
        let mut run =
            heiko_ui_plugin::testing::FakeRun::with_cards(vec![card("c1")]);
        let view = || Record::View {
            plugin: "rules".into(),
            view: json!({ "calls": {
                "c1": { "mark": "blocked", "reason": "rule R1" },
                "c2": { "mark": "flagged" },
                "c3": { "badge": { "piece": "mono", "text": "R1 0.10" } },
            } }),
        };
        state.apply(view(), &mut run);
        assert_eq!(
            run.marks["c1"],
            CardMark::Blocked {
                reason: "rule R1".into(),
                by: Some("rules".into()),
            }
        );
        assert!(!run.marks.contains_key("c3"));
        let marked = &state.plugins["rules"].marked;
        assert!(marked.contains_key("c1") && !marked.contains_key("c2"));
        // c2's card shows; the next view marks it, and leaves c1 be.
        run.cards.push(card("c2"));
        run.marks.clear();
        state.apply(view(), &mut run);
        assert_eq!(run.marks.keys().collect::<Vec<_>>(), ["c2"]);
        assert_eq!(run.marks["c2"], CardMark::Flagged);
    }

    /// A link in a run's view to one of the plugin's own pages that
    /// names a repository opens that page of the repository; one that
    /// names none, or another plugin's page, stays as it is.
    #[test]
    fn a_views_link_to_its_page_of_a_repository_opens_it() {
        let link = |params: Value, plugin: Value| {
            json!({ "piece": "link", "label": "Review", "to": {
                "plugin": plugin, "page": "rules", "params": params,
            } })
        };
        let to = |piece: Value| match embed(&piece, "rules-plugin") {
            Node::Link { to, .. } => to,
            other => panic!("not a link: {other:?}"),
        };
        let opened = to(link(
            json!({ "repo": "docbert", "rule": "R2" }),
            Value::Null,
        ));
        assert_eq!(
            opened,
            repo_page_link("rules-plugin", "rules", "docbert")
                .param("rule", "R2")
        );
        // One that names no repository opens its page of the app (ADR
        // 0050, gap L).
        assert_eq!(
            to(link(json!({ "note": "a" }), Value::Null)),
            app_page_link("rules-plugin", "rules").param("note", "a")
        );
        assert_eq!(
            to(link(json!({ "repo": "docbert" }), json!("heiko-jev"))).plugin,
            Some("heiko-jev".into())
        );
    }

    /// A plugin's pages of the app (ADR 0050, gap L): its sidebar entry
    /// opens the one `nav.page` names; its entry in a repository's menu
    /// opens its page of that repository, and what `app` keeps as `menu`
    /// follows it, as a tree of the app's; its setup step is drawn again
    /// of the repositories picked; a map's nodes open its pages.
    #[test]
    fn a_plugins_pages_of_the_app_are_opened_from_its_places() {
        let app = crate::AppView {
            value: json!({
                "nav": { "detail": "3 notes" },
                "menu": { "piece": "button", "label": "Import", "action": "import", "args": {}, "kind": "secondary" },
            }),
            setup: Some(json!({
                "step": "memories", "at": "after_repositories", "title": "Memories",
                "heading": "h", "detail": "d", "body": { "piece": "text", "text": "none picked" },
            })),
            ..crate::AppView::default()
        };
        let overview = Overview {
            plugins: vec![Entry {
                name: "heiko-memory".into(),
                description: String::new(),
                version: String::new(),
                standing: Standing::Active,
                declaration: Some(Declaration {
                    nav: Some(crate::Nav {
                        label: "Your notes".into(),
                        icon: "memory".into(),
                        page: Some("notes".into()),
                    }),
                    repo_menu: Some(crate::RepoNav {
                        label: "Memory".into(),
                        page: "notes".into(),
                        icon: "memory".into(),
                    }),
                    pages: vec!["notes".into()],
                    ..Declaration::named("heiko-memory")
                }),
                tests: Vec::new(),
                keeps_earlier: false,
                built_in: true,
                app: Some(app),
            }],
            ..Overview::default()
        };
        let nav: Vec<NavEntry> = nav_entries(&overview).collect();
        assert_eq!(nav[0].to, app_page_link("heiko-memory", "notes"));
        let menu: Vec<Node> = repo_menu(&overview, "shop").collect();
        assert_eq!(menu.len(), 2);
        let opens = |node: &Node| -> Vec<Link> {
            let mut found = Vec::new();
            rewrite(node.clone(), &mut |node| {
                if let Node::Link { to, .. } = &node {
                    found.push(to.clone());
                }
                node
            });
            found
        };
        assert_eq!(
            opens(&menu[0]),
            [repo_page_link("heiko-memory", "notes", "shop")]
        );
        match &menu[1] {
            Node::Button { action, .. } => {
                assert_eq!(action, &format!("{LUAU_APP}heiko-memory/import"))
            }
            other => panic!("not the app's button: {other:?}"),
        }

        let step = setup_step(&overview, 0, &[]).unwrap();
        assert_eq!(step.body, Node::text("none picked"));
        assert!(setup_step(&overview, 1, &[]).is_none());
        let picked = [SetupRepo {
            name: "acme/shop".into(),
            id: None,
            remote: Some("https://github.com/acme/shop".into()),
            folder: None,
            clone: heiko_ui_plugin::points::SetupClone::Picked,
        }];
        match setup_step(&overview, 0, &picked).unwrap().body {
            Node::Fetched { fetch, .. } => {
                assert_eq!(fetch["act"], "setup");
                assert_eq!(fetch["picked"][0]["name"], "acme/shop");
            }
            other => panic!("not drawn again: {other:?}"),
        }

        let graph = json!({ "piece": "graph", "height": 10.0, "edges": [], "nodes": [
            { "id": "a", "x": 0.5, "y": 0.5, "to": { "plugin": null, "page": "notes", "params": { "note": "b" } } },
        ] });
        match embed_app(&graph, "heiko-memory") {
            Node::Graph { nodes, .. } => assert_eq!(
                nodes[0].to,
                Some(app_page_link("heiko-memory", "notes").param("note", "b"))
            ),
            other => panic!("not a map: {other:?}"),
        }
    }

    /// `heiko.open_run` from a plugin's tree opens the run it names.
    #[test]
    fn open_run_opens_the_run_it_names() {
        let asked: std::rc::Rc<
            std::cell::RefCell<Vec<heiko_ui_plugin::Request>>,
        > = std::rc::Rc::default();
        let sink = asked.clone();
        let handle = Handle::new(
            NAME,
            std::rc::Rc::new(move |_, request| sink.borrow_mut().push(request)),
        );
        assert!(kernel_action(
            "heiko.open_run",
            &json!({ "run": "r7" }),
            None,
            &handle
        ));
        assert_eq!(
            asked.borrow().as_slice(),
            [heiko_ui_plugin::Request::OpenRun(RunId("r7".into()))]
        );
    }

    /// A popover's drafts are read from each of its inputs, however
    /// deep it sits.
    #[test]
    fn a_popovers_inputs_are_found_at_any_depth() {
        let pieces = json!({ "piece": "stack", "children": [
            { "piece": "input", "id": "a" },
            { "piece": "row", "children": [
                { "piece": "text", "text": "and" },
                { "piece": "input", "id": "b", "submit": "heiko.submit" },
            ] },
        ] });
        let mut ids = Vec::new();
        input_ids(&pieces, &mut ids);
        assert_eq!(ids, ["a", "b"]);
    }

    /// A keyed note is placed where the run is when its key first
    /// comes, and drawn again in place as it changes: a later key is
    /// placed later, and the earlier stays where it was.
    #[test]
    fn keyed_notes_stay_where_they_were_placed() {
        let mut state = State::default();
        let mut run = heiko_ui_plugin::testing::FakeRun::default();
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
        let mut run = heiko_ui_plugin::testing::FakeRun::default();
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
            changed: true,
            view: Some(json!({ "status": { "text": "2" } })),
            logs: vec!["added".into()],
            error: Some(("view".into(), "late".into())),
            interface: None,
        };
        let mut state = State::default();
        for record in done.records() {
            state
                .apply(record, &mut heiko_ui_plugin::testing::FakeRun::default());
        }
        let part = &state.plugins["p"];
        assert_eq!(part.state, json!({ "n": 2 }));
        assert_eq!(part.view, json!({ "status": { "text": "2" } }));
        assert_eq!(part.logs, ["added"]);
        assert_eq!(part.errors, [("view".to_owned(), "late".to_owned())]);
        // One whose model a run's mailbox kept, or that left it as it
        // was, records neither it nor its view.
        let unchanged = ActionDone {
            changed: false,
            ..done
        };
        assert!(unchanged.records().iter().all(|record| matches!(
            record,
            Record::Log { .. } | Record::Error { .. }
        )));
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
                .apply(record, &mut heiko_ui_plugin::testing::FakeRun::default());
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
            &mut heiko_ui_plugin::testing::FakeRun::default(),
        );
        assert_eq!(state.status().as_deref(), Some("p: off · 3 hooks failed"));
    }
}
