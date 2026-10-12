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
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Count, Kind::Top, Kind::Needle];

    pub fn name(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Top => "top",
            Self::Needle => "needle",
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
        Kind::Count | Kind::Needle => [220, 200, 200, 190, 190],
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
        || (kind == Kind::Count && given.replace(',', "").trim() == expected);
    let score = if exact {
        1.0
    } else if kind == Kind::Count {
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

    #[test]
    fn the_same_seed_makes_the_same_file_and_answer() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let one = generate(Kind::Count, 300, 7, a.path()).unwrap();
        let two = generate(Kind::Count, 300, 7, b.path()).unwrap();
        assert_eq!(one.expected, two.expected);
        assert_eq!(
            std::fs::read(a.path().join(FILE)).unwrap(),
            std::fs::read(b.path().join(FILE)).unwrap()
        );
    }

    #[test]
    fn the_count_is_the_number_of_billing_lines() {
        let dir = tempfile::tempdir().unwrap();
        let one = generate(Kind::Count, 500, 3, dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert_eq!(text.lines().count(), 500);
        // A line is billing when its words are one of the pool's, in
        // order around the filled slots.
        let matches = |text: &str, core: &str| {
            let mut rest = text;
            let mut parts = core.split(['{', '}']).enumerate();
            parts.all(|(at, part)| {
                // Odd parts are slot names.
                if at % 2 == 1 || part.is_empty() {
                    return true;
                }
                match rest.find(part) {
                    Some(found) => {
                        rest = &rest[found + part.len()..];
                        true
                    }
                    None => false,
                }
            })
        };
        let billing = text
            .lines()
            .filter(|line| {
                let text = line.split(" | ").nth(1).unwrap();
                BILLING.iter().any(|core| matches(text, core))
            })
            .count();
        assert_eq!(one.expected, billing.to_string());
    }

    #[test]
    fn the_needle_is_in_the_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let one = generate(Kind::Needle, 400, 5, dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert_eq!(text.matches(&one.expected).count(), 1);
    }

    #[test]
    fn top_has_a_clear_winner() {
        let dir = tempfile::tempdir().unwrap();
        let one = generate(Kind::Top, 2000, 9, dir.path()).unwrap();
        assert_eq!(one.expected, "performance");
    }

    #[test]
    fn answers_are_read_from_the_last_answer_line() {
        let good = score(Kind::Count, "42", "I counted.\nANSWER: 42");
        assert!(good.exact && good.score == 1.0);
        let near = score(Kind::Count, "100", "ANSWER: 90");
        assert!(!near.exact);
        assert!((near.score - 0.9).abs() < 1e-9);
        assert!(score(Kind::Top, "login", "**ANSWER: Login.**").exact);
        assert_eq!(score(Kind::Count, "4", "no idea").answer, None);
        assert!(score(Kind::Count, "1234", "ANSWER: 1,234").exact);
    }
}
