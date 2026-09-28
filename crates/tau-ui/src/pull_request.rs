//! A pull request made from a run: the draft the host writes from the
//! run's commits, and what GitHub says once it is open.

/// A commit the pull request carries, with its line counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCommit {
    pub title: String,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checks {
    Running,
    Passed,
    Failed,
    /// The repository runs no checks.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrState {
    /// The user is still editing it.
    Draft,
    Creating,
    Opened {
        number: u64,
        url: String,
        checks: Checks,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// `owner/name`.
    pub repo: String,
    /// The branch tau pushes.
    pub head: String,
    pub base: String,
    /// Whether the head merges into the base cleanly.
    pub mergeable: bool,
    /// One sentence on the run it comes from.
    pub summary: String,
    /// The tests the run passed, in words: `14 tests passed`.
    pub tests: Option<String>,
    pub title: String,
    /// The description, written from the run.
    pub body: String,
    pub commits: Vec<PrCommit>,
    pub draft: bool,
    /// Push the run's later turns to the same branch.
    pub keep_pushing: bool,
    pub state: PrState,
}

impl PullRequest {
    pub fn is_open(&self) -> bool {
        matches!(self.state, PrState::Opened { .. })
    }

    /// The description's first paragraph, for the phone.
    pub fn short_body(&self) -> String {
        let first = self.body.split("\n\n").next().unwrap_or_default();
        first.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_body_is_the_first_paragraph_on_one_line() {
        let pr = PullRequest {
            repo: "a/b".into(),
            head: "tau/x".into(),
            base: "main".into(),
            mergeable: true,
            summary: String::new(),
            tests: None,
            title: "t".into(),
            body: "one\ntwo\n\nthree".into(),
            commits: vec![],
            draft: true,
            keep_pushing: true,
            state: PrState::Draft,
        };
        assert_eq!(pr.short_body(), "one two");
        assert!(!pr.is_open());
    }
}
