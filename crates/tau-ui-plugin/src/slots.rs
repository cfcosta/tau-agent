//! Slots (ADR 0045): what the kernel asks of one plugin, by name, through
//! a trait of its own, such as version control ([`crate::vcs::VCS`]). A
//! plugin's host half fills the slots it can ([`crate::HostHalf::fill`]),
//! and the host takes each slot's filler from [`Fills`].

use std::{any::Any, collections::HashMap, marker::PhantomData, sync::Arc};

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

/// The fillers of the slots, by slot name. Cheap to clone.
#[derive(Default, Clone)]
pub struct Fills(HashMap<&'static str, Arc<dyn Any + Send + Sync>>);

impl Fills {
    /// Fills `slot` with `filler`, unless it is filled already: the first
    /// filler wins.
    pub fn fill<T: ?Sized + Send + Sync + 'static>(
        &mut self,
        slot: &Slot<T>,
        filler: Arc<T>,
    ) {
        // `T` may be unsized, as a trait is: the `Arc` is what is boxed.
        let filler: Arc<dyn Any + Send + Sync> = Arc::new(filler);
        self.0.entry(slot.name).or_insert(filler);
    }

    /// What fills `slot`, if anything does.
    pub fn get<T: ?Sized + Send + Sync + 'static>(
        &self,
        slot: &Slot<T>,
    ) -> Option<Arc<T>> {
        self.0.get(slot.name)?.downcast_ref::<Arc<T>>().cloned()
    }
}

impl std::fmt::Debug for Fills {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
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

    #[test]
    fn the_first_filler_wins() {
        let mut fills = Fills::default();
        assert!(fills.get(&NAMED).is_none());
        fills.fill(&NAMED, Arc::new(Called("first")) as Arc<dyn Named>);
        fills.fill(&NAMED, Arc::new(Called("second")) as Arc<dyn Named>);
        let named = fills.get(&NAMED).expect("filled");
        assert_eq!(named.name(), "first");
    }
}
