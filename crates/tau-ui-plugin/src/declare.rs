//! What a plugin declares of itself, in one place (ADR 0045): the
//! plugins it comes after or before, and its reach — the seams it steps
//! in at, the slots it fills, the services it provides and uses, and
//! what it reaches outside tau. The registry orders plugins by it, the
//! kernel gives a plugin only what it declared, and the Plugins screen
//! shows it.

use serde::{Deserialize, Serialize};

use crate::Seam;

/// A plugin's declaration ([`crate::UiPlugin::declaration`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Declaration {
    /// The plugins it comes after, where they are in the registry.
    pub after: Vec<&'static str>,
    /// The plugins it comes before, where they are in the registry.
    pub before: Vec<&'static str>,
    pub reach: Reach,
}

impl Declaration {
    pub fn new() -> Self {
        Self::default()
    }

    /// Comes after `plugins`: its hooks run after theirs, its part of a
    /// prompt follows theirs.
    pub fn after(mut self, plugins: &[&'static str]) -> Self {
        self.after.extend_from_slice(plugins);
        self
    }

    /// Comes before `plugins`.
    pub fn before(mut self, plugins: &[&'static str]) -> Self {
        self.before.extend_from_slice(plugins);
        self
    }

    /// Steps in at `seams` of a run: the host calls no other.
    pub fn seams(mut self, seams: &[Seam]) -> Self {
        self.reach.seams.extend_from_slice(seams);
        self
    }

    /// Fills `slots` ([`crate::slots`]): a filler for any other is
    /// refused.
    pub fn fills(mut self, slots: &[&str]) -> Self {
        self.reach.fills.extend(strings(slots));
        self
    }

    /// Gives other plugins `services`.
    pub fn provides(mut self, services: &[&str]) -> Self {
        self.reach.provides.extend(strings(services));
        self
    }

    /// Takes `services` ([`crate::Service::NAME`]) from the host: those
    /// it does not name, it is not given.
    pub fn services(mut self, services: &[&str]) -> Self {
        self.reach.services.extend(strings(services));
        self
    }

    /// Calls `tools` of other plugins'.
    pub fn tools(mut self, tools: &[&str]) -> Self {
        self.reach.tools.extend(strings(tools));
        self
    }

    /// Reads the store beyond its own records.
    pub fn store(mut self, store: StoreReach) -> Self {
        self.reach.store = store;
        self
    }

    /// Talks to `hosts` over the network.
    pub fn network(mut self, hosts: &[&str]) -> Self {
        self.reach.network.extend(strings(hosts));
        self
    }

    /// Keeps or reads the secrets named `secrets`.
    pub fn secrets(mut self, secrets: &[&str]) -> Self {
        self.reach.secrets.extend(strings(secrets));
        self
    }

    /// Only the computer may ask its host half: what a phone asks it is
    /// refused, as a sign-in that opens the computer's browser.
    pub fn computer_only(mut self) -> Self {
        self.reach.computer_only = true;
        self
    }

    /// Reads `paths` outside repositories, as the person knows them, `~`
    /// their home (ADR 0044): another tool's directory, such as
    /// `~/.claude`. The Plugins screen shows them on the plugin's page,
    /// and setup under its step.
    pub fn reads(mut self, paths: &[&str]) -> Self {
        self.reach.reads.extend(strings(paths));
        self
    }
}

fn strings(names: &[&str]) -> impl Iterator<Item = String> {
    names.iter().map(|name| (*name).to_owned())
}

/// What a plugin reaches. Rust plugins are compiled in, and so allowed;
/// the kernel still calls only the seams, gives only the services and
/// takes only the fillers it names. The rest (tools, the store, the
/// network, secrets and paths read) is a statement, shown the same way
/// as a Luau plugin's.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Reach {
    pub seams: Vec<Seam>,
    pub fills: Vec<String>,
    pub provides: Vec<String>,
    pub services: Vec<String>,
    pub tools: Vec<String>,
    pub store: StoreReach,
    pub network: Vec<String>,
    pub secrets: Vec<String>,
    pub reads: Vec<String>,
    /// Only the computer may ask its host half ([`Declaration::computer_only`]).
    #[serde(default)]
    pub computer_only: bool,
}

impl Reach {
    pub fn has(&self, seam: Seam) -> bool {
        self.seams.contains(&seam)
    }
}

/// How much of the store a plugin reads.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
pub enum StoreReach {
    /// Its own records only.
    #[default]
    Own,
    /// Every run's, read-only.
    Read,
}

/// Puts `plugins`, each with its name and declaration, in an order that
/// keeps every `after` and `before` between plugins present, the rest
/// by name: the same for the same plugins, in whatever order they came.
///
/// # Errors
///
/// A cycle, with two of the plugins in it.
pub fn order<'a>(
    plugins: &[(&'a str, &Declaration)],
) -> Result<Vec<usize>, (&'a str, &'a str)> {
    use std::collections::{BTreeMap, BTreeSet};

    let index: BTreeMap<&str, usize> = plugins
        .iter()
        .enumerate()
        .map(|(at, (name, _))| (*name, at))
        .collect();
    // `edges[a]` holds the plugins that come after `a`.
    let mut edges: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); plugins.len()];
    for (at, (_, declaration)) in plugins.iter().enumerate() {
        for name in &declaration.after {
            if let Some(&first) = index.get(name) {
                edges[first].insert(at);
            }
        }
        for name in &declaration.before {
            if let Some(&then) = index.get(name) {
                edges[at].insert(then);
            }
        }
    }
    let mut waiting = vec![0usize; plugins.len()];
    for then in edges.iter().flatten() {
        waiting[*then] += 1;
    }
    let mut ready: BTreeSet<(&str, usize)> = (0..plugins.len())
        .filter(|at| waiting[*at] == 0)
        .map(|at| (plugins[at].0, at))
        .collect();
    let mut ordered = Vec::with_capacity(plugins.len());
    while let Some((_, at)) = ready.pop_first() {
        ordered.push(at);
        for &then in &edges[at] {
            waiting[then] -= 1;
            if waiting[then] == 0 {
                ready.insert((plugins[then].0, then));
            }
        }
    }
    if ordered.len() == plugins.len() {
        return Ok(ordered);
    }
    // What is left waits on a cycle: walk back from one of them, along
    // what it waits on, until a plugin comes round again.
    let left: BTreeSet<usize> =
        (0..plugins.len()).filter(|at| waiting[*at] > 0).collect();
    let mut walked: Vec<usize> = Vec::new();
    let mut at = *left.first().expect("something is left");
    loop {
        if let Some(seen) = walked.iter().position(|was: &usize| *was == at) {
            let cycle = &walked[seen..];
            let first = cycle[0];
            let second = *cycle.get(1).unwrap_or(&first);
            return Err((plugins[first].0, plugins[second].0));
        }
        walked.push(at);
        at = (0..plugins.len())
            .find(|before| {
                left.contains(before) && edges[*before].contains(&at)
            })
            .expect("a plugin left waits on another left");
    }
}

#[cfg(test)]
mod tests {
    use hegel::generators as gs;

    use super::*;

    fn named(
        names: &[&'static str],
        declarations: &[Declaration],
    ) -> Vec<&'static str> {
        let plugins: Vec<(&str, &Declaration)> =
            names.iter().copied().zip(declarations).collect();
        order(&plugins)
            .expect("no cycle")
            .into_iter()
            .map(|at| names[at])
            .collect()
    }

    #[test]
    fn plugins_with_nothing_declared_go_by_name() {
        let declarations = vec![Declaration::new(); 3];
        assert_eq!(named(&["c", "a", "b"], &declarations), ["a", "b", "c"]);
    }

    #[test]
    fn after_and_before_win_over_names() {
        let declarations = [
            Declaration::new().after(&["b"]),
            Declaration::new(),
            Declaration::new().before(&["b"]),
        ];
        assert_eq!(named(&["a", "b", "c"], &declarations), ["c", "b", "a"]);
    }

    #[test]
    fn a_plugin_not_present_orders_nothing() {
        let declarations =
            [Declaration::new().after(&["gone"]), Declaration::new()];
        assert_eq!(named(&["b", "a"], &declarations), ["a", "b"]);
    }

    #[test]
    fn a_cycle_names_two_plugins_in_it() {
        let declarations = [
            Declaration::new(),
            Declaration::new().after(&["c"]),
            Declaration::new().after(&["b"]),
        ];
        let plugins: Vec<(&str, &Declaration)> =
            ["a", "b", "c"].into_iter().zip(&declarations).collect();
        let (first, second) = order(&plugins).unwrap_err();
        let mut both = [first, second];
        both.sort_unstable();
        assert_eq!(both, ["b", "c"]);
    }

    const NAMES: [&str; 6] = ["p0", "p1", "p2", "p3", "p4", "p5"];

    // Property: for any acyclic set of `after` edges (only from a later
    // name to an earlier one, so none cycle), the order keeps every
    // edge, holds each plugin once, and is the same for any order the
    // plugins came in.
    #[hegel::test]
    fn the_order_keeps_every_edge_whatever_order_plugins_came_in(
        tc: hegel::TestCase,
    ) {
        let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(6));
        let names = &NAMES[..count];
        let declarations: Vec<Declaration> = (0..count)
            .map(|at| {
                let after: Vec<&'static str> = (0..at)
                    .filter(|_| tc.draw(gs::booleans()))
                    .map(|first| NAMES[first])
                    .collect();
                Declaration::new().after(&after)
            })
            .collect();
        let mut shuffled: Vec<usize> = (0..count).collect();
        for at in (1..count).rev() {
            let with = tc.draw(gs::integers::<usize>().max_value(at));
            shuffled.swap(at, with);
        }

        let straight = named(names, &declarations);
        let shuffled_names: Vec<&'static str> =
            shuffled.iter().map(|at| names[*at]).collect();
        let shuffled_declarations: Vec<Declaration> = shuffled
            .iter()
            .map(|at| declarations[*at].clone())
            .collect();
        assert_eq!(named(&shuffled_names, &shuffled_declarations), straight);

        assert_eq!(straight.len(), count);
        let place = |name: &str| {
            straight
                .iter()
                .position(|was| *was == name)
                .expect("present")
        };
        for (at, declaration) in declarations.iter().enumerate() {
            for first in &declaration.after {
                assert!(place(first) < place(names[at]));
            }
        }
    }
}
