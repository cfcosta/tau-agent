//! Inventory: exact saved bytes, request identity, repository scope, decline,
//! replay, concurrent aliases, and later grants.
//! The generated sequence property uses a BTreeMap authorization oracle and
//! valid definitions, shrinking toward shorter histories and simple
//! versions. Workspace hegel.toml supplies case counts and CI's deterministic
//! profile; no per-test override is needed.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{collections::BTreeMap, sync::Arc};

use hegel::generators as gs;
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_codemode::{
    modules::{Definition, Library, Record as ModuleRecord, RepositoryPin},
    promotion::{Decision, Record as PromotionRecord, Request},
    store,
    ui::Action,
};
use tau_codemode_host::{
    CodemodeHost,
    PLUGIN,
    repository_modules::RepositoryModules,
};
use tau_store::{Entry, NewRun, RunKind, Store, TurnUsage};
use tau_ui_plugin::{
    HOST_RECORD,
    HostCx,
    HostHalf as _,
    HostRecord,
    RepoCtx,
    Services,
};

fn definition(
    name: &str,
    body: &str,
    dependencies: BTreeMap<String, String>,
) -> Definition {
    Definition::new(name.into(), body.into(), json!({}), dependencies).unwrap()
}

struct Fixture {
    path: std::path::PathBuf,
    repository: RepositoryModules,
    store: Store,
    runtime: tokio::runtime::Runtime,
    run: RunId,
}

impl Fixture {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
        let run = RunId(format!("run-{}", uuid::Uuid::now_v7()).into());
        runtime
            .block_on(store.create_run(&NewRun {
                id: &run.0,
                workflow_id: None,
                agent: "test",
                kind: RunKind::Root,
                model: "test",
                turns: 0,
            }))
            .unwrap();
        let path = std::env::temp_dir()
            .join(format!("promotion-{}", uuid::Uuid::now_v7()));
        let repository = RepositoryModules::new(path.join("codemode-modules"));
        let fixture = Self {
            path,
            repository,
            store,
            runtime,
            run,
        };
        fixture.append(
            HOST_RECORD,
            serde_json::to_value(HostRecord {
                repo: "repo".into(),
                ..Default::default()
            })
            .unwrap(),
        );
        fixture
    }

    fn append(&self, plugin: &str, body: Value) {
        self.runtime
            .block_on(self.store.append_turn(
                &self.run.0,
                &[Entry::Plugin {
                    plugin: plugin.into(),
                    body: body.to_string(),
                }],
                TurnUsage::default(),
            ))
            .unwrap();
    }

    fn append_codemode(&self, record: store::Record) {
        self.append(PLUGIN, serde_json::to_value(record).unwrap());
    }

    fn pin(&self, definitions: &[Definition]) -> RepositoryPin {
        RepositoryPin {
            owner: self.run.0.to_string(),
            selected: BTreeMap::new(),
            versions: definitions
                .iter()
                .map(|item| (item.version().to_owned(), item.clone()))
                .collect(),
        }
    }

    fn request(
        &self,
        root: Definition,
        dependencies: &[Definition],
    ) -> Request {
        let mut scratch = Library::default();
        scratch
            .apply(&ModuleRecord::Define {
                definition: root.clone(),
            })
            .unwrap();
        self.append_codemode(store::Record::Module(ModuleRecord::Define {
            definition: root.clone(),
        }));
        let pin = self.pin(dependencies);
        if !self.has_pin() {
            self.append_codemode(store::Record::RepositoryPin(pin.clone()));
        }
        let request = Request::capture(
            &self.run.0,
            &self.repository.scope(),
            &self.repository.key(),
            root,
            &scratch,
            &pin,
        )
        .unwrap();
        self.append_codemode(store::Record::Promotion(
            PromotionRecord::Requested(Box::new(request.clone())),
        ));
        request
    }

    fn has_pin(&self) -> bool {
        self.runtime
            .block_on(self.store.records(&self.run.0, PLUGIN))
            .unwrap()
            .iter()
            .any(|body| body.contains("repository_pin"))
    }

    fn cx(&self) -> HostCx {
        HostCx::new(
            self.store.clone(),
            self.runtime.handle().clone(),
            Services::default(),
            self.path.clone(),
            vec![RepoCtx {
                name: "repo".into(),
                checkout: self.path.join("checkout"),
                dir: self.path.clone(),
                workspaces: self.path.clone(),
            }],
            Arc::new(|_| {}),
        )
    }

    fn act(&self, request: &Request, decision: Decision) -> Option<Value> {
        self.runtime
            .block_on(
                CodemodeHost.act(
                    &(),
                    serde_json::to_value(Action::Promote {
                        run: self.run.clone(),
                        request_id: request.id.clone(),
                        decision,
                    })
                    .unwrap(),
                    &self.cx(),
                ),
            )
            .unwrap()
    }

    fn selected(&self) -> BTreeMap<String, String> {
        self.repository
            .snapshot("fresh", &Library::default())
            .unwrap()
            .selected
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn exact_old_request_approves_saved_bytes_after_scratch_replacement() {
    let f = Fixture::new();
    let old = definition("math", "return { n = 1 }", BTreeMap::new());
    let request = f.request(old.clone(), &[]);
    let replacement = definition("math", "return { n = 2 }", BTreeMap::new());
    f.append_codemode(store::Record::Module(ModuleRecord::Define {
        definition: replacement.clone(),
    }));
    assert!(f.act(&request, Decision::Approved).is_none());
    assert_eq!(f.selected()["math"], old.version());
    assert_eq!(
        f.repository
            .snapshot("fresh", &Library::default())
            .unwrap()
            .resolve("math", None)
            .unwrap()
            .source(),
        old.source()
    );
    assert_eq!(
        f.repository
            .snapshot(&f.run.0, &Library::default())
            .unwrap()
            .selected["math"],
        old.version()
    );
    assert!(
        tau_codemode::modules::pin_for_run(
            &f.runtime.block_on(f.cx().records(&f.run, PLUGIN)).unwrap(),
            &f.run.0
        )
        .unwrap()
        .unwrap()
        .selected
        .is_empty()
    );
    assert_eq!(
        f.repository
            .snapshot("fork", &Library::default())
            .unwrap()
            .selected["math"],
        old.version()
    );
    assert!(f.act(&request, Decision::Approved).is_none());
    assert_eq!(f.selected()["math"], old.version());
}

#[test]
fn decline_never_activates_and_cannot_be_reversed() {
    let f = Fixture::new();
    let root = definition("math", "return 1", BTreeMap::new());
    let request = f.request(root, &[]);
    assert!(f.act(&request, Decision::Declined).is_none());
    let before = f
        .runtime
        .block_on(f.store.records(&f.run.0, PLUGIN))
        .unwrap();
    assert!(f.act(&request, Decision::Declined).is_none());
    assert_eq!(
        f.runtime
            .block_on(f.store.records(&f.run.0, PLUGIN))
            .unwrap(),
        before,
        "a repeated decline must not append another terminal record"
    );
    assert!(f.selected().is_empty());
    assert!(
        f.act(&request, Decision::Approved).unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("declined")
    );
    assert!(f.selected().is_empty());
}

#[test]
fn changed_request_source_digest_dependencies_id_owner_and_scope_fail() {
    for changed in ["source", "digest", "dependencies", "id", "owner", "scope"]
    {
        let f = Fixture::new();
        let dependency = definition("base", "return 3", BTreeMap::new());
        let root = definition(
            "math",
            "return require('base')",
            BTreeMap::from([("base".into(), dependency.version().into())]),
        );
        let request = f.request(root, &[dependency]);
        let mut records =
            f.runtime.block_on(f.cx().records(&f.run, PLUGIN)).unwrap();
        let value = records
            .iter_mut()
            .find(|value| value.get("kind") == Some(&json!("promotion")))
            .unwrap();
        match changed {
            "source" => value["root"]["source"] = json!("return 99"),
            "digest" => value["digest"] = json!("0".repeat(64)),
            "dependencies" => {
                value["versions"].as_object_mut().unwrap().clear()
            }
            "id" => value["id"] = json!(uuid::Uuid::now_v7().to_string()),
            "owner" => value["owner"] = json!("foreign"),
            "scope" => value["repository_scope"] = json!("/foreign"),
            _ => unreachable!(),
        }
        let store::Record::Promotion(PromotionRecord::Requested(altered)) =
            serde_json::from_value(value.clone()).unwrap()
        else {
            panic!("expected requested promotion record");
        };
        let altered = *altered;
        let scratch = tau_codemode::modules::fold(&records);
        let pin = tau_codemode::modules::pin_for_run(&records, &f.run.0)
            .unwrap()
            .unwrap();
        assert!(
            altered
                .verify(
                    &scratch,
                    &pin,
                    &f.run.0,
                    &f.repository.scope(),
                    &f.repository.key()
                )
                .is_err(),
            "{changed}"
        );
        assert!(f.selected().is_empty());
        assert_eq!(request.root.name(), "math");
    }
}

#[test]
fn old_approval_replay_cannot_roll_back_newer_grant() {
    let f = Fixture::new();
    let first = f.request(definition("math", "return 1", BTreeMap::new()), &[]);
    assert!(f.act(&first, Decision::Approved).is_none());
    assert!(f.act(&first, Decision::Declined).is_some());
    let second =
        f.request(definition("math", "return 2", BTreeMap::new()), &[]);
    assert!(f.act(&second, Decision::Approved).is_none());
    assert_eq!(f.selected()["math"], second.root.version());
    assert!(f.act(&first, Decision::Approved).is_none());
    assert_eq!(f.selected()["math"], second.root.version());
    assert!(f.act(&first, Decision::Approved).is_none());
    assert_eq!(f.selected()["math"], second.root.version());
}

#[test]
fn concurrent_approvals_preserve_both_aliases() {
    let f = Arc::new(Fixture::new());
    let a = f.request(definition("alpha", "return 1", BTreeMap::new()), &[]);
    let b = f.request(definition("beta", "return 2", BTreeMap::new()), &[]);
    let threads: Vec<_> = [a.clone(), b.clone()]
        .into_iter()
        .map(|request| {
            let f = f.clone();
            std::thread::spawn(move || f.act(&request, Decision::Approved))
        })
        .collect();
    for thread in threads {
        assert!(thread.join().unwrap().is_none());
    }
    let selected = f.selected();
    assert_eq!(selected["alpha"], a.root.version());
    assert_eq!(selected["beta"], b.root.version());
}

#[test]
fn wrong_run_repository_duplicate_and_store_payload_cannot_authorize() {
    let f = Fixture::new();
    let request =
        f.request(definition("math", "return 1", BTreeMap::new()), &[]);
    let forged = store::Writes {
        set: BTreeMap::from([(
            "approval".into(),
            json!({"request_id":&request.id,"decision":"approved"}),
        )]),
        delete: vec![],
    };
    f.append_codemode(store::Record::Store(forged));
    assert!(f.selected().is_empty());
    let wrong_run = serde_json::to_value(Action::Promote {
        run: RunId("foreign".into()),
        request_id: request.id.clone(),
        decision: Decision::Approved,
    })
    .unwrap();
    assert!(
        f.runtime
            .block_on(CodemodeHost.act(&(), wrong_run, &f.cx()))
            .unwrap()
            .is_some()
    );
    let wrong_repo = HostCx::new(
        f.store.clone(),
        f.runtime.handle().clone(),
        Services::default(),
        f.path.clone(),
        vec![RepoCtx {
            name: "repo".into(),
            checkout: f.path.join("checkout"),
            dir: f.path.join("different"),
            workspaces: f.path.join("different"),
        }],
        Arc::new(|_| {}),
    );
    let action = serde_json::to_value(Action::Promote {
        run: f.run.clone(),
        request_id: request.id.clone(),
        decision: Decision::Approved,
    })
    .unwrap();
    assert!(
        f.runtime
            .block_on(CodemodeHost.act(&(), action.clone(), &wrong_repo))
            .unwrap()
            .is_some()
    );
    assert!(f.selected().is_empty());
    f.append_codemode(store::Record::Promotion(PromotionRecord::Requested(
        Box::new(request.clone()),
    )));
    assert!(
        f.runtime
            .block_on(CodemodeHost.act(&(), action, &f.cx()))
            .unwrap()
            .is_some()
    );
    assert!(f.selected().is_empty());
}

#[hegel::test]
fn request_replace_approve_matches_exact_version_oracle(tc: hegel::TestCase) {
    let sequence: Vec<(u8, u8)> = tc.draw(
        gs::vecs(hegel::tuples!(gs::integers::<u8>(), gs::integers::<u8>()))
            .max_size(16),
    );
    let f = Fixture::new();
    let mut requests = Vec::new();
    let mut granted_ids = Vec::<String>::new();
    for (choice, byte) in sequence {
        if choice % 3 == 0 || requests.is_empty() {
            let name = if byte % 2 == 0 { "alpha" } else { "beta" };
            let request = f.request(
                definition(name, &format!("return {byte}"), BTreeMap::new()),
                &[],
            );
            requests.push(request);
        } else {
            let request = &requests[byte as usize % requests.len()];
            if f.act(request, Decision::Approved).is_none()
                && !granted_ids.contains(&request.id)
            {
                granted_ids.push(request.id.clone());
            }
            // The oracle grants each ID once. A replay never changes aliases.
            let mut oracle = BTreeMap::new();
            for id in &granted_ids {
                let granted =
                    requests.iter().find(|item| &item.id == id).unwrap();
                oracle.insert(
                    granted.root.name().to_owned(),
                    granted.root.version().to_owned(),
                );
            }
            assert_eq!(f.selected(), oracle);
        }
    }
}
