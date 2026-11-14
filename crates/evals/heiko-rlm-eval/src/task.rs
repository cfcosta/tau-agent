//! The inputs and their questions. One generated file of support
//! messages, a few hundred to tens of thousands of lines, and a question
//! whose answer is known from how the file was made.
//!
//! - `count` asks how many messages are about billing: every line counts
//!   toward the answer, and a line's category is what it is mainly about,
//!   not which words it holds (a message asking for a refund button on the
//!   invoice page is a feature request). It is the shape of the paper's
//!   OOLONG: work that grows with the input.
//! - `top` asks which category has the most messages, so every category
//!   must be counted.
//! - `needle` asks for one passphrase hidden in the file, which `grep`
//!   finds: a control, where no recursion is needed.

use std::path::Path;

use crate::rng::Rng;

/// Where the messages are, relative to the working directory.
pub const FILE: &str = "data/tickets.txt";

pub const CATEGORIES: [&str; 5] =
    ["billing", "login", "performance", "feature", "praise"];

const BILLING: &[&str] = &[
    "I was charged twice for my subscription in {month}",
    "the invoice for {month} shows an amount I never agreed to",
    "please refund the payment I made for the {item} add-on",
    "my card was billed ${n} after I had already cancelled",
    "the receipt does not match what I actually paid in {month}",
    "why did the price of my plan go up at renewal",
    "I need a corrected tax invoice for the order I placed in {month}",
    "a duplicate charge of ${n} appeared on my statement",
    "my bank shows two identical entries from you for ${n}",
    "I was billed for seats we removed back in {month}",
    "the coupon code was ignored and I paid full price",
    "I still have not got my money back for the cancelled {item} order",
    "you took ${n} from my account and I cannot find what it was for",
    "our finance department says your last statement does not add up",
    "I upgraded for a day and was charged for a whole year",
    "the trial was supposed to be free but my card got hit for ${n}",
    "can someone explain the extra line on this month's bill",
    "my payment bounced, yet the money left my account anyway",
    "I am being taxed twice on the same purchase",
    "the annual fee was collected even though I chose monthly",
    "we were promised a credit note in {month} and nothing has arrived",
    "the amount on the invoice is in the wrong currency",
    "I got a payment reminder for a bill I settled weeks ago",
    "someone on my team was charged for the {item} plan by mistake",
    "the charge for {month} came through twice, a minute apart",
    "I disputed the ${n} payment with my bank because nobody answers my emails about it",
    "the pro-rated amount you billed me makes no sense",
    "my refund was approved but the money never showed up",
    "I never authorised the renewal that cost me ${n}",
    "the price on the checkout page was lower than what I was billed",
    "please send a receipt for the ${n} I paid in {month}",
    "the discount I was quoted is missing from my invoice",
    "the page that lists my invoices is slow, but what bothers me is that two of them are for the same ${n}",
    "I would love a refund button, the support chat refused to give me my ${n} back",
    "thanks for the new invoice layout, but you still billed me twice in {month}",
    "I cannot sign in to download my invoice, and it shows ${n} I do not owe",
];

const LOGIN: &[&str] = &[
    "I cannot sign in to my account on the {item}",
    "the password reset email never arrives",
    "two-factor codes are rejected every time I try",
    "my session logs me out after a minute",
    "it says my account is locked and offers no way to fix it",
    "the single sign-on page loops back to the start",
    "I forgot which email address I registered with",
    "the login button does nothing on my phone",
    "I get an unknown user error although I registered last year",
    "the verification link expired before I could open it",
    "my colleague can get in with the same credentials but I cannot",
    "I changed my password and now nothing is accepted",
    "the authenticator app shows a code that the site says is wrong",
    "I keep being asked to confirm my identity over and over",
    "after the update I am thrown back to the sign-in screen on every click",
    "my account was suspended for suspicious activity but I did nothing",
    "the magic link in the email opens a blank page",
    "I lost my phone and cannot get past the second step",
    "it accepts my password and then says the session is invalid",
    "our SSO provider works everywhere else except with you",
    "the recovery questions do not match anything I remember setting",
    "I am locked out since {month} and support tickets go nowhere",
    "I cannot get into the admin console with my usual login",
    "the app says my token has expired every few minutes",
    "I was removed from the workspace and cannot sign back in",
    "the sign-in page takes ages to load, then rejects my password",
    "please let me back into my account, the invoice for {month} is due and I cannot open it",
    "I wish the login screen would stop forgetting me, today it rejected my password three times",
    "thanks for the quick reply, though I still cannot log in on the {item}",
    "it would be nice to reset my password without email, because the reset link never reaches me",
    "the passkey I registered is not recognised on a new laptop",
    "I get a generic error when I try to authenticate",
    "my username seems to have vanished from your system",
    "captcha fails every time so I cannot reach the password prompt",
    "the code sent by text message arrives after it has expired",
    "I am not able to access my profile since the weekend",
];

const PERFORMANCE: &[&str] = &[
    "the {item} takes ages to load",
    "exports time out after a few minutes",
    "everything freezes when I open a large project",
    "search results are painfully slow today",
    "the app uses all my memory and crashes after an hour",
    "scrolling the list is very laggy on my laptop",
    "uploads stall at ninety percent",
    "pages take half a minute to render",
    "the {item} grinds to a halt when two of us edit at once",
    "syncing used to take seconds and now takes an hour",
    "I see a spinning wheel for a long time whenever I switch tabs",
    "the {item} became unusable after the last release, every click lags",
    "my browser tab runs out of memory and dies",
    "reports that used to be instant now take several minutes",
    "typing in the editor has a noticeable delay",
    "the app crashes whenever I attach a big file",
    "it hangs on startup for nearly a minute",
    "loading the {item} burns through my battery and heats up my machine",
    "the server keeps timing out during our morning rush",
    "bulk imports of ${n} rows never finish",
    "switching between projects is incredibly slow",
    "the whole interface stutters when notifications arrive",
    "it takes forever to open the invoice page, I just wanted to check a figure",
    "the sign-in screen hangs for ages before it shows anything",
    "refunds take ages to appear in the list, the page keeps timing out",
    "I would love a faster {item}, right now each action waits several seconds",
    "thanks for the new features, but the {item} is now twice as slow",
    "the {item} froze again in {month} and I lost my work",
    "response times are terrible since yesterday",
    "the mobile app freezes on the splash screen",
    "downloads start fast and then crawl to a stop",
    "filters take so long to apply that I assume it has crashed",
    "the page keeps timing out while it loads my data",
    "latency to your API is awful from our office",
    "the calendar view never finishes rendering",
    "the nightly job now takes eight hours instead of one",
];

const FEATURE: &[&str] = &[
    "it would be great to have dark mode in the {item}",
    "can you add a way to export to a spreadsheet",
    "please support bulk editing of items",
    "we need keyboard shortcuts for common actions",
    "an option to schedule reports would help our team",
    "could the calendar sync with other tools",
    "I wish I could filter by custom fields",
    "support for multiple workspaces would be amazing",
    "can you add a refund button to the invoice page",
    "it would help if the login screen remembered my company",
    "please let us choose how often we are billed",
    "an offline mode would make this so much better",
    "we would like to attach comments to individual rows",
    "is there any plan for a public API for the {item}",
    "it would be useful to see a history of changes per item",
    "please add an undo button, I deleted something by accident",
    "could you let admins set default views for the whole team",
    "I would like to pin my favourite reports to the top",
    "any chance of a native app for tablets",
    "templates for recurring tasks would save us a lot of time",
    "please make the {item} customisable with our brand colours",
    "we need a way to archive old projects instead of deleting them",
    "can invoices be sent automatically to our accountant every {month}",
    "allow signing in with a hardware key, please",
    "it would be fantastic if the {item} could run in the background",
    "a way to speed up large imports with a batch mode would be a great addition",
    "I would like two-factor codes by email as an alternative",
    "support for right-to-left languages would help our office",
    "please add a way to merge duplicate contacts",
    "could you add webhooks for when an item changes",
    "let us tag items and search by tag",
    "an option for a compact layout would be nice",
    "we would love SSO with our identity provider",
    "can you add a per-seat usage report for billing purposes",
    "a shortcut to duplicate a project would be handy",
    "please offer an export of all my data in a standard format",
];

const PRAISE: &[&str] = &[
    "the new update is fantastic",
    "your support team solved my problem in minutes",
    "I love how simple the setup was",
    "this has saved our team hours every week",
    "the redesign of the {item} looks wonderful",
    "best tool we have used for this",
    "thank you, everything works beautifully",
    "onboarding was a delight",
    "thanks for sorting out my duplicate charge so quickly",
    "signing in is so smooth now, great work",
    "it is so fast these days, well done",
    "the invoices are clear and easy to read, thank you",
    "the {item} is exactly what we were missing",
    "whoever wrote the documentation deserves a medal",
    "our whole department switched in {month} and nobody wants to go back",
    "the team that answered my ticket was kind and patient",
    "I recommend you to everyone I know",
    "you listened to our feedback and it shows, thank you",
    "five stars, would use again",
    "the keyboard shortcuts are a joy",
    "it just works, which is rare",
    "the latest release made my daily routine so much easier",
    "I was sceptical at first but the {item} won me over",
    "my refund arrived within a day, impressive service",
    "thank you for fixing the login problem so fast",
    "the export finished in seconds, brilliant",
    "great value for the price",
    "I appreciate how transparent your billing is",
    "the mobile app is lovely to use",
    "your changelog is the best I have read",
    "such a pleasant experience from sign-up to first report",
    "we saved ${n} a month after moving to your product",
    "the {item} has never let us down",
    "thanks for adding the feature I asked for in {month}, it is perfect",
    "cheers to the engineers, no downtime in {month}",
    "the customer success manager went above and beyond",
];

const OPENERS: &[&str] = &[
    "Hi team,",
    "Hello,",
    "Quick note:",
    "Hey,",
    "Good morning,",
    "FYI:",
];

const ITEMS: &[&str] = &[
    "dashboard",
    "report builder",
    "mobile app",
    "admin console",
    "calendar",
    "team settings",
    "export tool",
    "search page",
    "inbox",
    "billing page",
];

const MONTHS: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const FILLERS: &[&str] = &[
    "I'm on the team plan.",
    "Using the desktop app.",
    "This is my third time writing in.",
    "We are a small company of twelve.",
    "Running the latest version.",
    "I mostly use it on weekdays.",
    "Our account manager suggested I write.",
];

/// The kinds of question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Count,
    Top,
    Needle,
    /// Real text: how many of a project's documents are about codemode.
    Docs,
}

impl Kind {
    pub const ALL: [Kind; 4] =
        [Kind::Count, Kind::Top, Kind::Needle, Kind::Docs];

    pub fn name(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Top => "top",
            Self::Needle => "needle",
            Self::Docs => "docs",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// One generated input and its question.
#[derive(Debug, Clone)]
pub struct Instance {
    pub kind: Kind,
    pub lines: usize,
    pub bytes: usize,
    pub prompt: String,
    pub expected: String,
}

fn pool(category: usize) -> &'static [&'static str] {
    [BILLING, LOGIN, PERFORMANCE, FEATURE, PRAISE][category]
}

/// The share of the messages in each category, in per mille. `top` has a
/// winner; the others are spread so that `count` is about a fifth.
fn weights(kind: Kind) -> [usize; 5] {
    match kind {
        Kind::Top => [180, 170, 300, 190, 160],
        Kind::Count | Kind::Needle | Kind::Docs => [220, 200, 200, 190, 190],
    }
}

/// Writes the file under `dir` and returns its question.
pub fn generate(
    kind: Kind,
    lines: usize,
    seed: u64,
    dir: &Path,
) -> std::io::Result<Instance> {
    let mut rng = Rng::new(seed);
    let weights = weights(kind);
    let mut counts = [0usize; 5];
    let mut out = String::new();
    let needle_at = rng.range(lines / 10, lines - lines / 10);
    let mut passphrase = String::new();
    for n in 0..lines {
        let date =
            format!("2025-{:02}-{:02}", rng.range(1, 13), rng.range(1, 29));
        let user = rng.range(1, 9_000);
        if kind == Kind::Needle && n == needle_at {
            passphrase = format!(
                "{}-{}-{}",
                rng.pick(&["amber", "cobalt", "juniper", "saffron"]),
                rng.pick(&["falcon", "harbor", "lantern", "meadow"]),
                rng.range(10, 100)
            );
            out.push_str(&format!(
                "{n:06} {date} user_{user:04} | Note for the on-call: the \
                 deployment passphrase is {passphrase}.\n"
            ));
            continue;
        }
        let mut roll = rng.range(0, 1000);
        let category = weights
            .iter()
            .position(|weight| {
                if roll < *weight {
                    true
                } else {
                    roll -= weight;
                    false
                }
            })
            .unwrap_or(4);
        counts[category] += 1;
        let mut text = String::new();
        if rng.range(0, 10) < 4 {
            text.push_str(rng.pick(OPENERS));
            text.push(' ');
        }
        let core = rng
            .pick(pool(category))
            .replace("{item}", rng.pick(ITEMS))
            .replace("{month}", rng.pick(MONTHS))
            .replace("{n}", &rng.range(5, 480).to_string());
        text.push_str(&core);
        text.push('.');
        if rng.range(0, 2) == 0 {
            text.push(' ');
            text.push_str(rng.pick(FILLERS));
        }
        out.push_str(&format!("{n:06} {date} user_{user:04} | {text}\n"));
    }
    let path = dir.join(FILE);
    std::fs::create_dir_all(path.parent().expect("a file has a parent"))?;
    std::fs::write(&path, &out)?;

    let (prompt, expected) = match kind {
        Kind::Docs => unreachable!("documents are made by `generate_docs`"),
        Kind::Count => (
            format!(
                "{} How many messages are billing messages? Finish \
                     your reply with a line `ANSWER: <integer>`.",
                intro(lines, out.len())
            ),
            counts[0].to_string(),
        ),
        Kind::Top => (
            format!(
                "{} Which category has the most messages? Finish \
                     your reply with a line `ANSWER: <category>`, one of \
                     billing, login, performance, feature, praise.",
                intro(lines, out.len())
            ),
            CATEGORIES[counts
                .iter()
                .enumerate()
                .max_by_key(|(_, count)| **count)
                .map(|(at, _)| at)
                .unwrap_or(0)]
            .to_owned(),
        ),
        Kind::Needle => (
            format!(
                "The file `{FILE}` holds {lines} customer support messages, \
                 one per line (about {} KB). One line, among them, is a note \
                 for the on-call that gives the deployment passphrase. What \
                 is it? Finish your reply with a line `ANSWER: <passphrase>`.",
                out.len() / 1000
            ),
            passphrase,
        ),
    };
    Ok(Instance {
        kind,
        lines,
        bytes: out.len(),
        prompt,
        expected,
    })
}

/// The corpus of `docs`: every Markdown file under `root`, in an order
/// the seed shuffles, each under a `### DOCUMENT k` line with its first
/// heading removed, so a title does not give its subject away. The
/// answer is how many are about codemode, by their path.
pub fn generate_docs(
    root: &Path,
    seed: u64,
    dir: &Path,
) -> std::io::Result<Instance> {
    fn walk(
        dir: &Path,
        out: &mut Vec<std::path::PathBuf>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path.extension().is_some_and(|ext| ext == "md") {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    let mut rng = Rng::new(seed);
    for at in (1..files.len()).rev() {
        files.swap(at, rng.range(0, at + 1));
    }
    let mut out = String::new();
    let mut about = 0;
    for (n, path) in files.iter().enumerate() {
        let text = std::fs::read_to_string(path)?;
        let body = match text.split_once('\n') {
            Some((first, rest)) if first.starts_with('#') => rest,
            _ => &text,
        };
        if path.to_string_lossy().to_lowercase().contains("codemode") {
            about += 1;
        }
        out.push_str(&format!("### DOCUMENT {}\n{}\n\n", n + 1, body.trim()));
    }
    let path = dir.join(FILE);
    std::fs::create_dir_all(path.parent().expect("a file has a parent"))?;
    std::fs::write(&path, &out)?;
    let prompt = format!(
        "The file `{FILE}` holds {} technical documents from one software \
         project (about {} KB), each under a line `### DOCUMENT <number>`; \
         the title line of each has been removed. How many of the documents \
         are about codemode, the project's Luau scripting tool: its sandbox, \
         globals, modules, inference, evaluation, or how scripts read \
         artifacts and call tools? A document that only mentions codemode in \
         passing, as one tool among others, does not count. Finish your \
         reply with a line `ANSWER: <integer>`.",
        files.len(),
        out.len() / 1000
    );
    Ok(Instance {
        kind: Kind::Docs,
        lines: out.lines().count(),
        bytes: out.len(),
        prompt,
        expected: about.to_string(),
    })
}

fn intro(lines: usize, bytes: usize) -> String {
    format!(
        "The file `{FILE}` holds {lines} customer support messages, one per \
         line (about {} KB). Each message is mainly about exactly one of five \
         things: billing (charges, invoices, refunds, payments), login \
         (signing in, passwords, two-factor codes, locked accounts), \
         performance (slowness, freezes, crashes, timeouts), feature \
         (requests for new capabilities or options), or praise (thanks or \
         compliments, with no problem to solve). Classify each message by \
         what the customer is mainly saying, not by words that happen to \
         appear in it.",
        bytes / 1000
    )
}

/// How an answer scored.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Score {
    /// What the reply answered, if it had an `ANSWER:` line.
    pub answer: Option<String>,
    pub exact: bool,
    /// 1.0 when exact; for a count, 1 minus its relative error, at least 0.
    pub score: f64,
}

/// Scores `reply`, whose last `ANSWER:` line is its answer.
pub fn score(kind: Kind, expected: &str, reply: &str) -> Score {
    let answer = reply.lines().rev().find_map(|line| {
        let line = line.trim().trim_start_matches(['*', '`', ' ']);
        let rest = line.strip_prefix("ANSWER:")?;
        Some(rest.trim().trim_matches(['*', '`', '.', ' ']).to_owned())
    });
    let Some(given) = answer.clone() else {
        return Score {
            answer,
            exact: false,
            score: 0.0,
        };
    };
    let exact = given.eq_ignore_ascii_case(expected)
        || (matches!(kind, Kind::Count | Kind::Docs)
            && given.replace(',', "").trim() == expected);
    let score = if exact {
        1.0
    } else if matches!(kind, Kind::Count | Kind::Docs) {
        match (
            given.replace(',', "").trim().parse::<f64>(),
            expected.parse::<f64>(),
        ) {
            (Ok(given), Ok(expected)) if expected > 0.0 => {
                (1.0 - (given - expected).abs() / expected).max(0.0)
            }
            _ => 0.0,
        }
    } else {
        0.0
    };
    Score {
        answer,
        exact,
        score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `text` is exactly `core` with its slots filled: `{item}`
    /// and `{month}` from their lists, `{n}` with digits.
    fn fills(text: &str, core: &str) -> bool {
        let Some(open) = core.find('{') else {
            return text == core;
        };
        let close = open + core[open..].find('}').unwrap();
        let Some(rest) = text.strip_prefix(&core[..open]) else {
            return false;
        };
        let after = &core[close + 1..];
        let values: Vec<String> = match &core[open + 1..close] {
            "item" => ITEMS.iter().map(|item| (*item).to_owned()).collect(),
            "month" => MONTHS.iter().map(|month| (*month).to_owned()).collect(),
            "n" => (1..=rest.len())
                .map(|end| &rest[..end])
                .take_while(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
                .map(str::to_owned)
                .collect(),
            slot => panic!("unknown slot {slot}"),
        };
        values.iter().any(|value| {
            rest.strip_prefix(value.as_str())
                .is_some_and(|rest| fills(rest, after))
        })
    }

    /// The categories whose pool could have written the message on
    /// `line`: an opener or none, a filled core and its period, then a
    /// filler or none.
    fn categories_of(line: &str) -> Vec<usize> {
        let text = line.split(" | ").nth(1).unwrap();
        let openers = std::iter::once(String::new())
            .chain(OPENERS.iter().map(|opener| format!("{opener} ")));
        let bodies: Vec<&str> = openers
            .filter_map(|opener| text.strip_prefix(opener.as_str()))
            .flat_map(|body| {
                std::iter::once(body).chain(FILLERS.iter().filter_map(
                    move |filler| body.strip_suffix(&format!(" {filler}")),
                ))
            })
            .filter_map(|body| body.strip_suffix('.'))
            .collect();
        (0..CATEGORIES.len())
            .filter(|category| {
                pool(*category)
                    .iter()
                    .any(|core| bodies.iter().any(|body| fills(body, core)))
            })
            .collect()
    }

    /// For any kind, seed and length, the file has that many lines and
    /// the same seed makes the same file and answer; a `needle` answer
    /// occurs in the file once, and a `count` answer is the number of
    /// lines written from the billing pool.
    #[hegel::test(test_cases = 50)]
    fn generated_tasks_keep_their_promises(tc: hegel::TestCase) {
        use hegel::generators::{self as gs, Generator as _};
        let kind = tc.draw(
            gs::sampled_from(vec![Kind::Count, Kind::Top, Kind::Needle])
                .print_as_debug(),
        );
        let seed = tc.draw(gs::integers::<u64>());
        let lines = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![100_usize, 2_000]),
            gs::integers::<usize>().min_value(100).max_value(2_000),
        ));
        let (a, b) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let one = generate(kind, lines, seed, a.path()).unwrap();
        let two = generate(kind, lines, seed, b.path()).unwrap();
        let text = std::fs::read_to_string(a.path().join(FILE)).unwrap();
        assert_eq!(text.lines().count(), lines);
        assert_eq!(one.expected, two.expected);
        assert_eq!(text, std::fs::read_to_string(b.path().join(FILE)).unwrap());
        match kind {
            Kind::Needle => {
                assert_eq!(text.matches(&one.expected).count(), 1);
            }
            Kind::Count => {
                let billing = text
                    .lines()
                    .filter(|line| categories_of(line).contains(&0))
                    .count();
                assert_eq!(one.expected, billing.to_string());
            }
            _ => {}
        }
    }

    #[test]
    fn the_docs_corpus_hides_titles_and_counts_codemode_paths() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("sub")).unwrap();
        for (path, text) in [
            ("a.md", "# Alpha title\nabout things"),
            ("codemode-x.md", "# Secret title\nscripts"),
            ("sub/codemode-y.md", "# Another\nmore scripts"),
        ] {
            std::fs::write(root.path().join(path), text).unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let one = generate_docs(root.path(), 1, dir.path()).unwrap();
        assert_eq!(one.expected, "2");
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(!text.contains("title") && text.contains("### DOCUMENT 3"));
    }

    /// For any seed, `top` names the category the most messages in the
    /// file were written from, counted from the file itself; and its
    /// weights make that `performance` once there are a thousand lines.
    #[hegel::test(test_cases = 50)]
    fn top_names_the_category_with_the_most_messages(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let seed = tc.draw(gs::integers::<u64>());
        let lines =
            tc.draw(gs::integers::<usize>().min_value(1_000).max_value(3_000));
        let dir = tempfile::tempdir().unwrap();
        let one = generate(Kind::Top, lines, seed, dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        let mut counts = [0usize; 5];
        for line in text.lines() {
            let [category] = categories_of(line)[..] else {
                panic!("not from exactly one pool: {line}");
            };
            counts[category] += 1;
        }
        let most = (0..counts.len()).max_by_key(|at| counts[*at]).unwrap();
        assert_eq!(one.expected, CATEGORIES[most], "{counts:?}");
        assert_eq!(one.expected, "performance", "{counts:?}");
    }

    /// `n` as a person may write it: plain, or with thousands commas.
    fn written(n: u64, commas: bool) -> String {
        let digits = n.to_string();
        if !commas {
            return digits;
        }
        let mut out = String::new();
        for (at, digit) in digits.chars().enumerate() {
            if at > 0 && (digits.len() - at).is_multiple_of(3) {
                out.push(',');
            }
            out.push(digit);
        }
        out
    }

    /// An `ANSWER:` line, decorated as models write them.
    fn answer_line(tc: &hegel::TestCase, answer: &str) -> String {
        use hegel::generators as gs;
        match tc.draw(gs::integers::<u8>().max_value(3)) {
            0 => format!("ANSWER: {answer}"),
            1 => format!("**ANSWER: {answer}**"),
            2 => format!("`ANSWER: {answer}`"),
            _ => format!("  ANSWER: {answer}."),
        }
    }

    /// Prose and answer lines in any order: the last `ANSWER:` line is
    /// the answer; a count is exact when it is the number, commas aside,
    /// and otherwise scores one minus its relative error, at least 0; a
    /// reply with no answer line scores 0.
    #[hegel::test(test_cases = 500)]
    fn a_count_is_read_from_the_last_answer_line(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let expected =
            tc.draw(gs::integers::<u64>().min_value(1).max_value(100_000));
        let answers: Vec<u64> = tc.draw(
            gs::vecs(hegel::one_of!(
                gs::just(expected),
                gs::integers::<u64>().max_value(250_000),
            ))
            .max_size(4),
        );
        let mut reply = Vec::new();
        for answer in &answers {
            if tc.draw(gs::booleans()) {
                let prose = tc.draw(gs::sampled_from(vec![
                    "I counted.",
                    "",
                    "Let me check again.",
                ]));
                reply.push(prose.to_owned());
            }
            let commas = tc.draw(gs::booleans());
            reply.push(answer_line(&tc, &written(*answer, commas)));
        }
        let read = score(Kind::Count, &expected.to_string(), &reply.join("\n"));
        let Some(&last) = answers.last() else {
            assert_eq!(
                (read.answer, read.exact, read.score),
                (None, false, 0.0)
            );
            return;
        };
        assert_eq!(
            read.answer.as_deref().map(|answer| answer.replace(',', "")),
            Some(last.to_string()),
            "{reply:?}"
        );
        assert_eq!(read.exact, last == expected, "{reply:?}");
        let error = last.abs_diff(expected) as f64 / expected as f64;
        let want = if last == expected {
            1.0
        } else {
            (1.0 - error).max(0.0)
        };
        assert!((read.score - want).abs() < 1e-9, "{} != {want}", read.score);
    }

    /// A category is exact whatever its case, and scores nothing when it
    /// is another one.
    #[hegel::test(test_cases = 200)]
    fn a_category_is_exact_in_any_case(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let expected = tc.draw(gs::sampled_from(CATEGORIES.to_vec()));
        let given = tc.draw(gs::sampled_from(CATEGORIES.to_vec()));
        let cased: String = given
            .chars()
            .map(|c| {
                if tc.draw(gs::booleans()) {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        let read = score(Kind::Top, expected, &answer_line(&tc, &cased));
        assert_eq!(read.exact, given == expected);
        assert_eq!(read.score, if given == expected { 1.0 } else { 0.0 });
    }
}
