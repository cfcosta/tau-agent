//! The retrieval evaluation (`docs/research/memory.md`, "Evaluation
//! first"): how often search finds the note that answers a query, by how
//! far the query's words are from the note's, for BM25, ColBERT and the
//! two fused, as near-duplicates of each answer pile up.
//!
//! The corpus is `eval/harbor.toml`: facts about a made-up service, each
//! a template whose near-duplicates share its wording with another
//! subject and value, the first of them the older version it supersedes.
//! At each level `k` of [`LEVELS`], every fact has `k` near-duplicates,
//! and background notes that answer nothing fill the level to the same
//! size as every other.

use std::{collections::HashMap, fmt, path::Path};

use anyhow::{Context as _, anyhow};
use serde::{Deserialize, Serialize};

use crate::{
    Memory,
    colbert::{Colbert, Encoder},
    index::{Bm25, Index, index_text},
    memory::Draft,
    note::{By, LinkType, NoteType, Source},
    recall::recall_with,
    store::Notes,
};

/// The corpus.
pub const FIXTURE: &str = include_str!("../eval/harbor.toml");

/// How many near-duplicates each answer has, level by level.
pub const LEVELS: [usize; 6] = [0, 1, 2, 4, 8, 16];

/// How many notes a query gets back: the deepest cutoff measured.
pub const DEPTH: usize = 10;

#[derive(Deserialize)]
struct File {
    fact: Vec<Fact>,
    background: Vec<Background>,
}

/// A template of notes that answer no query.
#[derive(Debug, Clone, Deserialize)]
pub struct Background {
    pub kind: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub subjects: Vec<String>,
    pub values: Vec<String>,
}

/// The corpus: its facts and its background.
pub struct Fixture {
    pub facts: Vec<Fact>,
    pub background: Vec<Background>,
}

/// One fact of the corpus: see `eval/harbor.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Fact {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub subject: String,
    pub subjects: Vec<String>,
    pub value: String,
    pub values: Vec<String>,
    pub old_value: String,
    pub link: Option<String>,
    pub why: Option<String>,
    pub verbatim: String,
    pub paraphrase: String,
    pub indirect: Option<String>,
}

pub fn fixture() -> anyhow::Result<Fixture> {
    let file = toml::from_str::<File>(FIXTURE).context("eval/harbor.toml")?;
    Ok(Fixture {
        facts: file.fact,
        background: file.background,
    })
}

/// How many notes every level holds for each fact: the answer and as
/// many others as the deepest level's near-duplicates.
pub fn per_fact() -> usize {
    1 + LEVELS[LEVELS.len() - 1]
}

/// `template` over `subject` and `value`.
fn fill(template: &str, subject: &str, value: &str) -> String {
    template
        .replace("{subject_id}", &identifier(subject))
        .replace("{subject}", subject)
        .replace("{value}", value)
}

/// The note a template makes of `subject` and `value`.
fn draft(
    kind: &str,
    [title, description, body]: [&str; 3],
    subject: &str,
    value: &str,
) -> anyhow::Result<Draft> {
    let kind = NoteType::parse(kind)
        .ok_or_else(|| anyhow!("no note type {kind:?}"))?;
    Ok(Draft {
        kind,
        title: fill(title, subject, value),
        description: fill(description, subject, value),
        body: fill(body, subject, value),
        tags: Vec::new(),
        links: Vec::new(),
        id: None,
        supersedes: None,
        source: Source {
            by: By::Agent,
            run: None,
            turn: None,
            commit: None,
            files: Vec::new(),
        },
    })
}

/// Pairs of `subjects` and `values`, each once, both changing from one
/// to the next.
fn pairs(
    subjects: &[String],
    values: &[String],
) -> impl Iterator<Item = (String, String)> {
    let (s, v) = (subjects.len(), values.len());
    (0..s * v).map(move |i| {
        (subjects[i % s].clone(), values[(i + i / s) % v].clone())
    })
}

impl Fixture {
    /// The first `n` background notes, taking a pair from each template
    /// in turn.
    pub fn background(&self, n: usize) -> anyhow::Result<Vec<Draft>> {
        let mut each: Vec<_> = self
            .background
            .iter()
            .map(|template| {
                (template, pairs(&template.subjects, &template.values))
            })
            .collect();
        let mut drafts = Vec::new();
        while drafts.len() < n {
            let before = drafts.len();
            for (template, pairs) in &mut each {
                if drafts.len() == n {
                    break;
                }
                if let Some((subject, value)) = pairs.next() {
                    drafts.push(draft(
                        &template.kind,
                        [
                            &template.title,
                            &template.description,
                            &template.body,
                        ],
                        &subject,
                        &value,
                    )?);
                }
            }
            if drafts.len() == before {
                anyhow::bail!("the background has fewer than {n} notes");
            }
        }
        Ok(drafts)
    }
}

/// `subject` as an identifier: `webhook worker` is `webhook_worker`.
pub fn identifier(subject: &str) -> String {
    subject
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

impl Fact {
    /// The note this fact's template makes of `subject` and `value`.
    pub fn draft(&self, subject: &str, value: &str) -> anyhow::Result<Draft> {
        draft(
            &self.kind,
            [&self.title, &self.description, &self.body],
            subject,
            value,
        )
        .with_context(|| self.id.clone())
    }

    /// The subjects and values of this fact's `k` near-duplicates: its
    /// own subject with its older value first, then other subjects with
    /// other values, each pair once, both changing from one to the next.
    pub fn near_duplicates(&self, k: usize) -> Vec<(String, String)> {
        std::iter::once((self.subject.clone(), self.old_value.clone()))
            .chain(pairs(&self.subjects, &self.values))
            .take(k)
            .collect()
    }
}

/// How far a query's words are from its answer's.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Distance {
    /// Shares identifiers, paths or phrases with the answer.
    Verbatim,
    /// The same meaning in other words.
    Paraphrase,
    /// Answered by the note the matching one links to.
    Indirect,
}

impl Distance {
    pub const ALL: [Self; 3] =
        [Self::Verbatim, Self::Paraphrase, Self::Indirect];
}

impl fmt::Display for Distance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Verbatim => "verbatim",
            Self::Paraphrase => "paraphrase",
            Self::Indirect => "indirect",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub fact: String,
    pub distance: Distance,
    pub text: String,
    /// The note that answers it.
    pub answer: String,
    /// The older version the answer superseded, at levels that have it.
    pub older: Option<String>,
}

/// One level's notes, and the queries over them.
pub struct Corpus {
    pub notes: Notes,
    pub queries: Vec<Query>,
}

/// Writes the corpus at level `k` into `dir`, which should be empty:
/// [`per_fact`] notes for each fact, `k` of them near-duplicates of its
/// answer and the rest background.
pub fn corpus(
    dir: &Path,
    fixture: &Fixture,
    k: usize,
) -> anyhow::Result<Corpus> {
    let facts = &fixture.facts;
    let mut memory = Memory::open(dir, Box::new(Bm25::new()))?;
    let mut clock = 0;
    let mut write = |memory: &mut Memory, draft: Draft| {
        clock += 1;
        let title = draft.title.clone();
        memory
            .write(draft, clock)
            .map(|written| written.id)
            .map_err(|error| anyhow!("{title:?}: {error}"))
    };
    let mut answers = HashMap::new();
    let mut olders = HashMap::new();
    for fact in facts {
        let mut near = fact.near_duplicates(k).into_iter();
        let older = match near.next() {
            Some((subject, value)) => {
                Some(write(&mut memory, fact.draft(&subject, &value)?)?)
            }
            None => None,
        };
        let mut draft = fact.draft(&fact.subject, &fact.value)?;
        draft.supersedes = older.clone();
        answers.insert(fact.id.clone(), write(&mut memory, draft)?);
        if let Some(older) = older {
            olders.insert(fact.id.clone(), older);
        }
        for (subject, value) in near {
            write(&mut memory, fact.draft(&subject, &value)?)?;
        }
    }
    let filler = facts.len() * (per_fact() - 1 - k.min(per_fact() - 1));
    for draft in fixture.background(filler)? {
        write(&mut memory, draft)?;
    }
    let answer = |id: &str| {
        answers
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("no fact is called {id:?}"))
    };
    let mut queries = Vec::new();
    for fact in facts {
        let own = answer(&fact.id)?;
        for (distance, text) in [
            (Distance::Verbatim, &fact.verbatim),
            (Distance::Paraphrase, &fact.paraphrase),
        ] {
            queries.push(Query {
                fact: fact.id.clone(),
                distance,
                text: text.clone(),
                answer: own.clone(),
                older: olders.get(&fact.id).cloned(),
            });
        }
        if let Some(to) = &fact.link {
            memory
                .link(
                    &own,
                    &answer(to)?,
                    LinkType::Relates,
                    fact.why.clone(),
                    clock + 1,
                )
                .map_err(|error| anyhow!("{}: {error}", fact.id))?;
            if let Some(text) = &fact.indirect {
                queries.push(Query {
                    fact: fact.id.clone(),
                    distance: Distance::Indirect,
                    text: text.clone(),
                    answer: answer(to)?,
                    older: olders.get(to).cloned(),
                });
            }
        }
    }
    Ok(Corpus {
        notes: Notes::open(dir)?,
        queries,
    })
}

/// Puts every note of `notes` into `index`, as a scope's open does.
pub fn index_all(index: &mut dyn Index, notes: &Notes) -> anyhow::Result<()> {
    for note in notes.iter() {
        index.upsert(&note.id, &index_text(note))?;
    }
    Ok(())
}

/// How one query went.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    pub distance: Distance,
    /// Where the answer came, from 1; `None` past [`DEPTH`].
    pub rank: Option<usize>,
    /// Whether the superseded older version came before the answer,
    /// when there is one.
    pub older_first: Option<bool>,
}

/// Runs every query of `corpus` against `index`, as recall does for the
/// agent: with the hop along links when `hops` is set.
pub fn evaluate(
    corpus: &Corpus,
    index: &dyn Index,
    hops: bool,
) -> anyhow::Result<Vec<Outcome>> {
    corpus
        .queries
        .iter()
        .map(|query| {
            let hits =
                recall_with(&corpus.notes, index, &query.text, DEPTH, hops)?;
            let at = |id: &str| hits.iter().position(|hit| hit.id == id);
            let answer = at(&query.answer);
            Ok(Outcome {
                distance: query.distance,
                rank: answer.map(|at| at + 1),
                older_first: query.older.as_deref().map(|older| {
                    match (at(older), answer) {
                        (Some(older), Some(answer)) => older < answer,
                        (Some(_), None) => true,
                        (None, _) => false,
                    }
                }),
            })
        })
        .collect()
}

/// The share of `outcomes` whose answer came within the first `cutoff`.
pub fn recall_at(outcomes: &[Outcome], cutoff: usize) -> f32 {
    if outcomes.is_empty() {
        return 0.0;
    }
    let found = outcomes
        .iter()
        .filter(|outcome| outcome.rank.is_some_and(|rank| rank <= cutoff))
        .count();
    found as f32 / outcomes.len() as f32
}

/// Mean reciprocal rank, an answer past [`DEPTH`] counting 0.
pub fn mrr(outcomes: &[Outcome]) -> f32 {
    if outcomes.is_empty() {
        return 0.0;
    }
    let sum: f32 = outcomes
        .iter()
        .filter_map(|outcome| outcome.rank)
        .map(|rank| 1.0 / rank as f32)
        .sum();
    sum / outcomes.len() as f32
}

/// One line of the report: an index at a level, over one distance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub index: String,
    pub hops: bool,
    pub k: usize,
    pub distance: Distance,
    pub queries: usize,
    pub recall_5: f32,
    pub recall_10: f32,
    pub mrr: f32,
    /// The share of queries with an older version where it came first.
    pub older_first: f32,
}

pub fn rows(
    index: &str,
    hops: bool,
    k: usize,
    outcomes: &[Outcome],
) -> Vec<Row> {
    Distance::ALL
        .into_iter()
        .map(|distance| {
            let these: Vec<Outcome> = outcomes
                .iter()
                .filter(|outcome| outcome.distance == distance)
                .copied()
                .collect();
            let older: Vec<bool> = these
                .iter()
                .filter_map(|outcome| outcome.older_first)
                .collect();
            Row {
                index: index.to_owned(),
                hops,
                k,
                distance,
                queries: these.len(),
                recall_5: recall_at(&these, 5),
                recall_10: recall_at(&these, 10),
                mrr: mrr(&these),
                older_first: if older.is_empty() {
                    0.0
                } else {
                    older.iter().filter(|first| **first).count() as f32
                        / older.len() as f32
                },
            }
        })
        .collect()
}

/// An index to evaluate, by name, made fresh for each level.
pub struct Leg<'a> {
    pub name: &'a str,
    pub make: Box<dyn Fn() -> Box<dyn Index> + 'a>,
}

impl<'a> Leg<'a> {
    pub fn new(name: &'a str, make: impl Fn() -> Box<dyn Index> + 'a) -> Self {
        Self {
            name,
            make: Box::new(make),
        }
    }
}

/// ColBERT alone, without BM25: [`Colbert::semantic`] as an index.
pub struct Semantic<E: Encoder>(pub Colbert<E>);

impl<E: Encoder> Index for Semantic<E> {
    fn upsert(&mut self, id: &str, text: &str) -> anyhow::Result<()> {
        self.0.upsert(id, text)
    }

    fn remove(&mut self, id: &str) -> anyhow::Result<()> {
        self.0.remove(id)
    }

    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(String, f32)>> {
        self.0.semantic(query, limit)
    }
}

/// Every leg at every level, with and without the hop, in `work`, a
/// directory the run may fill and empty. `progress` hears of each level.
pub fn run(
    fixture: &Fixture,
    legs: &[Leg<'_>],
    work: &Path,
    mut progress: impl FnMut(usize),
) -> anyhow::Result<Vec<Row>> {
    let mut report = Vec::new();
    for k in LEVELS {
        progress(k);
        let dir = work.join(format!("k{k}"));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        let corpus = corpus(&dir, fixture, k)?;
        for leg in legs {
            let mut index = (leg.make)();
            index_all(index.as_mut(), &corpus.notes)?;
            for hops in [false, true] {
                let outcomes = evaluate(&corpus, index.as_ref(), hops)?;
                report.extend(rows(leg.name, hops, k, &outcomes));
            }
        }
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(report)
}

/// The report as text: a table per distance, levels down, each index's
/// recall at 5 and 10 across, then how often an older version came
/// before its replacement.
pub fn table(report: &[Row]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for row in report {
        if !names.contains(&row.index.as_str()) {
            names.push(&row.index);
        }
    }
    let mut out = String::new();
    for hops in [false, true] {
        for distance in Distance::ALL {
            let these: Vec<&Row> = report
                .iter()
                .filter(|row| row.hops == hops && row.distance == distance)
                .collect();
            let Some(first) = these.first() else { continue };
            out.push_str(&format!(
                "\n{distance} queries ({}), {}\n",
                first.queries,
                if hops {
                    "with the hop along links"
                } else {
                    "search alone"
                }
            ));
            out.push_str(&format!("{:>4}", "k"));
            for name in &names {
                out.push_str(&format!(" | {name:>16}"));
            }
            out.push_str(&format!(" | {:>12}\n", "older first"));
            for k in LEVELS {
                out.push_str(&format!("{k:>4}"));
                let mut older = Vec::new();
                for name in &names {
                    match these
                        .iter()
                        .find(|row| row.k == k && row.index == *name)
                    {
                        Some(row) => {
                            out.push_str(&format!(
                                " | {:>7.2} {:>8.2}",
                                row.recall_5, row.recall_10
                            ));
                            older.push(format!("{:.2}", row.older_first));
                        }
                        None => out.push_str(&format!(" | {:>16}", "")),
                    }
                }
                out.push_str(&format!(" | {:>12}\n", older.join("/")));
            }
        }
    }
    out.push_str("\nEach cell: recall at 5, recall at 10.\n");
    out
}
