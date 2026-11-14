//! jj's merge terms, as the model tests predict trees with them.

use std::collections::BTreeMap;

/// A file's contents; `None` is no file.
pub type Val = Option<&'static str>;

/// A path's value in a tree: jj's merge terms, counted (+1 for each
/// add, -1 for each remove, zeros dropped). One value counted once is a
/// resolved file; anything else is a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term(pub BTreeMap<Val, i32>);

impl Term {
    pub fn resolved(value: Val) -> Self {
        Self(BTreeMap::from([(value, 1)]))
    }

    pub fn value(&self) -> Option<Val> {
        match self.0.iter().collect::<Vec<_>>().as_slice() {
            [(value, 1)] => Some(**value),
            _ => None,
        }
    }

    /// What jj writes to disk for a conflict: its sides merged with a
    /// missing file read as empty, when that resolves; `None` when the
    /// file gets conflict markers.
    pub fn materialized(&self) -> Option<Val> {
        let mut counts: BTreeMap<&str, i32> = BTreeMap::new();
        for (value, n) in &self.0 {
            *counts.entry(value.unwrap_or("")).or_default() += n;
        }
        counts.retain(|_, n| *n != 0);
        let positive: Vec<&str> = counts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(value, _)| *value)
            .collect();
        match (counts.len(), positive.as_slice()) {
            (1, [value]) | (2, [value]) => Some(Some(*value)),
            _ => None,
        }
    }

    /// `old`, rebased from `base` onto `onto`: jj's `onto + old - base`,
    /// resolved as `trivial_merge` does with `same-change = accept`.
    pub fn rebase(onto: &Term, base: &Term, old: &Term) -> Term {
        let mut counts = onto.0.clone();
        for (value, n) in &old.0 {
            *counts.entry(*value).or_default() += n;
        }
        for (value, n) in &base.0 {
            *counts.entry(*value).or_default() -= n;
        }
        counts.retain(|_, n| *n != 0);
        let positive: Vec<Val> = counts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(value, _)| *value)
            .collect();
        match (counts.len(), positive.as_slice()) {
            (1, [value]) | (2, [value]) => Term::resolved(*value),
            _ => Term(counts),
        }
    }
}

/// Path to value; a path missing is no file.
pub type Tree = BTreeMap<&'static str, Term>;

pub fn get(tree: &Tree, path: &'static str) -> Term {
    tree.get(path)
        .cloned()
        .unwrap_or_else(|| Term::resolved(None))
}

pub fn set(tree: &mut Tree, path: &'static str, term: Term) {
    if term == Term::resolved(None) {
        tree.remove(path);
    } else {
        tree.insert(path, term);
    }
}

/// `old`, rebased from `base` onto `onto`, path by path. A path none of
/// them has stays missing.
pub fn rebase_tree(onto: &Tree, base: &Tree, old: &Tree) -> Tree {
    let paths: std::collections::BTreeSet<&'static str> = onto
        .keys()
        .chain(base.keys())
        .chain(old.keys())
        .copied()
        .collect();
    let mut tree = Tree::new();
    for path in paths {
        set(
            &mut tree,
            path,
            Term::rebase(&get(onto, path), &get(base, path), &get(old, path)),
        );
    }
    tree
}

/// The paths of `tree` in conflict.
pub fn conflicts(tree: &Tree) -> Vec<String> {
    tree.iter()
        .filter(|(_, term)| term.value().is_none())
        .map(|(path, _)| (*path).to_owned())
        .collect()
}
