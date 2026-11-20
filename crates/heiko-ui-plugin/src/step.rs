//! A tool call as a step on the spine (ADR 0060, ADR 0064): what only
//! the tool's plugin knows of it, the subject, the receipt and the body,
//! as data. The host draws the verb, the square, the number, folding and
//! `+ N more`.

use serde::{Deserialize, Serialize};

use crate::tree::{Ink, KeyValue, Node, Tone};

/// How a tool's own plugin describes a call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    /// The one argument the call is about: a path, a pattern, a command,
    /// `server · tool`, a title. Never JSON.
    pub subject: Option<String>,
    /// What came back, its parts joined with ` · `.
    pub receipt: Vec<Receipt>,
    /// Under the line, once the person opens it (or always, for a tree
    /// that does not fold).
    pub body: Option<Body>,
    /// The call failed, as its result tells, though the tool returned.
    pub failed: Option<String>,
    /// The call waits on the person: it leaves the spine, under an
    /// orange rule, and shows while its stretch is folded.
    pub waits: bool,
    /// One button at the line's end: todo's undo.
    pub action: Option<Node>,
    /// What the tool does, when its plugin says it here: its verb's
    /// color.
    pub kind: Option<heiko_ui_kit::theme::ToolKind>,
    /// The call has no line: only its body shows, a note in its place,
    /// or nothing.
    pub lineless: bool,
}

/// One part of a receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "lowercase")]
pub enum Receipt {
    /// `29 lines`, `5 matches`: the noun is written in the singular.
    Count { n: u64, noun: String },
    /// `+3 −1`, or `new · +12`, with its diffstat.
    Diff {
        added: u64,
        removed: u64,
        #[serde(default)]
        new: bool,
    },
    /// `exit 100`, `worked`, `relates`, in its tone.
    Stamp {
        text: String,
        #[serde(default)]
        tone: Ink,
    },
    /// What is not a count: `4.0 KB of 18 KB`, `12 of 29 lines`.
    Measure { text: String },
}

impl Receipt {
    pub fn count(n: usize, noun: impl Into<String>) -> Self {
        Self::Count {
            n: n as u64,
            noun: noun.into(),
        }
    }

    pub fn stamp(text: impl Into<String>, tone: Ink) -> Self {
        Self::Stamp {
            text: text.into(),
            tone,
        }
    }

    pub fn measure(text: impl Into<String>) -> Self {
        Self::Measure { text: text.into() }
    }

    /// As the line writes it; a diffstat's squares aside.
    pub fn text(&self) -> String {
        match self {
            Self::Count { n, noun } => counted(*n as usize, noun),
            Self::Diff {
                added, new: true, ..
            } => format!("new · +{added}"),
            Self::Diff { added, removed, .. } => format!("+{added} −{removed}"),
            Self::Stamp { text, .. } | Self::Measure { text } => text.clone(),
        }
    }
}

/// `n noun`, the number grouped and the noun's last word in the plural
/// unless `n` is one: `1 line`, `1,204 lines`, `3 new lines`, `2
/// matches`, `4 entries`.
pub fn counted(n: usize, noun: &str) -> String {
    let n_text = heiko_ui_kit::format::grouped(n);
    if n == 1 {
        return format!("{n_text} {noun}");
    }
    format!("{n_text} {}", plural(noun))
}

/// The noun's last word in the plural, by English's regular rules.
pub fn plural(noun: &str) -> String {
    let (lead, word) = match noun.rsplit_once(' ') {
        Some((lead, word)) => (format!("{lead} "), word),
        None => (String::new(), noun),
    };
    let vowel = |c: char| "aeiou".contains(c);
    let word = if ["s", "x", "z", "ch", "sh"]
        .iter()
        .any(|end| word.ends_with(end))
    {
        format!("{word}es")
    } else if let Some(stem) = word.strip_suffix('y')
        && stem.chars().last().is_some_and(|c| !vowel(c))
    {
        format!("{stem}ies")
    } else {
        format!("{word}s")
    };
    format!("{lead}{word}")
}

/// A step's body: one of the forms, which the host draws, folds and
/// cuts short, or a tree the plugin draws itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "lowercase")]
pub enum Body {
    /// Code, its lines numbered from `first_line`, highlighted as `lang`.
    Code {
        #[serde(default)]
        lang: Option<String>,
        text: String,
        #[serde(default)]
        first_line: Option<u32>,
    },
    /// Lines a search found, each under its path, numbered.
    Matches { lines: Vec<Match> },
    /// A unified diff: its added and removed lines.
    Diff {
        text: String,
        #[serde(default)]
        path: Option<String>,
    },
    /// Names in columns, each in its tone; `noun` is what one is.
    Names {
        names: Vec<Name>,
        #[serde(default = "Body::name_noun")]
        noun: String,
    },
    /// A program's lines, as the model got them.
    Output { lines: Vec<String> },
    /// Keys and their values.
    Record { pairs: Vec<KeyValue> },
    /// A body the person works in, drawn by the plugin: it folds only
    /// when it says so; an `inset` one sits under the line and says
    /// itself what was cut.
    Tree {
        node: Node,
        #[serde(default)]
        folds: bool,
        #[serde(default)]
        inset: bool,
    },
}

/// One line a search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub path: String,
    pub number: usize,
    pub text: String,
    /// It matched; else it is context around a match, drawn quiet.
    #[serde(default)]
    pub matched: bool,
    /// Where in `text` it matched: the first and last byte, from 1.
    #[serde(default)]
    pub spans: Vec<(u32, u32)>,
}

/// One name of a [`Body::Names`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Name {
    pub text: String,
    #[serde(default)]
    pub tone: Option<Tone>,
}

impl Name {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: None,
        }
    }
}

/// How much of a form shows before the person asks for all of it.
pub mod peek {
    pub const CODE: usize = 4;
    pub const MATCHES: usize = 8;
    pub const DIFF: usize = 12;
    pub const NAMES: usize = 9;
    /// A program's last lines.
    pub const OUTPUT: usize = 6;
}

/// A body as the host draws it: the tree, and what was cut from it, to
/// say as `+ N more <noun>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawn {
    pub node: Node,
    pub rest: Option<Rest>,
}

/// What a form left out until the person asks for all of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rest {
    pub n: usize,
    pub noun: String,
}

impl Rest {
    /// `+ 12 more lines`.
    pub fn label(&self) -> String {
        format!("+ {}", counted(self.n, &format!("more {}", self.noun)))
    }
}

impl Body {
    fn name_noun() -> String {
        "name".into()
    }

    /// Whether it starts closed, its line opening it.
    pub fn folds(&self) -> bool {
        match self {
            Self::Tree { folds, .. } => *folds,
            Self::Output { .. } => false,
            _ => true,
        }
    }

    /// Whether it sits under the line and says itself what was cut.
    pub fn inset(&self) -> bool {
        matches!(self, Self::Tree { inset: true, .. })
    }

    /// The tree to draw: all of it when `whole`, else down to its peek.
    /// `compact` is a phone's width.
    pub fn draw(&self, whole: bool, compact: bool) -> Drawn {
        let cut = |len: usize, peek: usize, noun: &str| {
            let rest = len.saturating_sub(peek);
            let shown = if whole { len } else { len - rest };
            let rest = (rest > 0 && !whole).then(|| Rest {
                n: rest,
                noun: noun.to_owned(),
            });
            (shown, rest)
        };
        match self {
            Self::Code {
                lang,
                text,
                first_line,
            } => {
                let lines: Vec<&str> = text.lines().collect();
                let (shown, rest) = cut(lines.len(), peek::CODE, "line");
                Drawn {
                    node: Node::Code {
                        lang: lang.clone(),
                        text: lines[..shown].join("\n"),
                        first_line: *first_line,
                    },
                    rest,
                }
            }
            Self::Matches { lines } => {
                let (shown, rest) = cut(lines.len(), peek::MATCHES, "line");
                Drawn {
                    node: matches(&lines[..shown]),
                    rest,
                }
            }
            Self::Diff { text, path } => {
                let len = heiko_ui_kit::diff::parse(text).len();
                let (shown, rest) = cut(len, peek::DIFF, "line");
                Drawn {
                    node: Node::Diff {
                        text: if rest.is_some() {
                            diff_head(text, shown)
                        } else {
                            text.clone()
                        },
                        path: path.clone(),
                    },
                    rest,
                }
            }
            Self::Names { names, noun } => {
                let (shown, rest) = cut(names.len(), peek::NAMES, noun);
                Drawn {
                    node: columns(&names[..shown], if compact { 2 } else { 3 }),
                    rest,
                }
            }
            Self::Output { lines } => {
                // A program's last lines say how it ended.
                let (shown, rest) = cut(lines.len(), peek::OUTPUT, "line");
                Drawn {
                    node: Node::Code {
                        lang: None,
                        text: lines[lines.len() - shown..].join("\n"),
                        first_line: None,
                    },
                    rest,
                }
            }
            Self::Record { pairs } => Drawn {
                node: Node::Rows {
                    rows: pairs.clone(),
                },
                rest: None,
            },
            Self::Tree { node, .. } => Drawn {
                node: node.clone(),
                rest: None,
            },
        }
    }
}

/// A diffstat's five squares: green by the share of lines added, red by
/// the share removed.
pub fn diffstat(added: usize, removed: usize) -> Node {
    use heiko_ui_kit::diff;
    let (green, red) = diff::squares(added, removed);
    let squares = |count: usize, tone| Node::Toned {
        text: "■".repeat(count),
        tone,
        mono: true,
    };
    Node::row(
        [
            (green, Tone::Good),
            (red, Tone::Bad),
            (diff::SQUARES - green - red, Tone::Neutral),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, tone)| squares(count, tone)),
    )
}

/// A unified diff down to its first `keep` lines, its headers aside.
pub fn diff_head(diff: &str, keep: usize) -> String {
    let mut kept = Vec::new();
    let mut left = None;
    for line in diff.lines() {
        match &mut left {
            None => {
                if line.starts_with("@@") {
                    left = Some(keep);
                }
            }
            Some(0) => break,
            Some(left) => *left -= 1,
        }
        kept.push(line);
    }
    kept.join("\n")
}

/// Found lines on a band: each file's path over its lines, every line
/// after its number, what matched marked, context lines quiet.
fn matches(lines: &[Match]) -> Node {
    let width = lines
        .iter()
        .map(|line| line.number.to_string().len())
        .max()
        .unwrap_or(1);
    let mut pieces = Vec::new();
    let mut path = None;
    for line in lines {
        if path != Some(&line.path) {
            path = Some(&line.path);
            pieces.push(Node::Toned {
                text: line.path.clone(),
                tone: Tone::Neutral,
                mono: true,
            });
        }
        // The number is part of the line, so a long one is cut at the
        // band's edge beside its number, not moved under it.
        let number = format!("{:>width$}  ", line.number);
        let shift = number.len() as u32;
        let text = format!("{number}{}", line.text.replace('\t', "    "));
        pieces.push(if line.matched {
            Node::Marked {
                text,
                spans: line
                    .spans
                    .iter()
                    .map(|(first, last)| (first + shift, last + shift))
                    .collect(),
            }
        } else {
            Node::Toned {
                text,
                tone: Tone::Neutral,
                mono: true,
            }
        });
    }
    Node::Band { children: pieces }
}

/// Space between two columns of names.
const GUTTER: usize = 3;

/// Names on a band, down `count` columns.
fn columns(names: &[Name], count: usize) -> Node {
    let column = |at: usize| {
        let cells = || names.iter().skip(at).step_by(count);
        // Every column but the last is padded, so the next starts clear
        // of its longest name.
        let width = cells()
            .map(|name| name.text.chars().count())
            .max()
            .filter(|_| at + 1 < count)
            .map_or(0, |widest| widest + GUTTER);
        Node::stack(cells().map(|name| {
            let text = format!("{:<width$}", name.text);
            match name.tone {
                Some(tone) => Node::Toned {
                    text,
                    tone,
                    mono: true,
                },
                None => Node::Mono { text },
            }
        }))
    };
    Node::Band {
        children: vec![Node::row((0..count.min(names.len())).map(column))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last word takes the plural, by the regular rules.
    #[test]
    fn a_count_writes_its_noun_in_the_plural() {
        assert_eq!(counted(1, "line"), "1 line");
        assert_eq!(counted(0, "line"), "0 lines");
        assert_eq!(counted(1204, "line"), "1,204 lines");
        assert_eq!(counted(2, "match"), "2 matches");
        assert_eq!(counted(4, "entry"), "4 entries");
        assert_eq!(counted(2, "key"), "2 keys");
        assert_eq!(counted(3, "new line"), "3 new lines");
        assert_eq!(counted(2, "more line"), "2 more lines");
    }

    /// A diff receipt says a new file as new.
    #[test]
    fn a_diff_receipt_says_what_changed() {
        let diff = |added, removed, new| Receipt::Diff {
            added,
            removed,
            new,
        };
        assert_eq!(diff(3, 1, false).text(), "+3 −1");
        assert_eq!(diff(12, 0, true).text(), "new · +12");
    }

    /// A form longer than its peek shows its peek and says the rest, and
    /// all of it once whole; output keeps its last lines.
    #[test]
    fn a_form_shows_its_peek_until_whole() {
        let text: Vec<String> = (1..=10).map(|n| format!("l{n}")).collect();
        let code = Body::Code {
            lang: None,
            text: text.join("\n"),
            first_line: Some(1),
        };
        let drawn = code.draw(false, false);
        assert_eq!(
            drawn.rest,
            Some(Rest {
                n: 6,
                noun: "line".into()
            })
        );
        assert_eq!(drawn.rest.as_ref().unwrap().label(), "+ 6 more lines");
        assert!(matches!(drawn.node, Node::Code { ref text, .. }
            if text == "l1\nl2\nl3\nl4"));
        assert_eq!(code.draw(true, false).rest, None);

        let output = Body::Output { lines: text };
        let Node::Code { text, .. } = output.draw(false, false).node else {
            panic!("code");
        };
        assert_eq!(text, "l5\nl6\nl7\nl8\nl9\nl10");
    }

    /// The texts of each column of drawn names, and what follows them.
    fn columns(body: &Node) -> Vec<Vec<String>> {
        let Node::Band { children } = body else {
            panic!("a band: {body:?}");
        };
        let Node::Row { children: columns } = &children[0] else {
            panic!("a row of columns: {:?}", children[0]);
        };
        columns
            .iter()
            .map(|column| match column {
                Node::Stack { children } => children
                    .iter()
                    .map(|cell| match cell {
                        Node::Mono { text } | Node::Toned { text, .. } => {
                            text.clone()
                        }
                        other => panic!("a name: {other:?}"),
                    })
                    .collect(),
                other => panic!("a column: {other:?}"),
            })
            .collect()
    }

    /// Names read across three columns, two on a phone; a column's names
    /// are as wide as each other, so the next column lines up, and the
    /// last is not padded. Past the peek, the rest is counted.
    #[test]
    fn names_read_across_plain_columns() {
        let names = |n: usize| Body::Names {
            names: std::iter::once(Name::plain("src/"))
                .chain((0..n).map(|n| Name::plain(format!("file-{n}.rs"))))
                .collect(),
            noun: "entry".into(),
        };
        let wide = columns(&names(4).draw(false, false).node);
        assert_eq!(wide.len(), 3);
        assert_eq!(wide[0].len(), 2);
        assert_eq!(wide[0][0].trim_end(), "src/");
        assert_eq!(wide[0][1].trim_end(), "file-2.rs");
        assert_eq!(wide[1], ["file-0.rs   ", "file-3.rs   "]);
        assert_eq!(wide[0][0].len(), wide[0][1].len());
        assert_eq!(wide[2], ["file-1.rs"]);
        assert_eq!(columns(&names(4).draw(false, true).node).len(), 2);
        let long = names(peek::NAMES + 2).draw(false, false);
        let shown: usize = columns(&long.node).iter().map(Vec::len).sum();
        assert_eq!(shown, peek::NAMES);
        assert_eq!(long.rest.unwrap().label(), "+ 3 more entries");
    }

    /// A diffstat always has five squares, however the lines split.
    #[test]
    fn a_diffstat_has_five_squares() {
        for (added, removed) in [(0, 0), (3, 1), (38, 0), (0, 2), (1, 900)] {
            let Node::Row { children } = diffstat(added, removed) else {
                panic!("a row");
            };
            let squares: usize = children
                .iter()
                .map(|node| match node {
                    Node::Toned { text, .. } => text.chars().count(),
                    other => panic!("squares: {other:?}"),
                })
                .sum();
            assert_eq!(squares, heiko_ui_kit::diff::SQUARES);
        }
    }

    /// A step goes over the wire and comes back the same.
    #[test]
    fn a_step_reads_back() {
        let step = Step {
            subject: Some("src/a.rs".into()),
            receipt: vec![
                Receipt::count(2, "match"),
                Receipt::Diff {
                    added: 1,
                    removed: 2,
                    new: false,
                },
                Receipt::stamp("exit 1", Ink::Bad),
                Receipt::measure("4.0 KB of 18 KB"),
            ],
            body: Some(Body::Names {
                names: vec![
                    Name::plain("a"),
                    Name {
                        text: "b/".into(),
                        tone: Some(Tone::Info),
                    },
                ],
                noun: "entry".into(),
            }),
            failed: None,
            waits: true,
            action: None,
            kind: Some(heiko_ui_kit::theme::ToolKind::Search),
            lineless: false,
        };
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(serde_json::from_value::<Step>(json).unwrap(), step);
    }
}
