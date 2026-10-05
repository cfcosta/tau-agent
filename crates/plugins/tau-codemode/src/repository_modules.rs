//! Host-owned immutable repository module versions and selected aliases.
//! No script tool reaches this store or its trusted activation operation.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    modules::{self, Definition, Library, RepositoryPin},
    promotion::{self, Request},
};

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
// A 64 KiB UTF-8 source can expand sixfold in JSON. Add 16 KiB of
// signatures, 32 bounded dependencies, and the definition envelope.
const MAX_VERSION_FILE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryManifest {
    selected: BTreeMap<String, String>,
    approved_requests: BTreeMap<String, ApprovalReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalReceipt {
    owner: String,
    digest: String,
}

pub(crate) struct ManifestLock(File);

impl Drop for ManifestLock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// The directory is tau's private per-repository directory, outside its checkout.
#[derive(Debug, Clone)]
pub struct RepositoryModules {
    dir: PathBuf,
}

impl RepositoryModules {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn scope(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }

    pub fn key(&self) -> String {
        promotion::repository_key(&self.dir)
    }

    fn versions_dir(&self) -> PathBuf {
        self.dir.join("versions")
    }

    fn version_path(&self, version: &str) -> Result<PathBuf, String> {
        if !modules::valid_version(version) {
            return Err("invalid repository version digest".into());
        }
        Ok(self.versions_dir().join(version))
    }

    /// Store a canonical definition without selecting it. Existing bytes are
    /// verified and never replaced, including on a same-digest retry.
    pub fn stage(&self, definition: &Definition) -> Result<(), String> {
        definition.verify()?;
        let bytes = serde_json::to_vec(definition)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_VERSION_FILE_BYTES {
            return Err("repository version exceeds 512 KiB".into());
        }
        fs::create_dir_all(self.versions_dir())
            .map_err(|error| error.to_string())?;
        let path = self.version_path(definition.version())?;
        if path.exists() {
            return self.verify_existing(&path, definition);
        }
        let temporary = self
            .versions_dir()
            .join(format!(".stage-{}", uuid::Uuid::now_v7()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            file.write_all(&bytes).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            match fs::hard_link(&temporary, &path) {
                Ok(()) => Ok(()),
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists =>
                {
                    self.verify_existing(&path, definition)
                }
                Err(error) => Err(error.to_string()),
            }
        })();
        let _ = fs::remove_file(&temporary);
        result?;
        File::open(self.versions_dir())
            .and_then(|dir| dir.sync_all())
            .map_err(|error| error.to_string())
    }

    fn verify_existing(
        &self,
        path: &Path,
        expected: &Definition,
    ) -> Result<(), String> {
        let actual = self.read_version(expected.name(), expected.version())?;
        if &actual != expected {
            return Err(
                "existing repository version has different content".into()
            );
        }
        if fs::read(path).map_err(|error| error.to_string())?
            != serde_json::to_vec(expected)
                .map_err(|error| error.to_string())?
        {
            return Err(
                "existing repository version is not canonical JSON".into()
            );
        }
        Ok(())
    }

    fn read_version(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Definition, String> {
        modules::validate_name(name)?;
        let path = self.version_path(version)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!("repository module {name}@{version} is missing: {error}")
        })?;
        if !metadata.file_type().is_file()
            || metadata.len() > MAX_VERSION_FILE_BYTES
        {
            return Err(format!(
                "repository module {name}@{version} is not a bounded regular file"
            ));
        }
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_VERSION_FILE_BYTES {
            return Err(format!(
                "repository module {name}@{version} is not a bounded regular file"
            ));
        }
        let definition: Definition =
            serde_json::from_slice(&bytes).map_err(|error| {
                format!(
                    "repository module {name}@{version} is corrupt: {error}"
                )
            })?;
        definition.verify()?;
        if definition.name() != name || definition.version() != version {
            return Err(format!(
                "repository module {name}@{version} has a name or version mismatch"
            ));
        }
        Ok(definition)
    }

    fn read_manifest(&self) -> Result<RepositoryManifest, String> {
        let path = self.dir.join("selected.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RepositoryManifest::default());
            }
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.file_type().is_file()
            || metadata.len() > MAX_MANIFEST_BYTES
        {
            return Err(
                "repository selection manifest is not a bounded regular file"
                    .into(),
            );
        }
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("repository selection manifest exceeds 64 KiB".into());
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| {
                format!("repository selection manifest is corrupt: {error}")
            })?;
        let manifest = if value
            .get("selected")
            .is_some_and(serde_json::Value::is_object)
        {
            serde_json::from_slice::<RepositoryManifest>(&bytes).map_err(
                |error| {
                    format!("repository selection manifest is corrupt: {error}")
                },
            )?
        } else {
            RepositoryManifest {
                selected: serde_json::from_slice(&bytes).map_err(|error| {
                    format!("repository selection manifest is corrupt: {error}")
                })?,
                ..Default::default()
            }
        };
        Self::validate_selected(&manifest.selected)?;
        if manifest.approved_requests.len() > promotion::MAX_RECEIPTS {
            return Err("repository approval receipts exceed quota".into());
        }
        for (id, receipt) in &manifest.approved_requests {
            if uuid::Uuid::parse_str(id).is_err()
                || receipt.owner.is_empty()
                || !modules::valid_version(&receipt.digest)
            {
                return Err("invalid repository approval receipt".into());
            }
        }
        Ok(manifest)
    }

    fn read_selected(&self) -> Result<BTreeMap<String, String>, String> {
        Ok(self.read_manifest()?.selected)
    }

    fn validate_selected(
        selected: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        if selected.len() > modules::MAX_PINNED_VERSIONS {
            return Err("repository selection has too many aliases".into());
        }
        for (name, version) in selected {
            modules::validate_name(name)?;
            if !modules::valid_version(version) {
                return Err(format!(
                    "invalid repository selection version for {name}"
                ));
            }
        }
        Ok(())
    }

    fn serialize_manifest(
        manifest: &RepositoryManifest,
    ) -> Result<Vec<u8>, String> {
        Self::validate_selected(&manifest.selected)?;
        if manifest.approved_requests.len() > promotion::MAX_RECEIPTS {
            return Err("repository approval receipts exceed quota".into());
        }
        let bytes =
            serde_json::to_vec(manifest).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("repository selection manifest exceeds 64 KiB".into());
        }
        Ok(bytes)
    }

    pub(crate) fn lock_manifest(&self) -> Result<ManifestLock, String> {
        fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.dir.join(".selected.lock"))
            .map_err(|error| error.to_string())?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(ManifestLock(file))
    }

    fn write_manifest(
        &self,
        manifest: &RepositoryManifest,
    ) -> Result<(), String> {
        let bytes = Self::serialize_manifest(manifest)?;
        let temporary =
            self.dir.join(format!(".selected-{}", uuid::Uuid::now_v7()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            file.write_all(&bytes).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            fs::rename(&temporary, self.dir.join("selected.json"))
                .map_err(|error| error.to_string())
        })();
        let _ = fs::remove_file(&temporary);
        result?;
        File::open(&self.dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| error.to_string())
    }

    /// Stage exact versions, then atomically select the root and retain its
    /// receipt. An identical replay cannot change a later selection.
    #[cfg(test)]
    fn approve(&self, request: &Request) -> Result<bool, String> {
        let lock = self.lock_manifest()?;
        self.approve_with_lock(request, &lock)
    }

    pub(crate) fn approve_with_lock(
        &self,
        request: &Request,
        _lock: &ManifestLock,
    ) -> Result<bool, String> {
        let mut manifest = self.read_manifest()?;
        let receipt = ApprovalReceipt {
            owner: request.owner.clone(),
            digest: request.digest.clone(),
        };
        if let Some(existing) = manifest.approved_requests.get(&request.id) {
            if existing != &receipt {
                return Err("repository receipt conflicts with request".into());
            }
            return Ok(false);
        }
        for definition in request.versions.values() {
            self.stage(definition)?;
        }
        self.read_version(request.root.name(), request.root.version())?;
        manifest.selected.insert(
            request.root.name().to_owned(),
            request.root.version().to_owned(),
        );
        manifest
            .approved_requests
            .insert(request.id.clone(), receipt);
        self.write_manifest(&manifest)?;
        Ok(true)
    }

    pub(crate) fn has_receipt_with_lock(
        &self,
        request: &Request,
        _lock: &ManifestLock,
    ) -> Result<bool, String> {
        let receipt = ApprovalReceipt {
            owner: request.owner.clone(),
            digest: request.digest.clone(),
        };
        match self.read_manifest()?.approved_requests.get(&request.id) {
            Some(existing) if existing == &receipt => Ok(true),
            Some(_) => Err("repository receipt conflicts with request".into()),
            None => Ok(false),
        }
    }

    #[cfg(test)]
    pub(crate) fn activate(
        &self,
        name: &str,
        version: &str,
    ) -> Result<(), String> {
        self.read_version(name, version)?;
        let _lock = self.lock_manifest()?;
        let mut manifest = self.read_manifest()?;
        manifest
            .selected
            .insert(name.to_owned(), version.to_owned());
        self.write_manifest(&manifest)
    }

    /// Capture selected aliases and exact dependency closure. Dependencies of
    /// inherited scratch definitions remain available after a fork repins.
    pub fn snapshot(
        &self,
        owner: &str,
        scratch: &Library,
    ) -> Result<RepositoryPin, String> {
        let selected = self.read_selected()?;
        let mut pin = RepositoryPin {
            owner: owner.to_owned(),
            selected,
            versions: BTreeMap::new(),
        };
        let mut visiting = BTreeSet::new();
        for (name, version) in pin.selected.clone() {
            self.capture_version_and_dependencies(
                &name,
                &version,
                &mut pin,
                &mut visiting,
            )?;
        }
        for definition in scratch.versions().values() {
            for (name, version) in definition.dependencies() {
                if scratch
                    .versions()
                    .get(version)
                    .is_none_or(|dependency| dependency.name() != name)
                {
                    self.capture_version_and_dependencies(
                        name,
                        version,
                        &mut pin,
                        &mut visiting,
                    )?;
                }
            }
        }
        pin.verify()?;
        Ok(pin)
    }

    fn capture_version_and_dependencies(
        &self,
        name: &str,
        version: &str,
        pin: &mut RepositoryPin,
        visiting: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        if let Some(existing) = pin.versions.get(version) {
            return if existing.name() == name {
                Ok(())
            } else {
                Err("repository dependency name mismatch".into())
            };
        }
        if !visiting.insert(version.to_owned()) {
            return Err(format!(
                "repository dependency cycle at {name}@{version}"
            ));
        }
        if visiting.len() + pin.versions.len() > modules::MAX_PINNED_VERSIONS {
            return Err("repository pin has too many versions".into());
        }
        let definition = self.read_version(name, version)?;
        for (dependency, digest) in definition.dependencies() {
            self.capture_version_and_dependencies(
                dependency, digest, pin, visiting,
            )?;
        }
        visiting.remove(version);
        pin.versions.insert(version.to_owned(), definition);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use async_trait::async_trait;
    use serde_json::{Value, json};
    use tau_agent::{
        agent::Agent,
        error::PluginError,
        plugin::{Decision, Plugin, PluginCtx, PluginRun, RunPlan, ToolCall},
    };
    use tau_ai::message::{InputBlock, Message};
    use tau_store::{Entry, Store};
    use tau_testing::{block_on_io, scripted::ScriptedModel};

    use super::*;

    struct Fixture {
        path: PathBuf,
        repository: RepositoryModules,
    }

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("codemode-repository-{}", uuid::Uuid::now_v7()));
            Self {
                repository: RepositoryModules::new(path.clone()),
                path,
            }
        }

        fn install(&self, definition: &Definition) {
            self.repository.stage(definition).unwrap();
            self.repository
                .activate(definition.name(), definition.version())
                .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn definition(
        name: &str,
        source: &str,
        dependencies: BTreeMap<String, String>,
    ) -> Definition {
        Definition::new(name.into(), source.into(), json!({}), dependencies)
            .unwrap()
    }

    #[test]
    fn receipt_replay_preserves_later_alias_after_missing_terminal() {
        let fixture = Fixture::new();
        let mut scratch = Library::default();
        let pin = RepositoryPin {
            owner: "run".into(),
            selected: BTreeMap::new(),
            versions: BTreeMap::new(),
        };
        let mut requests = Vec::new();
        for source in ["return 1", "return 2"] {
            let root = definition("math", source, BTreeMap::new());
            scratch
                .apply(&crate::modules::Record::Define {
                    definition: root.clone(),
                })
                .unwrap();
            requests.push(
                Request::capture(
                    "run",
                    &fixture.repository.scope(),
                    &fixture.repository.key(),
                    root,
                    &scratch,
                    &pin,
                )
                .unwrap(),
            );
        }
        // The first activation has no Store terminal. The second request
        // selects a newer alias before the first receipt is retried.
        assert!(fixture.repository.approve(&requests[0]).unwrap());
        assert!(fixture.repository.approve(&requests[1]).unwrap());
        assert!(!fixture.repository.approve(&requests[0]).unwrap());
        assert_eq!(
            fixture.repository.read_selected().unwrap()["math"],
            requests[1].root.version()
        );
        let lock = fixture.repository.lock_manifest().unwrap();
        assert!(
            fixture
                .repository
                .has_receipt_with_lock(&requests[0], &lock)
                .unwrap()
        );
    }

    #[test]
    fn staged_version_is_atomic_private_and_never_replaced() {
        let fixture = Fixture::new();
        let old = definition("m", "return { n = 1 }", BTreeMap::new());
        fixture.repository.stage(&old).unwrap();
        assert!(
            fixture.repository.read_selected().unwrap().is_empty(),
            "stage does not activate"
        );
        let path = fixture.repository.version_path(old.version()).unwrap();
        let original = fs::read(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fixture.repository.stage(&old).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        fixture.repository.activate("m", old.version()).unwrap();
        assert_eq!(
            fixture.repository.read_selected().unwrap()["m"],
            old.version()
        );
        fs::write(&path, b"corrupt").unwrap();
        assert!(
            fixture
                .repository
                .stage(&old)
                .unwrap_err()
                .contains("corrupt")
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            b"corrupt",
            "stage cannot repair or clobber an existing version"
        );
        assert!(
            fixture
                .repository
                .snapshot("run", &Library::default())
                .is_err()
        );
    }

    #[test]
    fn snapshot_captures_exact_dependencies_and_old_scratch_pins() {
        let fixture = Fixture::new();
        let old = definition("base", "return { n = 1 }", BTreeMap::new());
        let latest = definition("base", "return { n = 2 }", BTreeMap::new());
        let parent = definition(
            "parent",
            "return require('base')",
            BTreeMap::from([("base".into(), old.version().into())]),
        );
        let scratch = definition(
            "scratch",
            "return require('base')",
            BTreeMap::from([("base".into(), old.version().into())]),
        );
        fixture.install(&old);
        fixture.repository.stage(&latest).unwrap();
        fixture.repository.stage(&parent).unwrap();
        fixture
            .repository
            .activate("base", latest.version())
            .unwrap();
        fixture
            .repository
            .activate("parent", parent.version())
            .unwrap();
        let mut library = Library::default();
        library
            .apply(&crate::modules::Record::Define {
                definition: scratch,
            })
            .unwrap();
        let pin = fixture.repository.snapshot("new-run", &library).unwrap();
        assert_eq!(pin.selected["base"], latest.version());
        assert!(pin.versions.contains_key(old.version()));
        assert!(pin.versions.contains_key(latest.version()));
        assert!(pin.versions.contains_key(parent.version()));
        assert_eq!(pin.resolve("base", Some(old.version())).unwrap(), &old);
        fs::remove_file(
            fixture.repository.version_path(old.version()).unwrap(),
        )
        .unwrap();
        assert!(
            fixture
                .repository
                .snapshot("another-run", &library)
                .unwrap_err()
                .contains("missing")
        );
    }

    #[test]
    fn wrong_version_or_name_in_digest_file_is_rejected() {
        let fixture = Fixture::new();
        let original = definition("m", "return 1", BTreeMap::new());
        let other = definition("other", "return 1", BTreeMap::new());
        fixture.install(&original);
        let path = fixture.repository.version_path(original.version()).unwrap();
        fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
        assert!(
            fixture
                .repository
                .snapshot("run", &Library::default())
                .unwrap_err()
                .contains("mismatch")
        );
    }

    #[test]
    fn escape_heavy_source_roundtrips_at_unescaped_limit() {
        let fixture = Fixture::new();
        let source = "\u{0001}".repeat(modules::MAX_SOURCE_BYTES);
        let large = definition("escaped", &source, BTreeMap::new());
        let bytes = serde_json::to_vec(&large).unwrap();
        assert!(bytes.len() > 96 * 1024);
        assert!(bytes.len() <= MAX_VERSION_FILE_BYTES as usize);
        fixture.install(&large);
        assert_eq!(
            fixture
                .repository
                .read_version("escaped", large.version())
                .unwrap(),
            large
        );
        let pin = fixture
            .repository
            .snapshot("run", &Library::default())
            .unwrap();
        assert_eq!(pin.resolve("escaped", None).unwrap().source(), source);
        assert!(
            Definition::new(
                "too_large".into(),
                format!("{source}x"),
                json!({}),
                BTreeMap::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn dependency_closure_accepts_cap_and_rejects_next_node_before_read() {
        let fixture = Fixture::new();
        let mut next: Option<Definition> = None;
        let mut definitions = Vec::new();
        for index in (0..=modules::MAX_PINNED_VERSIONS).rev() {
            let dependencies =
                next.as_ref().map_or_else(BTreeMap::new, |child| {
                    BTreeMap::from([(
                        child.name().to_owned(),
                        child.version().to_owned(),
                    )])
                });
            let current =
                definition(&format!("node_{index}"), "return 1", dependencies);
            fixture.repository.stage(&current).unwrap();
            next = Some(current.clone());
            definitions.push(current);
        }
        let at_cap = &definitions[modules::MAX_PINNED_VERSIONS - 1];
        fixture
            .repository
            .activate(at_cap.name(), at_cap.version())
            .unwrap();
        let pin = fixture
            .repository
            .snapshot("run", &Library::default())
            .unwrap();
        assert_eq!(pin.versions.len(), modules::MAX_PINNED_VERSIONS);

        let over_cap = definitions.last().unwrap();
        fixture
            .repository
            .activate(over_cap.name(), over_cap.version())
            .unwrap();
        let deepest = &definitions[0];
        fs::remove_file(
            fixture.repository.version_path(deepest.version()).unwrap(),
        )
        .unwrap();
        assert!(
            fixture
                .repository
                .snapshot("run", &Library::default())
                .unwrap_err()
                .contains("too many versions")
        );
    }

    #[test]
    fn activation_rejects_alias_over_cap_without_publishing() {
        let fixture = Fixture::new();
        let mut selected = BTreeMap::new();
        for index in 0..=modules::MAX_PINNED_VERSIONS {
            let name = format!("alias_{index}");
            let current = definition(&name, "return 1", BTreeMap::new());
            fixture.repository.stage(&current).unwrap();
            if index < modules::MAX_PINNED_VERSIONS {
                selected.insert(name, current.version().to_owned());
            } else {
                fs::write(
                    fixture.path.join("selected.json"),
                    serde_json::to_vec(&selected).unwrap(),
                )
                .unwrap();
                let before =
                    fs::read(fixture.path.join("selected.json")).unwrap();
                assert!(
                    fixture
                        .repository
                        .activate(&name, current.version())
                        .unwrap_err()
                        .contains("too many aliases")
                );
                assert_eq!(
                    fs::read(fixture.path.join("selected.json")).unwrap(),
                    before
                );
            }
        }
        selected.insert("extra".into(), "0".repeat(64));
        fs::write(
            fixture.path.join("selected.json"),
            serde_json::to_vec(&selected).unwrap(),
        )
        .unwrap();
        assert!(
            fixture
                .repository
                .read_selected()
                .unwrap_err()
                .contains("too many aliases")
        );
    }

    fn scripted(scripts: &[&str]) -> ScriptedModel {
        let mut model = ScriptedModel::new();
        for source in scripts {
            let source = source.to_string();
            model = model
                .turn(move |turn| {
                    turn.tool_call("codemode", json!({"code": source}))
                })
                .turn(|turn| turn.text("done"));
        }
        model
    }

    async fn last_value(store: &Store, run: &str) -> Value {
        let result = store
            .transcript(run)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|entry| {
                let Entry::Message { body, .. } = entry else {
                    return None;
                };
                let Message::ToolResult(result) =
                    serde_json::from_str(&body).unwrap()
                else {
                    return None;
                };
                Some(result)
            })
            .next_back()
            .unwrap();
        assert!(!result.is_error, "{result:?}");
        let InputBlock::Text(text) = &result.content[1] else {
            panic!("expected text")
        };
        serde_json::from_str(&text.text).unwrap_or_else(|_| json!(text.text))
    }

    #[derive(Clone)]
    struct ActivateStagedFixture {
        repository: RepositoryModules,
        version: String,
    }

    #[async_trait]
    impl Plugin for ActivateStagedFixture {
        fn name(&self) -> &str {
            "activate-staged-fixture"
        }
        async fn start(
            &self,
            _: &mut RunPlan,
            _: &PluginCtx,
        ) -> Result<Box<dyn PluginRun>, PluginError> {
            Ok(Box::new(self.clone()))
        }
    }

    #[async_trait]
    impl PluginRun for ActivateStagedFixture {
        async fn before_tool(
            &mut self,
            call: &mut ToolCall,
            _: &PluginCtx,
        ) -> Result<Decision, PluginError> {
            if call.name == "codemode" {
                self.repository
                    .activate("m", &self.version)
                    .map_err(PluginError::from)?;
            }
            Ok(Decision::Allow)
        }
    }

    #[test]
    fn resume_keeps_pin_while_fork_and_new_run_see_new_alias() {
        block_on_io(async {
            let fixture = Fixture::new();
            let old = definition("m", "return { n = 1 }", BTreeMap::new());
            let new = definition("m", "return { n = 2 }", BTreeMap::new());
            fixture.install(&old);
            fixture.repository.stage(&new).unwrap();
            let store = tau_store_sqlite::memory().await.unwrap();
            let agent = Agent::new(scripted(&[
                "return {loaded=require('m').n,list=tools.module_list({}),inspect=tools.module_inspect({name='m'})}",
                "return {loaded=require('m').n,list=tools.module_list({}),inspect=tools.module_inspect({name='m'})}",
                "return {loaded=require('m').n,list=tools.module_list({}),inspect=tools.module_inspect({name='m'})}",
                "tools.module_define({name='m',source='return { n = 7 }'}); return {loaded=require('m').n,list=tools.module_list({}),inspect=tools.module_inspect({name='m'})}",
                "return {loaded=require('m').n,list=tools.module_list({}),inspect=tools.module_inspect({name='m'})}",
            ])).plugin(crate::Codemode::new(None).with_repository(fixture.path.clone()))
            .plugin(ActivateStagedFixture {
                repository: fixture.repository.clone(),
                version: new.version().into(),
            });
            let root = agent.run("root", &store).await.unwrap();
            let root_value = last_value(&store, &root.run.0).await;
            assert_eq!(root_value["loaded"], 1);
            assert_eq!(root_value["list"][0]["version"], old.version());
            assert_eq!(root_value["inspect"]["source"], old.source());
            assert_eq!(root_value["inspect"]["tests"], json!([]));
            assert_eq!(
                fixture.repository.read_selected().unwrap()["m"],
                new.version()
            );
            let resumed =
                agent.resume(&root.run).run("again", &store).await.unwrap();
            assert_eq!(resumed.run, root.run);
            let resumed_value = last_value(&store, &root.run.0).await;
            assert_eq!(resumed_value["loaded"], 1);
            assert_eq!(resumed_value["list"][0]["version"], old.version());
            let fork = agent
                .fork(&root.checkpoint())
                .run("fork", &store)
                .await
                .unwrap();
            let fork_value = last_value(&store, &fork.run.0).await;
            assert_eq!(fork_value["loaded"], 2);
            assert_eq!(fork_value["list"][0]["version"], new.version());
            assert_eq!(fork_value["inspect"]["source"], new.source());
            let overridden = agent
                .fork(&root.checkpoint())
                .run("override", &store)
                .await
                .unwrap();
            let fork_value = last_value(&store, &overridden.run.0).await;
            assert_eq!(fork_value["loaded"], 7);
            assert_eq!(
                fork_value["list"][0]["version"],
                fork_value["inspect"]["version"]
            );
            assert_eq!(fork_value["inspect"]["source"], "return { n = 7 }");
            let fresh = agent.run("fresh", &store).await.unwrap();
            let fresh_value = last_value(&store, &fresh.run.0).await;
            assert_eq!(fresh_value["loaded"], 2);
            assert_eq!(fresh_value["list"][0]["version"], new.version());
            assert_eq!(fresh_value["inspect"]["source"], new.source());
            let records: Vec<Value> = store
                .records(&fork.run.0, crate::PLUGIN)
                .await
                .unwrap()
                .into_iter()
                .map(|body| serde_json::from_str(&body).unwrap())
                .collect();
            let root_pin = crate::modules::pin_for_run(&records, &root.run.0)
                .unwrap()
                .unwrap();
            let fork_pin = crate::modules::pin_for_run(&records, &fork.run.0)
                .unwrap()
                .unwrap();
            assert_eq!(root_pin.selected["m"], old.version());
            assert_eq!(fork_pin.selected["m"], new.version());
            assert_eq!(crate::modules::fold(&records).versions().len(), 0);
        });
    }

    #[test]
    fn scratch_test_initializes_exact_repository_dependency() {
        block_on_io(async {
            let fixture = Fixture::new();
            let dependency =
                definition("repo_dep", "return { n = 42 }", BTreeMap::new());
            fixture.install(&dependency);
            let script = format!(
                r#"
local root = tools.module_define({{
    name='scratch_root',
    source="return {{ n = require('repo_dep').n }}",
    dependencies={{repo_dep='{version}'}}
}})
local report = tools.module_test({{
    name='scratch_root',
    code="assert(require('scratch_root').n == 42)"
}})
local repository_root_allowed = pcall(tools.module_test, {{name='repo_dep',code='return true'}})
return {{report=report, repository_root_allowed=repository_root_allowed, inspected=tools.module_inspect({{name='repo_dep',version='{version}'}}), selected=tools.module_list({{}})}}
"#,
                version = dependency.version()
            );
            let store = tau_store_sqlite::memory().await.unwrap();
            let agent = Agent::new(scripted(&[&script])).plugin(
                crate::Codemode::new(None).with_repository(&fixture.path),
            );
            let run = agent.run("test", &store).await.unwrap();
            let value = last_value(&store, &run.run.0).await;
            assert_eq!(value["report"]["passed"], true, "{value}");
            assert_eq!(value["repository_root_allowed"], false);
            assert_eq!(value["inspected"]["source"], dependency.source());
            assert_eq!(value["inspected"]["tests"], json!([]));
            assert_eq!(value["selected"].as_array().unwrap().len(), 2);
            let records: Vec<Value> = store
                .records(&run.run.0, crate::PLUGIN)
                .await
                .unwrap()
                .into_iter()
                .map(|body| serde_json::from_str(&body).unwrap())
                .collect();
            let scratch = crate::modules::fold(&records);
            assert_eq!(scratch.versions().len(), 1);
            assert!(!scratch.versions().contains_key(dependency.version()));
            assert_eq!(
                scratch
                    .tests(value["report"]["version"].as_str().unwrap())
                    .len(),
                1
            );
        });
    }

    #[test]
    fn script_cannot_import_repository_files_by_path() {
        block_on_io(async {
            let fixture = Fixture::new();
            fixture.install(&definition(
                "m",
                "return { n = 3 }",
                BTreeMap::new(),
            ));
            let store = tau_store_sqlite::memory().await.unwrap();
            let agent = Agent::new(scripted(&[
                "local ok = pcall(require, '../selected.json'); return ok",
            ]))
            .plugin(crate::Codemode::new(None).with_repository(&fixture.path));
            let run = agent.run("path", &store).await.unwrap();
            assert_eq!(last_value(&store, &run.run.0).await, json!(false));
        });
    }
}
