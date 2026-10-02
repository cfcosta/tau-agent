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

    fn verify(&self) -> Result<(), String> {
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

fn validate_name(name: &str) -> Result<(), String> {
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

fn valid_version(version: &str) -> bool {
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
}

/// Registered definitions remain available after selection changes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    versions: BTreeMap<String, Definition>,
    selected: BTreeMap<String, String>,
}

pub type Snapshot = Library;

impl Library {
    pub fn versions(&self) -> &BTreeMap<String, Definition> {
        &self.versions
    }
    pub fn selected(&self) -> &BTreeMap<String, String> {
        &self.selected
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
