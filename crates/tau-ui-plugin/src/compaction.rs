//! The `tau.compaction` slot (ADR 0045): which plugins rewrite a run's
//! context. Several can (fast pruning, the tree, the summary); the
//! slot's filler names those that do, and the kernel's gate turns off
//! the rewrites of every other ([`Rewriters`]). They run in the
//! registry's order, the first rewrite winning, as each declares.

use crate::{Seam, slots::Slot};

/// The slot.
pub const COMPACTION: Slot<dyn Compaction> = Slot::new("tau.compaction");

/// What fills the slot.
pub trait Compaction: Send + Sync {
    /// The plugins whose rewrites run, by name.
    fn rewriters(&self) -> Vec<String>;
}

/// The rewriters of a run, as the host gives them (a kernel value of
/// `crate::Services`): none named when nothing fills the slot, and then
/// every plugin that rewrites does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewriters(pub Vec<String>);

impl Rewriters {
    /// The seams `plugin`, which declared `seams`, steps in at in a run:
    /// a plugin that rewrites the context and is not named loses its
    /// rewrite, and its part in an idle compaction.
    pub fn seams(
        rewriters: Option<&Self>,
        plugin: &str,
        seams: &[Seam],
    ) -> Vec<Seam> {
        let off = rewriters.is_some_and(|rewriters| {
            seams.contains(&Seam::Rewrite)
                && !rewriters.0.iter().any(|name| name == plugin)
        });
        seams
            .iter()
            .copied()
            .filter(|seam| {
                !off || !matches!(seam, Seam::Rewrite | Seam::StartIdle)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewriter_not_named_loses_its_rewrite_and_its_idle_part() {
        let seams = [Seam::Start, Seam::StartIdle, Seam::Rewrite];
        let summary = Rewriters(vec!["summary".to_owned()]);
        assert_eq!(
            Rewriters::seams(Some(&summary), "pruning", &seams),
            [Seam::Start]
        );
        assert_eq!(Rewriters::seams(Some(&summary), "summary", &seams), seams);
        assert_eq!(Rewriters::seams(None, "pruning", &seams), seams);
        // A plugin that does not rewrite keeps its idle part.
        let notes = [Seam::StartIdle, Seam::Rewritten];
        assert_eq!(Rewriters::seams(Some(&summary), "notes", &notes), notes);
    }
}
