//! Finding notes for a query: the index for seed notes, then one hop
//! along links, inside one budget (`docs/research/memory.md`,
//! "Reading").
//!
//! Search finds entry points; links carry what search misses, such as a
//! decision that names neither of a query's words. Superseded notes rank
//! lower and are labelled, never hidden: history is often the answer.

use crate::{
    index::Index,
    note::{LinkType, NoteType},
    store::Notes,
};

/// How much a superseded note's score counts against a current one's.
pub const SUPERSEDED_WEIGHT: f32 = 0.5;

/// `score` with [`SUPERSEDED_WEIGHT`] against it: scaled toward zero
/// when positive and away from it when negative, so it always falls.
/// MaxSim's scores can be negative, the dot products of unit vectors.
fn weigh_down(score: f32) -> f32 {
    if score >= 0.0 {
        score * SUPERSEDED_WEIGHT
    } else {
        score / SUPERSEDED_WEIGHT
    }
}

/// A note found for a query.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub kind: NoteType,
    pub description: String,
    /// The body line that best matches the query, cut short.
    pub snippet: String,
    pub superseded: bool,
    /// Why the note may be stale, when it may.
    pub stale: Option<String>,
    /// How a note that search did not find was reached: the seed it is
    /// linked with, and how. `None` for a note search found.
    pub via: Option<(String, LinkType)>,
}

/// Up to `budget` notes for `query`: the best search hits (superseded
/// ones weighed down), then the notes linked with them, one hop, in the
/// seeds' order. A third of the budget is kept for linked notes; seeds
/// fill whatever links leave free.
pub fn recall(
    notes: &Notes,
    index: &dyn Index,
    query: &str,
    budget: usize,
) -> anyhow::Result<Vec<Hit>> {
    recall_with(notes, index, query, budget, true)
}

/// [`recall`], with the hop along links when `hops` is set, and without
/// it (search alone, superseded notes weighed down) when not: the
/// evaluation measures what the hop adds.
pub fn recall_with(
    notes: &Notes,
    index: &dyn Index,
    query: &str,
    budget: usize,
    hops: bool,
) -> anyhow::Result<Vec<Hit>> {
    if budget == 0 {
        return Ok(Vec::new());
    }
    let mut seeds: Vec<(String, f32)> = index
        .search(query, budget * 3)?
        .into_iter()
        .filter_map(|(id, score)| {
            // The index can lag the files; a note that is gone is skipped.
            let note = notes.get(&id)?;
            let score = if note.is_superseded() {
                weigh_down(score)
            } else {
                score
            };
            Some((id, score))
        })
        .collect();
    seeds.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let reserve = if hops { budget / 3 } else { 0 };
    let first = seeds.len().min(budget - reserve);
    let mut picked: Vec<(String, Option<(String, LinkType)>)> = seeds[..first]
        .iter()
        .map(|(id, _)| (id.clone(), None))
        .collect();
    let taken = |picked: &[(String, Option<(String, LinkType)>)], id: &str| {
        picked.iter().any(|(seen, _)| seen == id)
    };
    let hop_from = if hops { &seeds[..first] } else { &[][..] };
    'hop: for seed in hop_from.iter().map(|(id, _)| id) {
        for (other, kind) in linked(notes, seed) {
            if picked.len() >= budget {
                break 'hop;
            }
            if !taken(&picked, &other) {
                picked.push((other, Some((seed.clone(), kind))));
            }
        }
    }
    for (id, _) in &seeds[first..] {
        if picked.len() >= budget {
            break;
        }
        if !taken(&picked, id) {
            picked.push((id.clone(), None));
        }
    }

    Ok(picked
        .into_iter()
        .filter_map(|(id, via)| {
            let note = notes.get(&id)?;
            Some(Hit {
                snippet: snippet(&note.body, query)
                    .unwrap_or_else(|| note.description.clone()),
                id,
                title: note.title.clone(),
                kind: note.kind,
                description: note.description.clone(),
                superseded: note.is_superseded(),
                stale: note.stale.clone(),
                via,
            })
        })
        .collect())
}

/// The notes linked with `id` either way, each once with the first link
/// found: its own links first, then links into it. Paths (`about`) and
/// notes not written yet are skipped.
pub fn linked(notes: &Notes, id: &str) -> Vec<(String, LinkType)> {
    let mut found: Vec<(String, LinkType)> = Vec::new();
    let mut add = |other: String, kind: LinkType| {
        if other != id
            && kind != LinkType::About
            && notes.get(&other).is_some()
            && !found.iter().any(|(seen, _)| *seen == other)
        {
            found.push((other, kind));
        }
    };
    if let Some(note) = notes.get(id) {
        for link in note.all_links() {
            add(link.to, link.kind);
        }
    }
    for (from, link) in notes.backlinks(id) {
        add(from, link.kind);
    }
    found
}

const SNIPPET: usize = 200;

/// The body line holding most of the query's words, cut to about 200
/// characters; `None` when no line holds any.
fn snippet(body: &str, query: &str) -> Option<String> {
    let terms: Vec<String> = crate::index::words(query).collect();
    let (line, count) = body
        .lines()
        .map(|line| {
            let held = crate::index::words(line)
                .filter(|word| terms.contains(word))
                .count();
            (line, held)
        })
        .max_by_key(|(_, held)| *held)?;
    if count == 0 {
        return None;
    }
    let line = line.trim();
    Some(match line.char_indices().nth(SNIPPET) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    })
}
