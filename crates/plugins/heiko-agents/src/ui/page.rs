//! The Agents page and the New agent form, each a view tree (ADR 0045)
//! in the frame the interface gives a page.

use heiko_ui_plugin::{
    ViewCx,
    tree::{Ink, Kind, MarkState, Node, SquareLook, Tone},
};
use serde_json::{Value, json};

use super::{
    AgentsUi,
    CAP,
    Form,
    GO,
    KEYS,
    MCP,
    MODEL,
    NAME_INPUT,
    NEW,
    OPEN,
    OVERVIEW,
    PURPOSE_INPUT,
    SECTION,
    SERVERS,
    SET,
    START,
    WAKES,
    WINDOW,
    act_button,
    allowance,
    button,
    events,
    gap,
    open,
    state_mark,
};
use crate::{Act, Agent, Overview, Status};

/// How many agents stand side by side.
const COLUMNS: usize = 3;

/// Every agent: those that need the person first, then a column each.
pub(super) fn overview(view: &mut ViewCx<'_, AgentsUi>) -> Node {
    if let Some(section) = section(view) {
        return section;
    }
    let overview = view.data.clone();
    // A column opens its agent's conversation: its main chat, or the
    // first run the window knows in its repository.
    let runs: Vec<Option<String>> = overview
        .agents
        .iter()
        .map(|agent| {
            agent.main.clone().or_else(|| {
                super::conversation_of(&agent.name, view)
                    .map(|run| run.0.to_string())
            })
        })
        .collect();
    let draft = |key: &str| {
        view.ui_value(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    overview_tree(&overview, &runs, view.compact, &draft)
}

/// The agent and the part of it the page is opened for, when its
/// parameters name both: an agent's sidebar rows send them.
fn asked<'a>(view: &ViewCx<'a, AgentsUi>) -> Option<(&'a Agent, &'a str)> {
    let agent = view.data.agent(view.params.get("repo")?)?;
    let section = view.params.get(SECTION)?.as_str();
    matches!(section, WAKES | KEYS).then_some((agent, section))
}

/// The page's title: the agent's name when it shows one part of an
/// agent, else "Agents".
pub(super) fn title(view: &mut ViewCx<'_, AgentsUi>) -> String {
    match asked(view) {
        Some((agent, _)) => agent.name.clone(),
        None => "Agents".into(),
    }
}

/// The part of an agent the page is opened for, as the whole page.
fn section(view: &mut ViewCx<'_, AgentsUi>) -> Option<Node> {
    let (agent, section) = asked(view)?;
    section_tree(agent, section)
}

/// One part of `agent` as a page: its name over what wakes it
/// ([`WAKES`]) or its keys ([`KEYS`]); nothing for another `section`.
pub fn section_tree(agent: &Agent, section: &str) -> Option<Node> {
    let (title, count, rows) = match section {
        WAKES => (
            "What wakes it",
            agent.triggers.len() + agent.scheduled.len(),
            super::run::wakes(agent, super::run::PAGE_ROOM),
        ),
        KEYS => ("Keys", agent.secrets.len(), super::run::secrets(agent)),
        _ => return None,
    };
    let mut page = vec![
        Node::Title {
            text: agent.name.clone(),
            subtitle: None,
            tone: Default::default(),
        },
        Node::Header {
            title: title.into(),
            meta: (count > 0).then(|| count.to_string()),
            actions: vec![],
        },
    ];
    page.extend(rows);
    Some(Node::Stack { children: page })
}

/// The Agents page's tree: `runs` are what each agent's column opens, in
/// the order of `overview`'s agents; question fields' drafts are read
/// with `draft`. On a phone (`compact`) the agents are one under the
/// other.
pub fn overview_tree(
    overview: &Overview,
    runs: &[Option<String>],
    compact: bool,
    draft: &dyn Fn(&str) -> Option<String>,
) -> Node {
    // The page's one block, on its first section's head.
    let mut new = Some(button(
        "New agent",
        Kind::Primary,
        GO,
        json!({ "page": NEW }),
    ));
    let mut page = Vec::new();
    let waiting: Vec<&Agent> = overview
        .agents
        .iter()
        .filter(|agent| agent.needs_you())
        .collect();
    if !waiting.is_empty() {
        page.extend(needs_you(&waiting, new.take(), compact, draft));
        page.push(gap());
    }
    page.push(Node::Header {
        title: "Agents".into(),
        meta: (!overview.agents.is_empty())
            .then(|| overview.agents.len().to_string()),
        actions: new.into_iter().collect(),
    });
    if overview.agents.is_empty() {
        page.push(Node::Empty {
            text: "No agents yet.".into(),
        });
        return Node::Stack { children: page };
    }
    let columns: Vec<[Node; 2]> = overview
        .agents
        .iter()
        .enumerate()
        .map(|(at, agent)| agent_column(agent, runs.get(at).cloned().flatten()))
        .collect();
    if compact {
        page.extend(ruled(
            columns
                .into_iter()
                .map(|[top, foot]| Node::stack([top, foot]))
                .collect(),
        ));
    } else {
        // Rows of columns, each a row of what the agents are over a row
        // of what they did, so the allowances stand in one line; the
        // last row keeps its columns' widths.
        page.extend(columns.chunks(COLUMNS).map(|row| {
            let half = |at: usize| {
                let mut cells: Vec<Node> =
                    row.iter().map(|column| column[at].clone()).collect();
                cells.resize(COLUMNS, Node::Stack { children: vec![] });
                cells
            };
            Node::Table {
                head: vec![],
                rows: vec![half(0), half(1)],
                widths: vec![],
            }
        }));
    }
    Node::Stack { children: page }
}

/// `rows` with a hairline between each two.
pub(super) fn ruled(rows: Vec<Node>) -> Vec<Node> {
    let mut ruled = Vec::with_capacity(rows.len() * 2);
    for (at, row) in rows.into_iter().enumerate() {
        if at > 0 {
            ruled.push(Node::Rule);
        }
        ruled.push(row);
    }
    ruled
}

/// The shares of the width a "Needs you" row gives the agent, when, what
/// it says, and what the person can do.
const NEEDS_YOU: [f32; 4] = [1.6, 1., 6., 2.4];

/// The agents that wait on the person under their section's head, which
/// holds `new`: a ruled row for each thing, each answerable in place,
/// with one hot action and the rest links. On a phone (`compact`) a row
/// is its parts one under the other.
fn needs_you(
    agents: &[&Agent],
    new: Option<Node>,
    compact: bool,
    draft: &dyn Fn(&str) -> Option<String>,
) -> Vec<Node> {
    let name = |agent: &Agent| Node::Rich {
        text: format!("**{}**", agent.name),
    };
    // An agent's own state, when that is what waits: its mark, what to
    // say of it, and what the person can do.
    let own = |agent: &Agent| match &agent.status {
        Status::AtCap => Some((
            MarkState::Limit,
            format!(
                "Reached today's allowance; {} waiting.",
                events(agent.queued),
            ),
            Some(act_button(
                super::ALLOW_MORE,
                Kind::Hot,
                &Act::AllowMore {
                    agent: agent.name.clone(),
                },
            )),
        )),
        Status::Broken { error } => Some((
            MarkState::Failing,
            format!("Nothing wakes it: {error}"),
            None,
        )),
        _ => None,
    };
    let count: usize = agents.iter().map(|agent| agent.waits_on_you()).sum();
    let head = Node::Header {
        title: "Needs you".into(),
        meta: Some(count.to_string()),
        actions: new.into_iter().collect(),
    };
    if compact {
        let mut rows = Vec::new();
        for agent in agents {
            if let Some((state, text, action)) = own(agent) {
                let mut row = vec![
                    Node::Mark { state, count: None },
                    name(agent),
                    Node::text(text),
                    Node::Spacer,
                ];
                row.extend(action);
                rows.push(Node::Row { children: row });
            }
            // What it left for the person: its notes and questions.
            rows.extend(super::notices::rows(agent, true, draft));
        }
        let mut section = vec![head];
        section.extend(ruled(rows));
        return section;
    }
    let mut rows = Vec::new();
    for agent in agents {
        if let Some((state, text, action)) = own(agent) {
            let mut end = vec![Node::Spacer];
            end.extend(action);
            rows.push(vec![
                Node::row([Node::Mark { state, count: None }, name(agent)]),
                Node::Stack { children: vec![] },
                Node::text(text),
                Node::Row { children: end },
            ]);
        }
        rows.extend(super::notices::lines(agent, draft));
    }
    vec![
        head,
        Node::Table {
            head: vec![],
            rows,
            widths: NEEDS_YOU.to_vec(),
        },
    ]
}

/// One agent, in two parts: its name, where it stands, what it is for
/// and what wakes it; then what it did last and how much of the day's
/// allowance it used. `run` is the conversation it opens.
fn agent_column(agent: &Agent, run: Option<String>) -> [Node; 2] {
    let caption = match (&agent.status, agent.next()) {
        (Status::Working, _) => {
            vec![Node::text(format!("in a turn · {}", events(agent.queued)))]
        }
        (Status::Waiting, Some(next)) => vec![
            Node::text("waiting · next"),
            Node::Due {
                id: format!("agent-{}-next", agent.name),
                at: next * 1000,
            },
        ],
        (Status::Waiting, None) => vec![Node::text("waiting")],
        (Status::Paused, _) => vec![Node::text(format!(
            "paused · {} waiting",
            events(agent.queued)
        ))],
        (Status::AtCap, _) => vec![Node::text(format!(
            "at its cap · {} waiting",
            events(agent.queued)
        ))],
        (Status::Broken { .. }, _) => vec![Node::text("broken")],
    };
    let mut head = vec![
        Node::Title {
            text: agent.name.clone(),
            subtitle: None,
            tone: Default::default(),
        },
        Node::Spacer,
    ];
    if let Some(run) = run {
        head.push(button("open", Kind::Secondary, OPEN, json!({ "run": run })));
    }
    let mut state = vec![state_mark(&agent.status)];
    state.extend(caption);
    let mut column = vec![
        Node::Row { children: head },
        Node::Row { children: state },
        Node::text(agent.purpose.clone()),
    ];
    if let Some(problem) = &agent.sandbox {
        column.push(Node::Status {
            text: problem.clone(),
            detail: None,
            tone: Tone::Warn,
        });
    }
    // What wakes it, a ruled row each: its kind, what, and when next. A
    // poll that fails shows red; its panel says why.
    for trigger in &agent.triggers {
        column.push(Node::Rule);
        let mut row = vec![
            Node::Toned {
                text: trigger.kind_label().into(),
                tone: if trigger.error.is_some() {
                    Tone::Bad
                } else {
                    Tone::Info
                },
                mono: true,
            },
            Node::Mono {
                text: trigger.label(),
            },
            Node::Spacer,
        ];
        if let Some(next) = trigger.next {
            row.push(Node::Due {
                id: format!("agent-{}-{}-next", agent.name, trigger.name),
                at: next * 1000,
            });
        }
        column.push(Node::Row { children: row });
    }
    let mut foot = vec![match &agent.last {
        Some(last) => Node::row([
            Node::Toned {
                text: format!("{} ·", last.line),
                tone: Tone::Neutral,
                mono: false,
            },
            Node::Ago {
                id: format!("agent-{}-last", agent.name),
                at: last.at * 1000,
            },
        ]),
        None => Node::Toned {
            text: "nothing yet".into(),
            tone: Tone::Neutral,
            mono: false,
        },
    }];
    foot.extend(allowance(agent));
    [
        Node::Stack { children: column },
        Node::Stack { children: foot },
    ]
}

/// The New agent form: a name, what it should do, its model, cap and
/// window. Once the host made it, its conversation opens.
pub(super) fn new_agent(view: &mut ViewCx<'_, AgentsUi>) -> Node {
    let form = Form::of(view);
    // The host made it: open its conversation as soon as the window
    // knows it.
    if let Some(name) = &form.making {
        let agent = view.data.agent(name).cloned();
        let ready = agent.as_ref().is_some_and(|agent| agent.main.is_some())
            || super::conversation_of(name, view).is_some();
        if ready {
            match agent {
                Some(agent) => open(&agent, view),
                None => {
                    if let Some(run) = super::conversation_of(name, view) {
                        view.handle.open_run(&run);
                    }
                }
            }
            super::reset(&view.handle);
        }
    }
    let servers = super::mcp_servers(view.data_of(MCP));
    let taken = {
        let name = form.chosen_name();
        view.data.agent(&name).is_some()
            && form.making.as_deref() != Some(name.as_str())
    };
    new_agent_tree(&form, &servers, taken)
}

/// The form's tree: `servers` are the person's MCP servers, and `taken`
/// whether an agent has the name it would take.
pub fn new_agent_tree(form: &Form, servers: &[String], taken: bool) -> Node {
    // A choice: the one chosen outlined, the others a click away.
    let set = |label: &str, on: bool, key: &str, value: Value| {
        if on {
            return Node::Stamp {
                label: label.to_owned(),
                tone: Ink::Bright,
            };
        }
        button(
            label,
            Kind::Secondary,
            SET,
            json!({ "key": key, "value": value }),
        )
    };
    let label = |text: &str| Node::Heading { text: text.into(), tone: Default::default() };
    let suggested = form.suggested();
    let name = form.chosen_name();
    let mut page = vec![
        Node::Title {
            text: "New agent".into(),
            subtitle: None,
            tone: Default::default(),
        },
        label("Name"),
        Node::Input {
            id: NAME_INPUT.into(),
            placeholder: Some(if suggested.is_empty() {
                "pr-triage".into()
            } else {
                suggested.clone()
            }),
            multiline: false,
            submit: None,
            keep: true,
            secret: false,
            focus: None,
        },
    ];
    if form.name.trim().is_empty() && !suggested.is_empty() {
        page.push(Node::Rich {
            text: format!("Left empty, it is named `{suggested}`."),
        });
    }
    if !name.is_empty() && !crate::valid_name(&name) {
        page.push(Node::Status {
            text: "Lower-case letters, digits and dashes, starting with a \
                   letter."
                .into(),
            detail: None,
            tone: Tone::Bad,
        });
    }
    if taken {
        page.push(Node::Status {
            text: "An agent has this name already.".into(),
            detail: None,
            tone: Tone::Bad,
        });
    }
    page.push(label("What it should do"));
    page.push(Node::Input {
        id: PURPOSE_INPUT.into(),
        placeholder: Some(
            "Watch new pull requests on heiko-agent, label them, and tell me \
             when one touches auth."
                .into(),
        ),
        multiline: true,
        submit: None,
        keep: true,
        secret: false,
        focus: None,
    });
    let default_model = crate::default_model();
    page.push(label("Model"));
    page.push(Node::Row {
        children: heiko_ai::model::plan_models()
            .into_iter()
            .map(|plan| {
                let id = plan.id.to_owned();
                let on = form.model.as_deref().unwrap_or(&default_model) == id;
                let pick = if id == default_model {
                    Value::Null
                } else {
                    json!(id)
                };
                set(&id, on, MODEL, pick)
            })
            .collect(),
    });
    page.push(label("Most it may use a day"));
    page.push(Node::Row {
        children: [250_000, 500_000, 1_000_000, 2_000_000, 5_000_000]
            .into_iter()
            .map(|cap: u64| {
                set(
                    &super::cap_label(cap),
                    form.daily_cap == cap,
                    CAP,
                    json!(cap),
                )
            })
            .collect(),
    });
    page.push(label("Gather events for"));
    page.push(Node::Row {
        children: [(10, "10 s"), (30, "30 s"), (60, "1 min"), (300, "5 min")]
            .into_iter()
            .map(|(seconds, text): (u64, &str)| {
                set(text, form.window_secs == seconds, WINDOW, json!(seconds))
            })
            .collect(),
    });
    if !servers.is_empty() {
        page.push(label("MCP servers it starts with"));
        page.extend(servers.iter().map(|server| {
            let on = form.servers.contains(server);
            let mut next = form.servers.clone();
            if on {
                next.remove(server);
            } else {
                next.insert(server.clone());
            }
            let next = if next.is_empty() {
                Value::Null
            } else {
                json!(next)
            };
            // On or off: the one that holds outlined, the other a
            // click away.
            let segment = |label: &str, chosen: bool| Node::Square {
                label: label.into(),
                style: if chosen {
                    SquareLook::Outlined
                } else {
                    SquareLook::Quiet
                },
                tone: if chosen { Ink::Info } else { Ink::Neutral },
                action: (!chosen).then(|| SET.to_owned()),
                args: json!({ "key": SERVERS, "value": next }),
            };
            Node::row([
                Node::Mono {
                    text: server.clone(),
                },
                Node::Spacer,
                segment("on", on),
                segment("off", !on),
            ])
        }));
    }
    if let Some(error) = &form.error {
        page.push(Node::Status {
            text: error.clone(),
            detail: None,
            tone: Tone::Bad,
        });
    }
    page.push(Node::row([
        Node::Spacer,
        button("Cancel", Kind::Secondary, GO, json!({ "page": OVERVIEW })),
        button(
            if form.making.is_some() {
                "Starting it…"
            } else {
                "Start it"
            },
            Kind::Primary,
            START,
            Value::Null,
        ),
    ]));
    Node::Stack { children: page }
}
