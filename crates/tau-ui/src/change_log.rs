//! What a `vcs_log` result shows: the stack, the changes the agent can
//! still rewrite, over the trunk, the ones it cannot. Each part is in
//! runs of one commit scope, read from the first line's conventional
//! commit prefix (`feat(tau-ui): …`).

use serde::Deserialize;
use serde_json::Value;
use tau_agent::tool::TypedTool as _;
use tau_vcs::{ChangeInfo, tools::Log};

/// The tool whose results read as a [`ChangeLog`].
pub const TOOL: &str = Log::NAME;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeLog {
    /// The working copy (`@`), pinned above the stack.
    pub working_copy: Option<Change>,
    /// The mutable changes under `@`, newest first.
    pub stack: Vec<ScopeRun>,
    /// The immutable changes, newest first.
    pub trunk: Vec<ScopeRun>,
    /// Older changes exist past the call's limit.
    pub more: bool,
}

/// Changes next to each other in the log that share a scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeRun {
    /// The commit scope, else its type (`docs: …`), else none.
    pub scope: Option<String>,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub info: ChangeInfo,
    /// The conventional commit type (`feat`), when the first line has
    /// one.
    pub kind: Option<String>,
    /// The commit scope, else its type (`docs: …`), else none.
    pub scope: Option<String>,
    /// The first line without its prefix. Empty when the change has no
    /// description.
    pub subject: String,
}

#[derive(Deserialize)]
struct Details {
    changes: Vec<ChangeInfo>,
    more: bool,
}

impl ChangeLog {
    /// Reads a `vcs_log` result's details. `None` when they are not
    /// that shape.
    pub fn parse(details: &Value) -> Option<Self> {
        let details = Details::deserialize(details).ok()?;
        let mut log = Self {
            working_copy: None,
            stack: Vec::new(),
            trunk: Vec::new(),
            more: details.more,
        };
        for info in details.changes {
            let change = Change::new(info);
            if change.info.working_copy {
                log.working_copy = Some(change);
                continue;
            }
            let runs = if change.info.immutable {
                &mut log.trunk
            } else {
                &mut log.stack
            };
            match runs.last_mut() {
                Some(run) if run.scope == change.scope => {
                    run.changes.push(change)
                }
                _ => runs.push(ScopeRun {
                    scope: change.scope.clone(),
                    changes: vec![change],
                }),
            }
        }
        Some(log)
    }

    /// Changes on the stack, the working copy included.
    pub fn stack_len(&self) -> usize {
        usize::from(self.working_copy.is_some()) + count(&self.stack)
    }

    pub fn trunk_len(&self) -> usize {
        count(&self.trunk)
    }

    /// The card's one-line account: `7 on the stack · 3 on trunk`.
    pub fn summary(&self) -> String {
        match (self.stack_len(), self.trunk_len()) {
            (0, 0) => "No changes yet".into(),
            (stack, 0) => format!("{stack} on the stack"),
            (0, trunk) => format!("{trunk} on trunk"),
            (stack, trunk) => {
                format!("{stack} on the stack · {trunk} on trunk")
            }
        }
    }

    /// Every change, newest first.
    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.working_copy.iter().chain(
            self.stack
                .iter()
                .chain(&self.trunk)
                .flat_map(|run| &run.changes),
        )
    }

    /// The change with this full change id.
    pub fn change(&self, change_id: &str) -> Option<&Change> {
        self.changes()
            .find(|change| change.info.change_id == change_id)
    }
}

impl Change {
    /// Reads the first line's conventional commit prefix.
    pub fn new(info: ChangeInfo) -> Self {
        let (kind, scope, subject) = conventional(&info.description);
        Self {
            info,
            kind,
            scope,
            subject,
        }
    }

    /// The first eight letters of the change id, enough to pass back.
    pub fn short_id(&self) -> &str {
        let id = &self.info.change_id;
        &id[..id.len().min(8)]
    }
}

fn count(runs: &[ScopeRun]) -> usize {
    runs.iter().map(|run| run.changes.len()).sum()
}

/// The type, scope and subject of a first line such as
/// `feat(tau-ui)!: a subject`. A line without the prefix is all
/// subject; a prefix without a scope takes its type as the scope.
fn conventional(description: &str) -> (Option<String>, Option<String>, String) {
    let line = description.lines().next().unwrap_or_default().trim();
    let parsed = line.split_once(": ").and_then(|(prefix, subject)| {
        let prefix = prefix.strip_suffix('!').unwrap_or(prefix);
        let (kind, scope) = match prefix.split_once('(') {
            Some((kind, rest)) => (kind, Some(rest.strip_suffix(')')?)),
            None => (prefix, None),
        };
        let word = |text: &str| {
            !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric())
        };
        let scoped = |text: &str| {
            !text.is_empty()
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
        };
        (word(kind) && scope.is_none_or(scoped)).then(|| {
            let scope = scope.unwrap_or(kind);
            (kind.to_owned(), scope.to_owned(), subject.trim().to_owned())
        })
    });
    match parsed {
        Some((kind, scope, subject)) => (Some(kind), Some(scope), subject),
        None => (None, None, line.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn info(id: &str, description: &str, flags: &[&str]) -> Value {
        json!({
            "change_id": id,
            "commit_id": format!("{id}-commit"),
            "description": description,
            "empty": flags.contains(&"empty"),
            "conflict": false,
            "immutable": flags.contains(&"immutable"),
            "working_copy": flags.contains(&"@"),
        })
    }

    fn scopes(runs: &[ScopeRun]) -> Vec<(Option<&str>, usize)> {
        runs.iter()
            .map(|run| (run.scope.as_deref(), run.changes.len()))
            .collect()
    }

    #[test]
    fn splits_the_stack_from_trunk_in_runs_of_one_scope() {
        let log = ChangeLog::parse(&json!({
            "changes": [
                info("w", "", &["@", "empty"]),
                info("a", "feat(tau-memory): tools\n\nBody.\n", &[]),
                info("b", "fix(tau-memory): writes", &[]),
                info("c", "feat(tau-agent): rewrites", &[]),
                info("d", "feat(tau-agent): plugins", &["immutable"]),
                info("e", "docs: design", &["immutable"]),
                info("f", "Merge the thing", &["immutable"]),
            ],
            "more": true,
        }))
        .expect("a log");
        assert_eq!(log.working_copy.as_ref().unwrap().info.change_id, "w");
        assert_eq!(
            scopes(&log.stack),
            [(Some("tau-memory"), 2), (Some("tau-agent"), 1)]
        );
        // tau-agent on trunk is its own run: the boundary splits it.
        assert_eq!(
            scopes(&log.trunk),
            [(Some("tau-agent"), 1), (Some("docs"), 1), (None, 1)]
        );
        let first = &log.stack[0].changes[0];
        assert_eq!(first.kind.as_deref(), Some("feat"));
        assert_eq!(first.subject, "tools");
        assert_eq!(log.trunk[2].changes[0].subject, "Merge the thing");
        assert!(log.more);
        assert_eq!(log.summary(), "4 on the stack · 3 on trunk");
        assert_eq!(log.changes().count(), 7);
        assert_eq!(log.change("e").unwrap().subject, "design");
    }

    #[test]
    fn reads_the_prefix_only_when_it_is_one() {
        let parse = conventional;
        assert_eq!(
            parse("feat(api)!: break it"),
            (Some("feat".into()), Some("api".into()), "break it".into())
        );
        assert_eq!(parse("Note: not a type").0, Some("Note".into()));
        for line in ["just words", "two words: here", "feat(): empty", ""] {
            assert_eq!(parse(line), (None, None, line.to_owned()), "{line}");
        }
    }

    #[test]
    fn other_details_are_not_a_log() {
        assert!(ChangeLog::parse(&json!({ "summary": "5 matches" })).is_none());
        let empty =
            ChangeLog::parse(&json!({ "changes": [], "more": false })).unwrap();
        assert_eq!(empty.summary(), "No changes yet");
    }
}
