//! How the interface words an agent: times relative to now, its
//! triggers, a name from its purpose, and its commands.

use hegel::generators as gs;
use tau_agents::{
    Agent,
    Overview,
    Status,
    Trigger,
    TriggerKind,
    shown::{relative, suggest_name},
    ui::commands,
    valid_name,
};

fn trigger(kind: TriggerKind) -> Trigger {
    Trigger {
        name: "t".into(),
        kind,
        last: None,
        next: None,
        error: None,
        live: None,
    }
}

fn agent(name: &str, triggers: Vec<Trigger>) -> Agent {
    Agent {
        name: name.into(),
        purpose: String::new(),
        status: Status::Waiting,
        triggers,
        versions: Vec::new(),
        spend_today: 0.5,
        cap_today: 1.0,
        queued: 0,
        last: None,
        model: "gpt-6-luna".into(),
        effort: "low".into(),
        main: None,
        secrets: Vec::new(),
        sandbox: None,
        notices: Vec::new(),
        scheduled: Vec::new(),
    }
}

/// The unit a time is said in, independently of how `relative` picks it:
/// the largest of days, hours and minutes that fits the gap, counted
/// whole; under a minute either way is now.
fn said(gap: i64) -> String {
    let far = gap.unsigned_abs();
    if far < 60 {
        return "now".to_owned();
    }
    let (count, unit) = [(86_400, "d"), (3_600, "h"), (60, "m")]
        .into_iter()
        .find(|(seconds, _)| far >= *seconds)
        .map(|(seconds, unit)| (far / seconds, unit))
        .unwrap();
    if gap > 0 {
        format!("in {count}{unit}")
    } else {
        format!("{count}{unit} ago")
    }
}

/// A time is said from now, in the largest whole unit its distance fits,
/// as ahead or behind (the agents page's times). Gaps reach every unit's
/// edges on both sides of now.
#[hegel::test(test_cases = 500)]
fn a_time_is_said_from_now_in_its_largest_whole_unit(tc: hegel::TestCase) {
    let now: i64 =
        tc.draw(gs::integers::<i64>().min_value(0).max_value(1 << 40));
    let edges: Vec<i64> = [0, 59, 60, 3_599, 3_600, 86_399, 86_400]
        .into_iter()
        .flat_map(|gap| [gap, -gap])
        .collect();
    let gap: i64 = tc.draw(hegel::one_of!(
        gs::sampled_from(edges),
        gs::integers::<i64>()
            .min_value(-(1 << 30))
            .max_value(1 << 30),
    ));
    assert_eq!(relative(now + gap, now), said(gap), "gap {gap}");
}

#[test]
fn triggers_say_what_wakes_it() {
    let schedule = trigger(TriggerKind::Schedule {
        every: None,
        at: Some("09:00".into()),
        days: vec!["Mon".into(), "Fri".into()],
    });
    assert_eq!(schedule.label(), "09:00 Mon Fri");
    assert_eq!(schedule.kind_label(), "schedule");
    let every = trigger(TriggerKind::Schedule {
        every: Some("15m".into()),
        at: None,
        days: Vec::new(),
    });
    assert_eq!(every.label(), "every 15m");
    let command = trigger(TriggerKind::Command {
        command: "triage".into(),
        hint: Some("<number>".into()),
    });
    assert_eq!(command.label(), "/triage <number>");
}

/// Words a purpose is made of: ones that name something, in any case,
/// the small words a name leaves out, ones that start with a digit, and
/// long ones.
const WORDS: [&str; 16] = [
    "Watch",
    "RustSec",
    "advisories",
    "feed",
    "morning",
    "pr2",
    "the",
    "a",
    "every",
    "to",
    "of",
    "42",
    "7up",
    "supercalifragilistic",
    "internationalization",
    "x",
];

/// The small words of [`WORDS`] a name leaves out.
const LEFT_OUT: [&str; 5] = ["the", "a", "every", "to", "of"];

/// A name for an agent comes from its purpose: the first three of its
/// words that name something, lower case, joined by dashes, cut to forty
/// characters with no dash left at the end; when that does not make a
/// name (an agent's name starts with a letter), `agent`. Purposes are
/// drawn from [`WORDS`] between runs of punctuation, spaces and `é`;
/// any other text gives a valid name too.
#[hegel::test(test_cases = 500)]
fn a_suggested_name_is_the_purposes_first_words(tc: hegel::TestCase) {
    let words: Vec<&str> =
        tc.draw(gs::vecs(gs::sampled_from(WORDS.to_vec())).max_size(7));
    let mut purpose = tc.draw(gs::sampled_from(vec!["", " ", "!!"])).to_owned();
    for word in &words {
        purpose.push_str(word);
        purpose.push_str(
            tc.draw(gs::sampled_from(vec![" ", ", ", "-", "/", "é", "!!"])),
        );
    }
    let kept: Vec<String> = words
        .iter()
        .map(|word| word.to_ascii_lowercase())
        .filter(|word| !LEFT_OUT.contains(&word.as_str()))
        .take(3)
        .collect();
    let joined = kept.join("-");
    let cut: String = joined.chars().take(40).collect();
    let cut = cut.trim_end_matches('-');
    let rule = regex::Regex::new(r"\A[a-z][a-z0-9-]{0,39}\z").unwrap();
    let expected = if rule.is_match(cut) {
        if joined.len() > 40 {
            tc.event("cut to forty");
        }
        cut.to_owned()
    } else {
        tc.event("agent");
        "agent".to_owned()
    };
    assert_eq!(suggest_name(&purpose), expected, "{purpose:?}");

    let any: String = tc.draw(gs::text().max_size(200));
    assert!(valid_name(&suggest_name(&any)), "{any:?}");
}

#[test]
fn every_command_trigger_is_a_composer_command() {
    let overview = Overview {
        agents: vec![
            agent(
                "pr-triage",
                vec![
                    trigger(TriggerKind::Command {
                        command: "triage".into(),
                        hint: Some("<number>".into()),
                    }),
                    trigger(TriggerKind::Schedule {
                        every: Some("1h".into()),
                        at: None,
                        days: Vec::new(),
                    }),
                ],
            ),
            agent(
                "digest",
                vec![trigger(TriggerKind::Command {
                    command: "digest".into(),
                    hint: None,
                })],
            ),
        ],
    };
    let listed: Vec<(String, String, String)> = commands(&overview)
        .into_iter()
        .map(|command| (command.name, command.args, command.hint))
        .collect();
    assert_eq!(
        listed,
        [
            ("triage".into(), "<number>".into(), "Tell pr-triage".into()),
            ("digest".into(), String::new(), "Tell digest".into()),
        ]
    );
}

#[test]
fn an_agent_needs_the_person_at_its_cap_or_broken() {
    let mut agent = agent("a", Vec::new());
    assert!(!agent.needs_you());
    agent.status = Status::AtCap;
    assert!(agent.needs_you());
    agent.status = Status::Broken {
        error: "no agent.luau".into(),
    };
    assert!(agent.needs_you());
    agent.spend_today = 3.0;
    assert_eq!(agent.spent_share(), 1.0);
}

#[test]
fn a_stream_says_whether_it_is_open() {
    let mut stream = trigger(TriggerKind::Stream {
        url: "https://ci.example.com/events".into(),
        via: tau_agents::StreamVia::Sse,
        headers: None,
    });
    assert_eq!(stream.label(), "https://ci.example.com/events");
    assert_eq!(stream.kind_label(), "stream");
    assert_eq!(stream.connection(), None);
    stream.live = Some(true);
    assert_eq!(stream.connection(), Some("open"));
    stream.live = Some(false);
    assert_eq!(stream.connection(), Some("closed"));
}

#[test]
fn an_mcp_trigger_says_whether_it_watches() {
    let mut watch = trigger(TriggerKind::Mcp {
        server: "ci".into(),
        uri: "ci://tau-agent/main".into(),
    });
    assert_eq!(watch.label(), "ci ci://tau-agent/main");
    assert_eq!(watch.kind_label(), "mcp");
    watch.live = Some(true);
    assert_eq!(watch.connection(), Some("watching"));
    watch.live = Some(false);
    assert_eq!(watch.connection(), Some("not watching"));
}

#[test]
fn an_agent_is_its_main_chat_only() {
    let overview = Overview {
        agents: vec![Agent {
            main: Some("its-main".into()),
            ..agent("pr-triage", Vec::new())
        }],
    };
    assert!(overview.agent_of_main("pr-triage", "its-main").is_some());
    // A sub-agent or chat in its repository is not the agent.
    assert!(overview.agent_of_main("pr-triage", "a-sub-agent").is_none());
    // Before the host names its main chat, no run is the agent.
    let unnamed = Overview {
        agents: vec![agent("pr-triage", Vec::new())],
    };
    assert!(unnamed.agent_of_main("pr-triage", "its-main").is_none());
}

#[test]
fn the_form_offers_the_person_s_mcp_servers_that_are_on() {
    use tau_agents::ui::mcp_servers;
    let data = serde_json::json!({
        "started": true,
        "servers": [
            { "name": "github", "enabled": true },
            { "name": "linear" },
            { "name": "off", "enabled": false },
        ],
    });
    assert_eq!(mcp_servers(Some(&data)), ["github", "linear"]);
    // Without tau-mcp's data, or with a shape it does not know, none.
    assert!(mcp_servers(None).is_empty());
    assert!(mcp_servers(Some(&serde_json::json!([1, 2]))).is_empty());
}
