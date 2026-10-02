//! Immutable codemode module definitions and selected versions.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
pub const MAX_SIGNATURES_BYTES: usize = 16 * 1024;
pub const MAX_DEPENDENCIES: usize = 32;
pub const MAX_REGISTERED_BYTES: usize = 1024 * 1024;
pub const MAX_VERSIONS: usize = 128;
/// Repository pins have their own quota, independent of scratch modules.
pub const MAX_PINNED_VERSIONS: usize = 256;
pub const MAX_PINNED_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TEST_CODE_BYTES: usize = 64 * 1024;
pub const MAX_TEST_FIXTURES: usize = 128;
pub const MAX_TEST_FIXTURE_BYTES: usize = 1024 * 1024;
pub const MAX_TEST_REPORT_BYTES: usize = 256 * 1024;
pub const MAX_TEST_RECORDS: usize = 128;
pub const MAX_TEST_RECORD_BYTES: usize = 8 * 1024 * 1024;

/// A definition's content fixes its version. Fields are private so callers
/// cannot change content while retaining the old digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    name: String,
    version: String,
    source: String,
    signatures: Value,
    dependencies: BTreeMap<String, String>,
}

impl Definition {
    pub fn new(
        name: String,
        source: String,
        signatures: Value,
        dependencies: BTreeMap<String, String>,
    ) -> Result<Self, String> {
        validate_name(&name)?;
        if source.len() > MAX_SOURCE_BYTES {
            return Err("module source exceeds 64 KiB".into());
        }
        let signatures = normalize(signatures);
        if serde_json::to_vec(&signatures)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_SIGNATURES_BYTES
        {
            return Err("module signatures exceed 16 KiB".into());
        }
        if dependencies.len() > MAX_DEPENDENCIES {
            return Err("module has more than 32 dependencies".into());
        }
        for (name, version) in &dependencies {
            validate_name(name)?;
            if !valid_version(version) {
                return Err(format!("invalid dependency version for {name}"));
            }
        }
        let canonical =
            serde_json::to_vec(&(&name, &source, &signatures, &dependencies))
                .map_err(|error| error.to_string())?;
        let version = Sha256::digest(canonical)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self {
            name,
            version,
            source,
            signatures,
            dependencies,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn signatures(&self) -> &Value {
        &self.signatures
    }
    pub fn dependencies(&self) -> &BTreeMap<String, String> {
        &self.dependencies
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        let rebuilt = Self::new(
            self.name.clone(),
            self.source.clone(),
            self.signatures.clone(),
            self.dependencies.clone(),
        )?;
        if rebuilt.version != self.version || !valid_version(&self.version) {
            return Err("module version does not match its content".into());
        }
        Ok(())
    }
}

pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let valid_start = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if name.len() > 64
        || !valid_start
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("module name must be a 1..64 character identifier".into());
    }
    Ok(())
}

pub(crate) fn valid_version(version: &str) -> bool {
    version.len() == 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn normalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map
                .into_iter()
                .map(|(key, value)| (key, normalize(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => {
            Value::Array(values.into_iter().map(normalize).collect())
        }
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Record {
    Define { definition: Definition },
    Select { name: String, version: String },
    Test { test: ModuleTest },
}

/// One fake call expected by a test, in invocation order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedCall {
    pub name: String,
    pub args: Value,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn deserialize_present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    // Omitted means no readable value; explicit JSON null is still a value.
    Value::deserialize(deserializer).map(Some)
}

/// The bounded result retained for exact-version inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestReport {
    pub name: String,
    pub version: String,
    pub passed: bool,
    pub output: String,
    #[serde(default)]
    pub output_truncated: bool,
    pub calls: Vec<Value>,
    pub error: Option<String>,
    #[serde(default)]
    pub error_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleTest {
    name: String,
    version: String,
    code: String,
    tools: Vec<ExpectedCall>,
    result: TestReport,
}

impl ModuleTest {
    pub fn new(
        name: String,
        version: String,
        code: String,
        tools: Vec<ExpectedCall>,
        result: TestReport,
    ) -> Result<Self, String> {
        validate_name(&name)?;
        if !valid_version(&version)
            || result.name != name
            || result.version != version
        {
            return Err("module test names an invalid version or report".into());
        }
        if code.len() > MAX_TEST_CODE_BYTES {
            return Err("module test code exceeds 64 KiB".into());
        }
        if tools.len() > MAX_TEST_FIXTURES {
            return Err("module test has more than 128 fake calls".into());
        }
        for call in &tools {
            validate_name(&call.name)?;
            if !call.args.is_object() {
                return Err("module test fake args must be an object".into());
            }
        }
        if serde_json::to_vec(&tools)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_TEST_FIXTURE_BYTES
        {
            return Err("module test fixtures exceed 1 MiB".into());
        }
        if result.output.len() > 64 * 1024
            || serde_json::to_vec(&result)
                .map_err(|error| error.to_string())?
                .len()
                > MAX_TEST_REPORT_BYTES
        {
            return Err("module test report exceeds its bound".into());
        }
        Ok(Self {
            name,
            version,
            code,
            tools,
            result,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn result(&self) -> &TestReport {
        &self.result
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn tools(&self) -> &[ExpectedCall] {
        &self.tools
    }

    fn verify(&self) -> Result<(), String> {
        Self::new(
            self.name.clone(),
            self.version.clone(),
            self.code.clone(),
            self.tools.clone(),
            self.result.clone(),
        )
        .map(|_| ())
    }
}

/// Registered definitions remain available after selection changes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    versions: BTreeMap<String, Definition>,
    selected: BTreeMap<String, String>,
    tests: BTreeMap<String, Vec<ModuleTest>>,
}

pub type Snapshot = Library;

/// Immutable host-approved definitions captured for one run. The full source
/// is persisted so UI and phone folds do not need repository filesystem access.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepositoryPin {
    pub owner: String,
    pub selected: BTreeMap<String, String>,
    pub versions: BTreeMap<String, Definition>,
}

impl RepositoryPin {
    pub fn verify(&self) -> Result<(), String> {
        if self.owner.is_empty() || self.versions.len() > MAX_PINNED_VERSIONS {
            return Err(
                "repository pin has an invalid owner or too many versions"
                    .into(),
            );
        }
        let mut bytes = 0usize;
        for (version, definition) in &self.versions {
            definition.verify()?;
            if version != definition.version() {
                return Err(
                    "repository pin version key differs from content".into()
                );
            }
            bytes += definition_bytes(definition);
            if bytes > MAX_PINNED_BYTES {
                return Err("repository pin exceeds 16 MiB".into());
            }
        }
        for (name, version) in &self.selected {
            validate_name(name)?;
            if self
                .versions
                .get(version)
                .is_none_or(|definition| definition.name() != name)
            {
                return Err(format!(
                    "repository selection {name}@{version} is missing or mismatched"
                ));
            }
        }
        for definition in self.versions.values() {
            for (name, version) in definition.dependencies() {
                if self
                    .versions
                    .get(version)
                    .is_none_or(|dependency| dependency.name() != name)
                {
                    return Err(format!(
                        "repository dependency {name}@{version} is missing or mismatched"
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Option<&Definition> {
        let version =
            version.or_else(|| self.selected.get(name).map(String::as_str))?;
        self.versions
            .get(version)
            .filter(|definition| definition.name() == name)
    }
}

/// Only a pin owned by this run is active. An inherited pin belongs to its
/// original run and must not override a fork's newly selected aliases.
pub fn pin_for_run(
    records: &[Value],
    owner: &str,
) -> Result<Option<RepositoryPin>, String> {
    let mut found = None;
    for value in records {
        if value.get("kind").and_then(Value::as_str) != Some("repository_pin") {
            continue;
        }
        let pin_owner = value
            .get("owner")
            .and_then(Value::as_str)
            .filter(|owner| !owner.is_empty())
            .ok_or("corrupt repository pin owner")?;
        if pin_owner != owner {
            continue;
        }
        let crate::store::Record::RepositoryPin(pin) =
            serde_json::from_value(value.clone())
                .map_err(|error| format!("corrupt repository pin: {error}"))?
        else {
            return Err("corrupt repository pin record".into());
        };
        pin.verify()?;
        if found.as_ref().is_some_and(|previous| previous != &pin) {
            return Err("conflicting repository pins for run".into());
        }
        found = Some(pin);
    }
    Ok(found)
}

/// Resolve a visible module with the same scratch-first precedence as `require`.
/// The caller supplies only the pin owned by the current run.
pub fn resolve_visible<'a>(
    scratch: &'a Library,
    pin: Option<&'a RepositoryPin>,
    name: &str,
    version: Option<&str>,
) -> Option<&'a Definition> {
    let scratch_version =
        version.or_else(|| scratch.selected().get(name).map(String::as_str));
    scratch_version
        .and_then(|version| scratch.versions().get(version))
        .filter(|definition| definition.name() == name)
        .or_else(|| pin.and_then(|pin| pin.resolve(name, version)))
}

impl Library {
    pub fn versions(&self) -> &BTreeMap<String, Definition> {
        &self.versions
    }
    pub fn selected(&self) -> &BTreeMap<String, String> {
        &self.selected
    }
    pub fn tests(&self, version: &str) -> &[ModuleTest] {
        self.tests.get(version).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn apply(&mut self, record: &Record) -> Result<(), String> {
        match record {
            Record::Define { definition } => {
                definition.verify()?;
                if let Some(existing) = self.versions.get(definition.version())
                {
                    if existing != definition {
                        return Err(
                            "module version already has different content"
                                .into(),
                        );
                    }
                } else {
                    if self.versions.len() >= MAX_VERSIONS {
                        return Err("module library has 128 versions".into());
                    }
                    let total: usize =
                        self.versions.values().map(definition_bytes).sum();
                    if total + definition_bytes(definition)
                        > MAX_REGISTERED_BYTES
                    {
                        return Err("module library exceeds 1 MiB".into());
                    }
                    self.versions
                        .insert(definition.version.clone(), definition.clone());
                }
                self.selected.insert(
                    definition.name.clone(),
                    definition.version.clone(),
                );
            }
            Record::Select { name, version } => {
                if self
                    .versions
                    .get(version)
                    .is_none_or(|definition| definition.name() != name)
                {
                    return Err(
                        "module selection names an unknown version".into()
                    );
                }
                self.selected.insert(name.clone(), version.clone());
            }
            Record::Test { test } => {
                test.verify()?;
                if self
                    .versions
                    .get(test.version())
                    .is_none_or(|definition| definition.name() != test.name())
                {
                    return Err("module test names an unknown version".into());
                }
                let count: usize = self.tests.values().map(Vec::len).sum();
                if count >= MAX_TEST_RECORDS {
                    return Err("module library has 128 tests".into());
                }
                let total: usize = self
                    .tests
                    .values()
                    .flatten()
                    .map(|test| {
                        serde_json::to_vec(test).expect("test is JSON").len()
                    })
                    .sum();
                let size = serde_json::to_vec(test)
                    .map_err(|error| error.to_string())?
                    .len();
                if total + size > MAX_TEST_RECORD_BYTES {
                    return Err("module library tests exceed 8 MiB".into());
                }
                self.tests
                    .entry(test.version().to_owned())
                    .or_default()
                    .push(test.clone());
            }
        }
        Ok(())
    }
}

fn definition_bytes(definition: &Definition) -> usize {
    serde_json::to_vec(definition)
        .expect("definition is JSON")
        .len()
}

/// Fold plugin JSON records oldest first; invalid records leave the library unchanged.
pub fn fold(records: &[Value]) -> Snapshot {
    let mut library = Library::default();
    for value in records {
        if value.get("kind").and_then(Value::as_str) != Some("module") {
            continue;
        }
        if let Ok(crate::store::Record::Module(record)) =
            serde_json::from_value(value.clone())
        {
            let _ = library.apply(&record);
        }
    }
    library
}
