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
    use hegel::generators as gs;

    use super::*;

    #[derive(Debug, PartialEq)]
    struct Number(u32);

    #[derive(Debug, PartialEq)]
    struct Text(String);

    #[derive(Debug, PartialEq)]
    struct Missing;

    fn assert_snapshot_matches_model(
        snapshot: &Services,
        expected: &(Option<u32>, Option<String>),
    ) {
        assert_eq!(snapshot.get::<Number>().map(|number| number.0), expected.0);
        assert_eq!(
            snapshot.get::<Text>().map(|text| text.0.as_str()),
            expected.1.as_deref()
        );
        assert_eq!(snapshot.get::<Missing>(), None);
    }

    fn assert_snapshots_match_model(
        snapshots: &[Services],
        model: &[(Option<u32>, Option<String>)],
    ) {
        assert_eq!(snapshots.len(), model.len());
        for (snapshot, expected) in snapshots.iter().zip(model) {
            assert_snapshot_matches_model(snapshot, expected);
        }
    }

    // Property inventory: every history prefix must match the independent typed
    // model, preserve each type's last write, and leave Missing absent. Draw at
    // most 24 operations (tag 0..=4, u8 target, u32 number, optional text up to
    // 8 chars); cap snapshots at 8 and turn excess clone requests into queries,
    // so generation needs no rejection. Hegel shrinks by dropping operations;
    // the initial clone and distinct branch writes stay forced as a COW witness.
    #[hegel::test]
    fn cloned_snapshots_isolate_typed_writes_and_keep_last_values(
        tc: hegel::TestCase,
    ) {
        let operations: Vec<(u8, u8, u32, Option<String>)> = tc.draw(
            gs::vecs(hegel::tuples!(
                gs::integers::<u8>().max_value(4),
                gs::integers::<u8>(),
                gs::integers::<u32>(),
                gs::optional(gs::text().max_size(8)),
            ))
            .max_size(24),
        );

        let mut snapshots = vec![Services::default()];
        let mut model = vec![(None, None)];
        assert_snapshots_match_model(&snapshots, &model);

        snapshots.push(snapshots[0].clone());
        model.push(model[0].clone());
        assert_snapshots_match_model(&snapshots, &model);

        snapshots[0] = snapshots[0].clone().with(Number(7));
        model[0].0 = Some(7);
        assert_snapshots_match_model(&snapshots, &model);

        snapshots[1] = snapshots[1].clone().with(Text("branch".to_owned()));
        model[1].1 = Some("branch".to_owned());
        assert_snapshots_match_model(&snapshots, &model);

        // Keep the original last-write behavior explicit, with both service
        // types present in one snapshot.
        snapshots[0] = snapshots[0]
            .clone()
            .with(Number(11))
            .with(Text("primary".to_owned()));
        model[0] = (Some(11), Some("primary".to_owned()));
        assert_snapshots_match_model(&snapshots, &model);

        snapshots[1] =
            snapshots[1].clone().with(Text("branch-last".to_owned()));
        model[1].1 = Some("branch-last".to_owned());
        assert_snapshots_match_model(&snapshots, &model);

        // A populated clone must preserve both types, not just isolate writes.
        snapshots.push(snapshots[0].clone());
        model.push(model[0].clone());
        assert_snapshots_match_model(&snapshots, &model);

        for (operation, target_index, number, text) in operations {
            let target = usize::from(target_index) % snapshots.len();
            match operation {
                // The API's same-type `with` call inserts when absent and
                // replaces when present; both tags exercise that upsert law.
                0 | 1 => {
                    snapshots[target] =
                        snapshots[target].clone().with(Number(number));
                    model[target].0 = Some(number);
                }
                2 | 3 => {
                    if let Some(text) = text {
                        snapshots[target] =
                            snapshots[target].clone().with(Text(text.clone()));
                        model[target].1 = Some(text);
                    }
                }
                4 if snapshots.len() < 8 => {
                    snapshots.push(snapshots[target].clone());
                    model.push(model[target].clone());
                }
                4 => {} // Query instead of cloning once the snapshot bound is reached.
                _ => unreachable!("operation tags are generated in 0..=4"),
            }
            assert_snapshots_match_model(&snapshots, &model);
        }
    }
}
