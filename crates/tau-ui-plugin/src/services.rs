//! What the host hands plugins by type: the metered Jev, a run's
//! workspace, anything a host adds later. The set is open: a host puts
//! in what it has, and a plugin takes what it needs, so a new service
//! needs no change to the interface.

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Arc,
};

/// Values by type, cheap to clone.
#[derive(Clone, Default)]
pub struct Services(Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>);

impl Services {
    /// These services and `value`, which replaces one of its type.
    pub fn with<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        Arc::make_mut(&mut self.0).insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// The service of type `T`, if the host has one.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.0.get(&TypeId::of::<T>())?.downcast_ref()
    }
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Services({} values)", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A service comes back by its type, the last one put in winning,
    /// and a missing type is `None`.
    #[hegel::test(test_cases = 100)]
    fn services_come_back_by_type(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let numbers: Vec<u32> =
            tc.draw(gs::vecs(gs::integers::<u32>()).min_size(1).max_size(5));
        let text: Option<String> =
            tc.draw(gs::optional(gs::text().max_size(8)));
        let mut services = Services::default();
        for n in &numbers {
            services = services.with(*n);
        }
        if let Some(text) = &text {
            services = services.with(text.clone());
        }
        let copy = services.clone();
        assert_eq!(copy.get::<u32>(), numbers.last());
        assert_eq!(copy.get::<String>(), text.as_ref());
        assert_eq!(copy.get::<u64>(), None);
    }
}
