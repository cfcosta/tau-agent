//! Building lines with a model (`docs/reference/tree-compaction.md`,
//! "Building lines"), against a stand-in that answers by what it is
//! asked.
//!
//! Property inventory:
//! - growing builds every line the view holds and leaves it within its
//!   budget, asking only for lines that need a model (oracle: the free
//!   rule, recomputed from the requests);
//! - no request shows a line's label, which a compactor copies into its
//!   output (OptChat, section 4.2);
//! - no more than `jobs` requests run at once;
//! - cutting at a byte limit never splits a character.

use std::sync::{
    Arc,
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use hegel::{
    TestCase,
    generators::{self as gs, Generator as _},
};
use regex::Regex;
use tau_agent::plugin::AskError;
use tau_ai::{
    message::{
        AssistantBlock,
        AssistantMessage,
        InputBlock,
        Message,
        StopReason,
        TextContent,
        Timestamp,
        Usage,
        UserContent,
    },
    responses::request::Settings,
};
use tau_testing::block_on;
use tau_tree_compaction::{
    build::{
        Ask,
        BuildError,
        Builder,
        COMPACT_PROMPT,
        RECENT_CHARS,
        SCALE,
        TRIES,
        cut_bytes,
    },
    tree::{Entry, History, Kind, NODE_BYTES, Node},
};

/// A request the stand-in got: its instructions and its messages.
#[derive(Debug, Clone)]
struct Asked {
    settings: Settings,
    input: Vec<Message>,
}

impl Asked {
    /// Every text the request shows, in order.
    fn texts(&self) -> Vec<String> {
        self.input
            .iter()
            .flat_map(|message| match message {
                Message::User(user) => match &user.content {
                    UserContent::Text(text) => vec![text.clone()],
                    UserContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|block| match block {
                            InputBlock::Text(text) => Some(text.text.clone()),
                            InputBlock::Image(_) => None,
                        })
                        .collect(),
                },
                Message::Assistant(reply) => vec![reply.text()],
                Message::ToolResult(_) => Vec::new(),
            })
            .collect()
    }

    /// The step: what the first message's second block asks.
    fn step(&self) -> String {
        self.texts()[1].clone()
    }
}

type Answer = dyn Fn(&Asked) -> Result<String, String> + Send + Sync;

/// A model that answers each request with `answer`, keeping every
/// request and how many ran at once.
struct Fake {
    answer: Box<Answer>,
    asked: Mutex<Vec<Asked>>,
    running: AtomicUsize,
    most: AtomicUsize,
}

impl Fake {
    fn new(
        answer: impl Fn(&Asked) -> Result<String, String> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            answer: Box::new(answer),
            asked: Mutex::new(Vec::new()),
            running: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
        })
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }
}

fn reply(
    text: String,
    stop: StopReason,
    error: Option<String>,
) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantBlock::Text(TextContent {
            text,
            text_signature: None,
        })],
        model: "fake".into(),
        response_id: None,
        usage: Usage::default(),
        stop_reason: stop,
        error_message: error,
        timestamp: 0,
    }
}

#[async_trait]
impl Ask for Fake {
    async fn ask(
        &self,
        settings: Settings,
        input: &[Message],
    ) -> Result<AssistantMessage, AskError> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        // Lets the other requests in flight start before this answers.
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        let asked = Asked {
            settings,
            input: input.to_vec(),
        };
        let answer = (self.answer)(&asked);
        self.asked.lock().unwrap().push(asked);
        self.running.fetch_sub(1, Ordering::SeqCst);
        Ok(match answer {
            Ok(text) => reply(text, StopReason::Stop, None),
            Err(message) => {
                reply(String::new(), StopReason::Error, Some(message))
            }
        })
    }

    fn now(&self) -> Timestamp {
        0
    }
}

/// A line of `bytes` bytes for a request, tagged by its step so lines
/// differ.
fn line_of(asked: &Asked, bytes: usize) -> String {
    let step = asked.step();
    let tag = step.lines().last().unwrap_or_default();
    let mut line: String = format!("s:{}", tag.len())
        .chars()
        .chain(std::iter::repeat('y'))
        .take(bytes)
        .collect();
    line.truncate(bytes);
    line
}

/// An entry's text: words with no digits, `+` or `|`, so a label in a
/// request can only come from the request itself.
#[hegel::composite]
fn entry(tc: &TestCase) -> Entry {
    let words = tc.draw(gs::integers::<usize>().min_value(1).max_value(400));
    let kind = tc.draw(
        gs::sampled_from(vec![Kind::User, Kind::Talk, Kind::Tool, Kind::Echo])
            .print_as_debug(),
    );
    let text = (0..words)
        .map(|w| ["alpha", "beta", "gamma"][w % 3])
        .collect::<Vec<_>>()
        .join(" ");
    Entry::new(kind, text)
}

#[hegel::test(test_cases = 60)]
fn growing_builds_every_line_within_budget_asking_only_when_needed(
    tc: TestCase,
) {
    let entries: Vec<Entry> = tc.draw(
        gs::vecs(entry())
            .min_size(16)
            .max_size(120)
            .print_as_debug(),
    );
    // Room for a few lines more than the coarsest view, one per set bit
    // of the count, so most cases must merge.
    let floor = entries.len().count_ones() as usize;
    let lines = tc.draw(
        gs::integers::<usize>()
            .min_value(floor)
            .max_value(floor + 20),
    );
    let budget = lines * NODE_BYTES;
    let bytes =
        tc.draw(gs::integers::<usize>().min_value(100).max_value(NODE_BYTES));
    let fake = Fake::new(move |asked| Ok(line_of(asked, bytes)));
    let mut history = History::new();
    for entry in &entries {
        history.push(entry.clone());
    }
    let grown =
        block_on(Builder::new("m").grow(&mut history, budget, &*fake)).unwrap();
    assert!(history.view().iter().all(|node| history.is_built(*node)));
    assert!(history.view_bytes(0) <= budget);
    let asked = fake.asked();
    tc.event_value("requests", asked.len() as f64);
    tc.event_value("lines", history.view().len() as f64);
    tc.event_value(
        "merges asked",
        asked
            .iter()
            .filter(|asked| asked.step().contains("Merge these"))
            .count() as f64,
    );
    assert_eq!(grown.asked, asked.len());
    // Exactly the long entries were compressed by a model; a short one
    // is its own line, word for word.
    let compressed = asked
        .iter()
        .filter(|asked| asked.step().contains("Compress this message"))
        .count();
    let long = entries
        .iter()
        .filter(|entry| entry.line().len() > NODE_BYTES)
        .count();
    assert_eq!(compressed, long);
    for (id, entry) in entries.iter().enumerate() {
        if entry.line().len() <= NODE_BYTES {
            assert_eq!(
                history.text(Node::leaf(id)),
                Some(entry.line().as_str())
            );
        }
    }
    // A merge asks only when its two lines do not fit together.
    for asked in asked
        .iter()
        .filter(|asked| asked.step().contains("Merge these"))
    {
        let step = asked.step();
        let lines: Vec<&str> = step.lines().rev().take(2).collect();
        assert!(lines[0].len() + 1 + lines[1].len() > NODE_BYTES);
    }
    for asked in &asked {
        assert_eq!(
            asked.settings.instructions.as_deref(),
            Some(COMPACT_PROMPT)
        );
        assert_eq!(asked.texts().len(), 2, "context, then the step");
        assert!(asked.texts()[0].starts_with("<chat>"));
    }
}

#[hegel::test(test_cases = 40)]
fn no_request_shows_a_label(tc: TestCase) {
    let entries: Vec<Entry> =
        tc.draw(gs::vecs(entry()).min_size(2).max_size(60).print_as_debug());
    let fake = Fake::new(|asked| Ok(line_of(asked, 300)));
    let mut history = History::new();
    // Folded in two batches, so the second's requests read the first's
    // view as context.
    let half = entries.len() / 2;
    for entry in &entries[..half] {
        history.push(entry.clone());
    }
    block_on(Builder::new("m").grow(&mut history, 4_000, &*fake)).unwrap();
    for entry in &entries[half..] {
        history.push(entry.clone());
    }
    block_on(Builder::new("m").grow(&mut history, 4_000, &*fake)).unwrap();
    let label = Regex::new(r"\d+\+\d+\|").unwrap();
    for asked in fake.asked() {
        for text in asked.texts() {
            assert!(!label.is_match(&text), "a label in:\n{text}");
        }
    }
}

#[hegel::test(test_cases = 30)]
fn no_more_than_jobs_requests_run_at_once(tc: TestCase) {
    let jobs = tc.draw(gs::integers::<usize>().min_value(1).max_value(6));
    let count = tc.draw(gs::integers::<usize>().min_value(jobs).max_value(40));
    let fake = Fake::new(|asked| Ok(line_of(asked, 400)));
    let mut history = History::new();
    for id in 0..count {
        history.push(Entry::new(
            Kind::Echo,
            format!("{id} {}", "z".repeat(NODE_BYTES)),
        ));
    }
    let mut builder = Builder::new("m");
    builder.jobs = jobs;
    block_on(builder.grow(&mut history, 1_000_000, &*fake)).unwrap();
    let most = fake.most.load(Ordering::SeqCst);
    assert!(most <= jobs, "{most} at once, {jobs} allowed");
    assert_eq!(most, jobs, "every slot is used when there is work for it");
}

#[hegel::test]
fn cutting_at_a_byte_limit_never_splits_a_character(tc: TestCase) {
    let text: String = tc.draw(gs::text().max_size(200));
    let bytes = tc.draw(gs::integers::<usize>().max_value(900));
    let cut = cut_bytes(&text, bytes);
    assert!(text.starts_with(cut));
    assert!(cut.len() <= bytes);
    // The largest such prefix: the next character would pass the limit.
    if let Some(next) = text[cut.len()..].chars().next() {
        assert!(cut.len() + next.len_utf8() > bytes);
    }
}

/// A line over the limit goes back in the same conversation, cut where
/// the limit falls; after every try the shortest is kept.
#[test]
fn a_long_line_goes_back_cut_at_the_limit_and_the_shortest_is_kept() {
    let sizes = [600, 550, 530, 520, 515, 513];
    let tries = Arc::new(AtomicUsize::new(0));
    let counter = tries.clone();
    let fake = Fake::new(move |_| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        Ok("w".repeat(sizes[n]))
    });
    let mut history = History::new();
    history.push(Entry::new(Kind::Echo, "v".repeat(2 * NODE_BYTES)));
    block_on(Builder::new("m").grow(&mut history, 10_000, &*fake)).unwrap();
    assert_eq!(fake.asked().len(), TRIES);
    assert_eq!(history.text(Node::leaf(0)).unwrap().len(), 515);
    let last = fake.asked().pop().unwrap();
    // The first message, then each try and what it was told.
    assert_eq!(last.input.len(), 1 + 2 * (TRIES - 1));
    let Message::User(told) = &last.input[2] else {
        panic!("expected the correction")
    };
    let UserContent::Text(told) = &told.content else {
        panic!("text")
    };
    assert_eq!(
        *told,
        format!(
            "That line is 600 bytes; the limit is {NODE_BYTES}. It must end where it is cut here:\n{}| ← LIMIT",
            "w".repeat(NODE_BYTES)
        )
    );
}

/// A line that fits on its second try is kept, with no third.
#[test]
fn a_line_that_fits_stops_the_tries() {
    let tries = Arc::new(AtomicUsize::new(0));
    let counter = tries.clone();
    let fake = Fake::new(move |_| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        Ok("w".repeat(if n == 0 { 700 } else { 300 }))
    });
    let mut history = History::new();
    history.push(Entry::new(Kind::Echo, "v".repeat(2 * NODE_BYTES)));
    block_on(Builder::new("m").grow(&mut history, 10_000, &*fake)).unwrap();
    assert_eq!(fake.asked().len(), 2);
    assert_eq!(history.text(Node::leaf(0)).unwrap().len(), 300);
}

/// A request that fails, or answers nothing, fails the growth, and the
/// error names the line.
#[test]
fn a_failed_request_fails_the_growth() {
    let fake = Fake::new(|_| Err("overloaded".to_owned()));
    let mut history = History::new();
    history.push(Entry::new(Kind::Echo, "v".repeat(2 * NODE_BYTES)));
    let error = block_on(Builder::new("m").grow(&mut history, 10_000, &*fake))
        .unwrap_err();
    assert!(
        matches!(&error, BuildError::Failed { node, message }
        if node == "0+1" && message == "overloaded"),
        "{error}"
    );
    let fake = Fake::new(|_| Ok("  ".to_owned()));
    let error = block_on(Builder::new("m").grow(&mut history, 10_000, &*fake))
        .unwrap_err();
    assert!(
        matches!(&error, BuildError::Empty { node } if node == "0+1"),
        "{error}"
    );
}

/// A level-0 line reads the view's built lines before it as `<chat>`,
/// and the messages after them, up to its own, as `<recent>`: a line of
/// its batch still being built is read as its message.
#[test]
fn a_message_is_compressed_with_what_came_before_it() {
    let fake = Fake::new(|asked| Ok(line_of(asked, 200)));
    let mut history = History::new();
    history.push(Entry::new(Kind::User, "fix the parser"));
    block_on(Builder::new("m").grow(&mut history, 10_000, &*fake)).unwrap();
    let read = format!("read src/parse.rs {}", "p".repeat(NODE_BYTES));
    history.push(Entry::new(Kind::Tool, read.clone()));
    history.push(Entry::new(Kind::Echo, "q".repeat(2 * NODE_BYTES)));
    history.push(Entry::new(Kind::Talk, "the parser bails on a closer"));
    history.push(Entry::new(Kind::Echo, "r".repeat(2 * NODE_BYTES)));
    block_on(Builder::new("m").grow(&mut history, 10_000, &*fake)).unwrap();
    let asked = fake.asked();
    assert_eq!(asked.len(), 3);
    let context = |message: &str| {
        asked
            .iter()
            .find(|asked| asked.step().ends_with(message))
            .unwrap_or_else(|| panic!("no request for {message:?}"))
            .texts()[0]
            .clone()
    };
    assert_eq!(context(&read), "<chat>\nuser: fix the parser\n</chat>");
    assert_eq!(
        context(&"q".repeat(2 * NODE_BYTES)),
        format!(
            "<chat>\nuser: fix the parser\n</chat>\n<recent>\ntool: {read}\n</recent>"
        )
    );
    // The talk line is built at once, needing no model; it still reads
    // as a message, after the tool line still being built.
    let last = context(&"r".repeat(2 * NODE_BYTES));
    assert!(last.starts_with(
        "<chat>\nuser: fix the parser\n</chat>\n<recent>\ntool: read"
    ));
    assert!(last.ends_with("talk: the parser bails on a closer\n</recent>"));
    let step = asked[0].step();
    assert!(step.starts_with(&format!(
        "For scale, this line is exactly {NODE_BYTES} bytes:\n{SCALE}\n\nCompress this message"
    )));
}

/// The scale line is what it says it is.
#[test]
fn the_scale_line_is_exactly_a_line_long() {
    assert_eq!(SCALE.len(), NODE_BYTES);
    assert!(!SCALE.contains('\n'));
}

/// `<recent>` keeps the newest messages that fit in its 6,000
/// characters, whole lines only: ten of exactly 1,000 before a message
/// leave the newest six.
#[test]
fn recent_keeps_the_newest_messages_that_fit() {
    assert_eq!(RECENT_CHARS, 6_000);
    let fake = Fake::new(|asked| Ok(line_of(asked, 200)));
    let mut history = History::new();
    for n in 0..10 {
        // "echo: " and a tag, then letters to exactly 1,000 characters.
        let tag = format!("m{n} ");
        let text = format!("{tag}{}", "k".repeat(1_000 - 6 - tag.len()));
        history.push(Entry::new(Kind::Echo, text));
    }
    let last = "z".repeat(2 * NODE_BYTES);
    history.push(Entry::new(Kind::Echo, last.clone()));
    block_on(Builder::new("m").grow(&mut history, 1_000_000, &*fake)).unwrap();
    let asked = fake.asked();
    let context = &asked
        .iter()
        .find(|asked| asked.step().ends_with(&last))
        .expect("the last message's request")
        .texts()[0];
    let recent = context
        .split_once("<recent>\n")
        .and_then(|(_, rest)| rest.strip_suffix("\n</recent>"))
        .expect("a recent block");
    let tags: Vec<&str> = recent
        .lines()
        .map(|line| line.split(' ').nth(1).unwrap())
        .collect();
    assert_eq!(tags, ["m4", "m5", "m6", "m7", "m8", "m9"]);
    assert!(recent.lines().all(|line| line.chars().count() == 1_000));
}
