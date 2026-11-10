//! What the host hands plugins by type: a run's workspace, the person's
//! model, what a plugin provides others (such as tau-jev's Jev), anything
//! a host adds later. The set is open: a host puts
//! in what it has, and a plugin takes what it needs, so a new service
//! needs no change to the interface.

use std::{
    any::{Any, TypeId},
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
};

/// The names of the services the host offers ([`Services::offer`]),
/// which a plugin declares to be given them.
pub mod names {
    /// A run's workspace ([`crate::WorkspaceDir`]).
    pub const WORKSPACE_DIR: &str = "tau.workspace-dir";
    /// Where the person's skills are ([`crate::SkillsDir`]).
    pub const SKILLS_DIR: &str = "tau.skills-dir";
    /// What a run's turns leave in its workspace ([`crate::TurnHooks`]).
    pub const TURN_HOOKS: &str = "tau.turn-hooks";
    /// The commands a run leaves running ([`crate::RunCommands`]).
    pub const RUN_COMMANDS: &str = "tau.run-commands";
    /// Wakes a run that waits ([`crate::RunWake`]).
    pub const RUN_WAKE: &str = "tau.run-wake";
    /// That the run is an agent's, in its own repository
    /// ([`crate::AgentRepo`]).
    pub const AGENT_REPO: &str = "tau.agent-repo";
    /// What a repository's agent commands start through
    /// ([`crate::RepoLauncher`]).
    pub const REPO_LAUNCHER: &str = "tau.repo-launcher";
    /// The host's conversations ([`crate::Conversations`]).
    pub const CONVERSATIONS: &str = "tau.conversations";
    /// Tools outside any run ([`crate::OutsideTools`]).
    pub const OUTSIDE_TOOLS: &str = "tau.outside-tools";
    /// MCP resources outside any run ([`crate::OutsideResources`]).
    pub const OUTSIDE_RESOURCES: &str = "tau.outside-resources";
    /// Repositories no person attends ([`crate::Unattended`]).
    pub const UNATTENDED: &str = "tau.unattended";
    /// The person's model, outside any run ([`crate::Model`]).
    pub const MODEL: &str = "tau.model";
    /// Who the person is, for the commits made for them.
    pub const IDENTITY: &str = "tau.identity";
    /// The computer's window, as phones mirror it
    /// (`Arc<dyn crate::app::App>`).
    pub const APP: &str = "tau.app";

    /// Every service the host offers by name.
    pub const ALL: [&str; 13] = [
        WORKSPACE_DIR,
        TURN_HOOKS,
        RUN_COMMANDS,
        RUN_WAKE,
        AGENT_REPO,
        REPO_LAUNCHER,
        CONVERSATIONS,
        OUTSIDE_TOOLS,
        OUTSIDE_RESOURCES,
        UNATTENDED,
        MODEL,
        IDENTITY,
        APP,
    ];
}

/// Values by type, cheap to clone.
///
/// A value is the kernel's, given to every plugin ([`Services::with`]:
/// its directories, saved settings and jobs), or a named service
/// ([`Services::offer`]), given only to the plugins that declare its
/// name ([`crate::Declaration::services`]). The registry hands each
/// plugin its own view ([`Services::scoped`]).
#[derive(Clone, Default)]
pub struct Services {
    values: Arc<HashMap<TypeId, Value>>,
    /// Whose view this is, and the names it declared; every name when
    /// none.
    scope: Option<Arc<Scope>>,
}

#[derive(Clone)]
struct Value {
    /// The service's name; none for the kernel's own.
    name: Option<&'static str>,
    value: Arc<dyn Any + Send + Sync>,
}

struct Scope {
    plugin: &'static str,
    names: Vec<String>,
}

impl Services {
    /// These services and `value`, the kernel's own, which every plugin
    /// is given; it replaces one of its type.
    pub fn with<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.put(None, value)
    }

    /// These services and `value`, the service named `name`, which only
    /// plugins that declare `name` are given; it replaces one of its
    /// type.
    pub fn offer<T: Send + Sync + 'static>(
        self,
        name: &'static str,
        value: T,
    ) -> Self {
        self.put(Some(name), value)
    }

    fn put<T: Send + Sync + 'static>(
        mut self,
        name: Option<&'static str>,
        value: T,
    ) -> Self {
        Arc::make_mut(&mut self.values).insert(
            TypeId::of::<T>(),
            Value {
                name,
                value: Arc::new(value),
            },
        );
        self
    }

    /// The service of type `T`, if the host has one and this view may
    /// have it. A plugin that reaches a service it did not declare gets
    /// none, and the kernel says so once, naming both.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        let value = self.values.get(&TypeId::of::<T>())?;
        if let (Some(name), Some(scope)) = (value.name, &self.scope)
            && !scope.names.iter().any(|declared| declared == name)
        {
            undeclared(scope.plugin, name);
            return None;
        }
        value.value.downcast_ref()
    }

    /// The view of `plugin`, which declared `names`: the kernel's values
    /// and the services it named.
    pub fn scoped(&self, plugin: &'static str, names: &[String]) -> Self {
        Self {
            values: Arc::clone(&self.values),
            scope: Some(Arc::new(Scope {
                plugin,
                names: names.to_vec(),
            })),
        }
    }

    /// These services and all of `other`'s, which replace any of their
    /// types here.
    pub fn and(mut self, other: &Services) -> Self {
        let values = Arc::make_mut(&mut self.values);
        for (id, value) in other.values.iter() {
            values.insert(*id, value.clone());
        }
        self
    }

    /// The names of the services here, the kernel's own left out.
    pub fn names(&self) -> BTreeSet<&'static str> {
        self.values
            .values()
            .filter_map(|value| value.name)
            .collect()
    }
}

/// What a plugin's host half gives other plugins
/// ([`crate::HostHalf::provide`]): only the services its declaration
/// names ([`crate::Declaration::provides`]).
pub struct Provide<'a> {
    plugin: &'static str,
    names: &'a [String],
    services: Services,
}

impl<'a> Provide<'a> {
    pub(crate) fn new(
        plugin: &'static str,
        names: &'a [String],
        services: Services,
    ) -> Self {
        Self {
            plugin,
            names,
            services,
        }
    }

    /// What the host has put in so far, as the plugin is given it.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.services.get::<T>()
    }

    /// Gives `value` as the service `name`. A name the plugin does not
    /// declare it provides is refused, and the kernel says so.
    pub fn offer<T: Send + Sync + 'static>(
        &mut self,
        name: &'static str,
        value: T,
    ) {
        if !self.names.iter().any(|declared| declared == name) {
            eprintln!(
                "tau-ui-plugin: {} provides the service {name}, which it \
                 does not declare; it is refused",
                self.plugin
            );
            return;
        }
        self.services = std::mem::take(&mut self.services).offer(name, value);
    }

    pub(crate) fn into_services(self) -> Services {
        self.services
    }
}

/// Says once that `plugin` reached the service `name` it did not
/// declare.
fn undeclared(plugin: &'static str, name: &'static str) {
    static SAID: Mutex<BTreeSet<(&str, &str)>> = Mutex::new(BTreeSet::new());
    if SAID.lock().expect("not poisoned").insert((plugin, name)) {
        eprintln!(
            "tau-ui-plugin: {plugin} reached the service {name}, which it \
             does not declare; it is not given it"
        );
    }
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Services({} values)", self.values.len())
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

    #[test]
    fn a_plugin_is_given_the_kernels_values_and_the_services_it_declares() {
        let services = Services::default()
            .with(Number(1))
            .offer("text", Text("named".to_owned()));
        let declared = services.scoped("reader", &["text".to_owned()]);
        let undeclared = services.scoped("other", &[]);

        assert_eq!(declared.get::<Number>(), Some(&Number(1)));
        assert_eq!(declared.get::<Text>(), Some(&Text("named".to_owned())));
        assert_eq!(undeclared.get::<Number>(), Some(&Number(1)));
        assert_eq!(undeclared.get::<Text>(), None);
        assert_eq!(services.get::<Text>(), Some(&Text("named".to_owned())));
        assert_eq!(services.names(), BTreeSet::from(["text"]));
    }

    #[test]
    fn a_plugin_provides_only_the_services_it_declares() {
        let declared = ["text".to_owned()];
        let mut provide = Provide::new("giver", &declared, Services::default());
        provide.offer("text", Text("given".to_owned()));
        provide.offer("number", Number(2));
        let services = provide.into_services();
        assert_eq!(services.get::<Text>(), Some(&Text("given".to_owned())));
        assert_eq!(services.get::<Number>(), None);

        let joined = Services::default().with(Number(1)).and(&services);
        assert_eq!(joined.get::<Number>(), Some(&Number(1)));
        assert_eq!(joined.get::<Text>(), Some(&Text("given".to_owned())));
    }

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
