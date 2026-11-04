//! The Agents page and the New agent form, each a view tree (ADR 0045)
//! in the frame the interface gives a page.

use gpui::AnyElement;
use serde_json::{Value, json};
use tau_ui_kit::format::usd;
use tau_ui_plugin::{
    ViewCx,
    tree::{Kind, Node, Tone},
};

use super::{
    AgentsUi,
    CAP,
    Form,
    GO,
    MCP,
    MODEL,
    NAME_INPUT,
    NEW,
    OPEN,
    OVERVIEW,
    PURPOSE_INPUT,
    SERVERS,
    SET,
    START,
    WINDOW,
    act_button,
    button,
    on_action,
    open,
    status_pill,
    tree_tone,
};
use crate::{Act, Agent, COMMAND, Overview, Status};

/// What a click on "Allow more" lets an agent spend, on top of its cap.
const MORE: f64 = 1.0;

/// Every agent: those that need the person first, then a card each.
pub(super) fn overview(view: &mut ViewCx<'_, AgentsUi>) -> AnyElement {
    let overview = view.data.clone();
    // A card opens its agent's conversation: its main chat, or the first
    // run the window knows in its repository.
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
    let tree = overview_tree(&overview, &runs, &draft);
    let actions = on_action(view.handle.clone(), None, None);
    view.tree("agents-overview", &tree, actions)
}

/// The Agents page's tree: `runs` are what each agent's card opens, in
/// the order of `overview`'s agents; question fields' drafts are read
/// with `draft`.
pub fn overview_tree(
    overview: &Overview,
    runs: &[Option<String>],
    draft: &dyn Fn(&str) -> Option<String>,
) -> Node {
    let mut page = vec![
        Node::row([
            Node::Title {
                text: "Agents".into(),
                subtitle: None,
            },
            Node::Spacer,
            button("New agent", Kind::Primary, GO, json!({ "page": NEW })),
        ]),
        Node::Rich {
            text: format!(
                "Each agent is one conversation that never ends, in a \
                 repository of its own. What it listens to arrives as \
                 messages; it works through codemode and rewrites its own \
                 Luau, tools and instructions as it learns. Agents run while \
                 tau is open and catch up when it starts. `/{COMMAND}` and \
                 what it should do makes one from any chat."
            ),
        },
    ];
    let waiting: Vec<&Agent> = overview
        .agents
        .iter()
        .filter(|agent| agent.needs_you())
        .collect();
    if !waiting.is_empty() {
        page.extend(needs_you(&waiting, draft));
    }
    if overview.agents.is_empty() {
        page.push(Node::Empty {
            text: "No agents yet.".into(),
        });
    } else {
        page.extend(overview.agents.iter().enumerate().map(|(at, agent)| {
            agent_card(agent, runs.get(at).cloned().flatten())
        }));
    }
    Node::Stack { children: page }
}

/// The agents that wait on the person, each with what to do about it.
fn needs_you(
    agents: &[&Agent],
    draft: &dyn Fn(&str) -> Option<String>,
) -> Vec<Node> {
    let mut card = Vec::new();
    for agent in agents {
        let (line, action) = match &agent.status {
            Status::AtCap => (
                format!(
                    "Reached its {} for today; {} waiting.",
                    usd(agent.cap_today),
                    events(agent.queued),
                ),
                Some(act_button(
                    &format!("Allow {} more today", usd(MORE)),
                    Kind::Primary,
                    &Act::AllowMore {
                        agent: agent.name.clone(),
                        amount: MORE,
                    },
                )),
            ),
            Status::Broken { error } => {
                (format!("Nothing wakes it: {error}"), None)
            }
            _ => continue,
        };
        let mut row = vec![
            Node::Mono {
                text: agent.name.clone(),
            },
            Node::text(line),
            Node::Spacer,
        ];
        row.extend(action);
        card.push(Node::Row { children: row });
    }
    // What each agent left for the person: its notes and questions.
    for agent in agents.iter().filter(|agent| !agent.notices.is_empty()) {
        if !card.is_empty() {
            card.push(Node::Rule);
        }
        card.push(Node::Mono {
            text: agent.name.clone(),
        });
        card.extend(super::notices::rows(agent, draft));
    }
    let count: usize = agents.iter().map(|agent| agent.waits_on_you()).sum();
    vec![
        Node::row([
            Node::Heading {
                text: "Needs you".into(),
            },
            Node::Pill {
                text: count.to_string(),
                tone: Tone::Info,
            },
        ]),
        Node::Card { children: card },
    ]
}

/// `n events`, said as a count.
fn events(count: usize) -> String {
    match count {
        0 => "no events".to_owned(),
        1 => "1 event".to_owned(),
        n => format!("{n} events"),
    }
}

/// One agent: what it is for, where it stands, what wakes it, and what
/// it spent of today's cap; `run`, the conversation it opens.
fn agent_card(agent: &Agent, run: Option<String>) -> Node {
    let caption = match (&agent.status, agent.next()) {
        (Status::Working, _) => {
            Node::text(format!("in a turn · {}", events(agent.queued)))
        }
        (Status::Waiting, Some(next)) => Node::row([
            Node::text("waiting · next"),
            Node::Due {
                id: format!("agent-{}-next", agent.name),
                at: next * 1000,
            },
        ]),
        (Status::Waiting, None) => Node::text("waiting"),
        (Status::Paused, _) => {
            Node::text(format!("paused · {} waiting", events(agent.queued)))
        }
        (Status::AtCap, _) => {
            Node::text(format!("at its cap · {} waiting", events(agent.queued)))
        }
        (Status::Broken { .. }, _) => Node::text("broken"),
    };
    let last = match &agent.last {
        Some(last) => Node::row([
            Node::text(format!("{} ·", last.line)),
            super::run::ago(format!("agent-{}-last", agent.name), last.at),
        ]),
        None => Node::text("nothing yet"),
    };
    let mut card = vec![
        Node::row([
            Node::Icon {
                name: "sub-agent".into(),
                tone: tree_tone(&agent.status),
            },
            Node::Mono {
                text: agent.name.clone(),
            },
            status_pill(&agent.status),
            Node::Spacer,
            caption,
        ]),
        Node::text(agent.purpose.clone()),
    ];
    if let Some(problem) = &agent.sandbox {
        card.push(Node::Status {
            text: problem.clone(),
            detail: None,
            tone: Tone::Warn,
        });
    }
    if !agent.triggers.is_empty() {
        // A poll that fails shows red; its panel says why.
        card.push(Node::Row {
            children: agent
                .triggers
                .iter()
                .flat_map(|trigger| {
                    let failing = trigger.error.is_some();
                    [
                        Node::Icon {
                            name: trigger_icon(trigger.kind_label()).into(),
                            tone: if failing {
                                Tone::Bad
                            } else {
                                Tone::Neutral
                            },
                        },
                        Node::Pill {
                            text: trigger.label(),
                            tone: if failing {
                                Tone::Bad
                            } else {
                                Tone::Neutral
                            },
                        },
                    ]
                })
                .collect(),
        });
    }
    card.push(Node::Rule);
    card.push(Node::row([
        last,
        Node::Spacer,
        Node::Mono {
            text: format!(
                "{} of {}",
                usd(agent.spend_today),
                usd(agent.cap_today)
            ),
        },
    ]));
    card.push(Node::Progress {
        share: f64::from(agent.spent_share()),
        label: None,
    });
    if let Some(run) = run {
        card.push(Node::row([
            Node::Spacer,
            button("Open", Kind::Secondary, OPEN, json!({ "run": run })),
        ]));
    }
    Node::Card { children: card }
}

/// The icon of a trigger's kind, by its file's name.
fn trigger_icon(kind: &str) -> &'static str {
    match kind {
        "schedule" => "clock",
        "command" => "terminal",
        "poll" => "history",
        "stream" => "arrow",
        "mcp" => "plug",
        _ => "chat",
    }
}

/// The New agent form: a name, what it should do, its model, cap and
/// window. Once the host made it, its conversation opens.
pub(super) fn new_agent(view: &mut ViewCx<'_, AgentsUi>) -> AnyElement {
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
                        view.handle.open_run(&run, view.cx);
                    }
                }
            }
            super::reset(&view.handle, view.cx);
        }
    }
    let servers = super::mcp_servers(view.data_of(MCP));
    let taken = {
        let name = form.chosen_name();
        view.data.agent(&name).is_some()
            && form.making.as_deref() != Some(name.as_str())
    };
    let tree = new_agent_tree(&form, &servers, taken);
    let actions = on_action(view.handle.clone(), Some(form), None);
    view.tree("agents-new", &tree, actions)
}

/// The form's tree: `servers` are the person's MCP servers, and `taken`
/// whether an agent has the name it would take.
pub fn new_agent_tree(form: &Form, servers: &[String], taken: bool) -> Node {
    let set = |label: &str, on: bool, key: &str, value: Value| {
        button(
            label,
            if on { Kind::Primary } else { Kind::Secondary },
            SET,
            json!({ "key": key, "value": value }),
        )
    };
    let label = |text: &str| Node::Heading { text: text.into() };
    let suggested = form.suggested();
    let name = form.chosen_name();
    let mut page = vec![
        Node::Title {
            text: "New agent".into(),
            subtitle: None,
        },
        Node::Rich {
            text: format!(
                "It gets a repository and a conversation of its own. What it \
                 should do is its first message; from there it sets up what \
                 wakes it, its tools and its instructions itself. \
                 `/{COMMAND}` and what it should do in any chat opens this \
                 form."
            ),
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
            "Watch new pull requests on tau-agent, label them, and tell me \
             when one touches auth."
                .into(),
        ),
        multiline: true,
        submit: None,
        keep: true,
    });
    let default_model = crate::default_model();
    page.push(label("Model"));
    page.push(Node::text(
        "A small model keeps a conversation that never ends cheap.",
    ));
    page.push(Node::Row {
        children: tau_ai::model::plan_models()
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
    page.push(label("Most it may spend a day"));
    page.push(Node::Row {
        children: [0.5, 1.0, 2.0, 5.0]
            .into_iter()
            .map(|cap: f64| {
                set(
                    &format!("${cap:.2}"),
                    (form.daily_cap - cap).abs() < f64::EPSILON,
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
        page.push(Node::text(
            "Yours, as tau-mcp lists them. It can connect more itself.",
        ));
        page.push(Node::Row {
            children: servers
                .iter()
                .map(|server| {
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
                    set(server.as_str(), on, SERVERS, next)
                })
                .collect(),
        });
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
    page.push(Node::row([
        Node::Icon {
            name: "info".into(),
            tone: Tone::Neutral,
        },
        Node::text("Only you can change what it may spend; it cannot."),
    ]));
    Node::Stack { children: page }
}
