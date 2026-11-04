//! Slots (ADR 0045): what the kernel asks of one plugin, by name, through
//! a trait of its own, such as version control ([`crate::vcs::VCS`]). A
//! plugin's host half offers to fill the slots it can
//! ([`crate::HostHalf::fill`]), and the host takes each slot's filler from
//! [`Fills`]: the person's pick, else the first in the slot's declared
//! order.

use std::{
    any::Any,
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    sync::Arc,
};

/// A slot: its name, and the trait its filler is.
pub struct Slot<T: ?Sized> {
    name: &'static str,
    _t: PhantomData<fn() -> Arc<T>>,
}

impl<T: ?Sized> Slot<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _t: PhantomData,
        }
    }

    /// Its name, such as `tau.vcs`.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// The plugin key the interface's state keeps the person's picks under
/// (ADR 0045): for each slot by name, the plugin they picked to fill it.
/// Being UI state, a pick reaches phones and outlives a restart.
pub const PICKS: &str = "tau.slots";

/// Who fills a slot among `candidates`, given in the slot's order
/// ([`Fills::candidates`]): the person's pick when it is one of them,
/// else the first.
pub fn chosen<'a>(
    candidates: &'a [String],
    pick: Option<&str>,
) -> Option<&'a str> {
    candidates
        .iter()
        .find(|candidate| Some(candidate.as_str()) == pick)
        .or_else(|| candidates.first())
        .map(String::as_str)
}

/// A plugin that can fill a slot, and what it fills it with.
#[derive(Clone)]
struct Candidate {
    plugin: String,
    filler: Arc<dyn Any + Send + Sync>,
}

/// What can fill each slot, and who does. Cheap to clone.
///
/// Every plugin that can fill a slot is a candidate. The slot is filled
/// by the person's pick ([`Fills::pick`]) when that plugin is a
/// candidate, else by the first candidate in the slot's declared order
/// ([`Fills::declare`], from `Registry::slot_order`), else by the first
/// by name: never by the order plugins were registered in.
#[derive(Default, Clone)]
pub struct Fills {
    candidates: HashMap<&'static str, Vec<Candidate>>,
    orders: HashMap<String, Vec<String>>,
    picks: BTreeMap<String, String>,
    /// The plugin whose fillers [`Fills::fill`] takes now.
    filling: String,
}

impl Fills {
    /// Declares `slot`'s order: who fills it first when several can.
    pub fn declare(&mut self, slot: &str, order: &[&str]) {
        self.orders.insert(
            slot.to_owned(),
            order.iter().map(|name| (*name).to_owned()).collect(),
        );
    }

    /// The fillers [`Fills::fill`] takes from now on are `plugin`'s: the
    /// registry says so before it asks a plugin to fill its slots.
    pub fn filling(&mut self, plugin: &str) {
        self.filling = plugin.to_owned();
    }

    /// The plugin filling now can fill `slot` with `filler`; filled
    /// again by the same plugin, the last filler counts.
    pub fn fill<T: ?Sized + Send + Sync + 'static>(
        &mut self,
        slot: &Slot<T>,
        filler: Arc<T>,
    ) {
        // `T` may be unsized, as a trait is: the `Arc` is what is boxed.
        let filler: Arc<dyn Any + Send + Sync> = Arc::new(filler);
        let candidates = self.candidates.entry(slot.name).or_default();
        candidates.retain(|candidate| candidate.plugin != self.filling);
        candidates.push(Candidate {
            plugin: self.filling.clone(),
            filler,
        });
    }

    /// The person picked `plugin` to fill `slot`, or nobody: the slot's
    /// order decides again.
    pub fn pick(&mut self, slot: &str, plugin: Option<&str>) {
        match plugin {
            Some(plugin) => {
                self.picks.insert(slot.to_owned(), plugin.to_owned());
            }
            None => {
                self.picks.remove(slot);
            }
        }
    }

    /// Takes the picks the interface's state holds ([`PICKS`]), in place
    /// of those it had.
    pub fn picks_from(&mut self, ui: &crate::UiState) {
        self.picks = ui
            .of(PICKS)
            .iter()
            .filter_map(|(slot, plugin)| {
                Some((slot.clone(), plugin.as_str()?.to_owned()))
            })
            .collect();
    }

    /// The plugins that can fill the slot named `slot`, in its order:
    /// those its declaration names, as it names them, then the rest by
    /// name.
    pub fn candidates(&self, slot: &str) -> Vec<String> {
        let order = self.orders.get(slot);
        let rank = |plugin: &str| {
            order
                .and_then(|order| order.iter().position(|name| name == plugin))
                .unwrap_or(usize::MAX)
        };
        let mut names: Vec<String> = self
            .candidates
            .get(slot)
            .into_iter()
            .flatten()
            .map(|candidate| candidate.plugin.clone())
            .collect();
        names.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
        names
    }

    /// The slots some plugin can fill, by name.
    pub fn slots(&self) -> Vec<&'static str> {
        let mut slots: Vec<&'static str> =
            self.candidates.keys().copied().collect();
        slots.sort_unstable();
        slots
    }

    /// The plugin that fills the slot named `slot`, if any can.
    pub fn filler(&self, slot: &str) -> Option<String> {
        let candidates = self.candidates(slot);
        let pick = self.picks.get(slot).map(String::as_str);
        chosen(&candidates, pick).map(str::to_owned)
    }

    /// What fills `slot`, if anything does.
    pub fn get<T: ?Sized + Send + Sync + 'static>(
        &self,
        slot: &Slot<T>,
    ) -> Option<Arc<T>> {
        let filler = self.filler(slot.name)?;
        self.candidates
            .get(slot.name)?
            .iter()
            .find(|candidate| candidate.plugin == filler)?
            .filler
            .downcast_ref::<Arc<T>>()
            .cloned()
    }
}

impl std::fmt::Debug for Fills {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let filled: BTreeMap<&str, Option<String>> = self
            .slots()
            .into_iter()
            .map(|slot| (slot, self.filler(slot)))
            .collect();
        f.debug_map().entries(filled).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Named: Send + Sync {
        fn name(&self) -> &str;
    }

    struct Called(&'static str);

    impl Named for Called {
        fn name(&self) -> &str {
            self.0
        }
    }

    const NAMED: Slot<dyn Named> = Slot::new("test.named");

    /// The slot's order picks among those who can fill it, whatever
    /// order they offered in; the person's pick wins over it, and a pick
    /// nobody can fill counts for nothing.
    #[test]
    fn the_order_then_the_pick_decides() {
        let offer = |order: &[&'static str]| {
            let mut fills = Fills::default();
            fills.declare(NAMED.name(), &["second", "first"]);
            for name in order {
                fills.filling(name);
                fills.fill(&NAMED, Arc::new(Called(name)) as Arc<dyn Named>);
            }
            fills
        };
        for order in
            [["first", "second", "third"], ["third", "second", "first"]]
        {
            let mut fills = offer(&order);
            assert_eq!(fills.get(&NAMED).unwrap().name(), "second");
            assert_eq!(
                fills.candidates(NAMED.name()),
                ["second", "first", "third"]
            );
            fills.pick(NAMED.name(), Some("third"));
            assert_eq!(fills.get(&NAMED).unwrap().name(), "third");
            fills.pick(NAMED.name(), Some("nobody"));
            assert_eq!(fills.get(&NAMED).unwrap().name(), "second");
        }
        assert!(Fills::default().get(&NAMED).is_none());
    }
}
