//! What the checks did: in one run, from its reports and records, and
//! over a repository's runs, for its page.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Check, Verdict, VerdictKind};

/// What the checks did in one run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    /// Tool calls checked, and final answers checked.
    pub calls: u32,
    pub answers: u32,
    /// Questions asked of Jev: one per rule per check.
    pub questions: u32,
    /// Checks Jev could not answer.
    pub failed: u32,
    /// What Jev cost, in US dollars.
    pub cost: f64,
    /// The rules behind each block, flag and hold, in order.
    pub blocked: Vec<String>,
    pub flagged: Vec<String>,
    pub held: Vec<String>,
    /// How many holds a run may have.
    pub max_holds: Option<u32>,
    /// Final answers that stood but were flagged for a person, with the
    /// rule and its score.
    pub flagged_answers: Vec<FlaggedAnswer>,
}

/// A final answer flagged for review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlaggedAnswer {
    pub rule: String,
    pub text: String,
    pub score: f64,
    pub answer: String,
}

impl Stats {
    /// Counts one of the plugin's report or record bodies: a check, a
    /// verdict or a failure. `answer` is the run's last text, which a
    /// flagged final answer is about.
    pub fn add(&mut self, body: &Value, answer: Option<String>) {
        if body["kind"] == "error" {
            self.failed += 1;
        } else if let Some(check) = Check::parse(body) {
            match check.call_id {
                Some(_) => self.calls += 1,
                None => self.answers += 1,
            }
            self.questions += check.scores.len() as u32;
            self.cost += check.cost;
        } else if let Some(verdict) = Verdict::parse(body) {
            match verdict.kind {
                VerdictKind::Blocked => self.blocked.push(verdict.rule),
                VerdictKind::Flagged => {
                    if verdict.call_id.is_none() {
                        self.flagged_answers.push(FlaggedAnswer {
                            rule: verdict.rule.clone(),
                            text: verdict.text.clone(),
                            score: verdict.score,
                            answer: answer.unwrap_or_default(),
                        });
                    }
                    self.flagged.push(verdict.rule)
                }
                VerdictKind::Held => {
                    self.held.push(verdict.rule);
                    self.max_holds = verdict.max_holds.or(self.max_holds);
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.calls == 0 && self.answers == 0
    }

    /// The plugin's state in a line: `1 blocked · 1 flagged`.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            (self.blocked.len(), "blocked"),
            (self.flagged.len(), "flagged"),
            (self.held.len(), "held"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, what)| format!("{count} {what}"))
        .collect();
        if parts.is_empty() {
            let checks = self.calls + self.answers;
            format!(
                "{checks} {}, all clear",
                if checks == 1 { "check" } else { "checks" }
            )
        } else {
            parts.join(" · ")
        }
    }
}

/// What a repository's runs say about its constitution.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RulesStats {
    pub runs: usize,
    /// Calls and final answers checked.
    pub checked: u32,
    /// Checks Jev could not answer.
    pub failed: u32,
    pub blocked: usize,
    pub flagged: usize,
    /// Flagged calls and answers nobody has looked at.
    pub waiting: usize,
    /// Runs whose answer was sent back at least once.
    pub held_runs: usize,
    pub cost: f64,
    /// Per rule: blocked, flagged, held.
    pub per_rule: HashMap<String, (usize, usize, usize)>,
}

impl RulesStats {
    /// What the checks did over a repository's runs: `loaded`, each run
    /// the interface has with what it saw, and `history`, what the store
    /// says of each checked run. A loaded run counts as loaded, which is
    /// as current as it gets; a stored one only when it is not loaded.
    /// The review queue (`waiting`) is the caller's to count.
    pub fn of(loaded: &[(&str, &Stats)], history: &[(String, Stats)]) -> Self {
        let mut stats = RulesStats::default();
        let stored = history
            .iter()
            .filter(|(run, _)| !loaded.iter().any(|(id, _)| id == run))
            .map(|(_, checks)| checks);
        for checks in loaded.iter().map(|(_, checks)| *checks).chain(stored) {
            stats.runs += 1;
            stats.checked += checks.calls + checks.answers;
            stats.failed += checks.failed;
            stats.blocked += checks.blocked.len();
            stats.flagged += checks.flagged.len();
            stats.cost += checks.cost;
            if !checks.held.is_empty() {
                stats.held_runs += 1;
            }
            for (list, slot) in [
                (&checks.blocked, 0),
                (&checks.flagged, 1),
                (&checks.held, 2),
            ] {
                for rule in list {
                    let counts =
                        stats.per_rule.entry(rule.clone()).or_default();
                    match slot {
                        0 => counts.0 += 1,
                        1 => counts.1 += 1,
                        _ => counts.2 += 1,
                    }
                }
            }
        }
        stats
    }
}
