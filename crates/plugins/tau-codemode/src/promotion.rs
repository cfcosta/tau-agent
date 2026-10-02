//! Exact, bounded repository promotion requests and their durable decisions.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::modules::{Definition, Library, ModuleTest, RepositoryPin};

pub const MAX_REQUESTS: usize = 128;
pub const MAX_CLOSURE: usize = 256;
pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_FOLDED_BYTES: usize = MAX_TOTAL_BYTES + MAX_REQUESTS * 512;
pub const MAX_RECEIPTS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub owner: String,
    pub repository_scope: String,
    pub repository_key: String,
    pub root: Definition,
    pub versions: BTreeMap<String, Definition>,
    pub tests: Vec<ModuleTest>,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub request_id: String,
    pub owner: String,
    pub digest: String,
    pub decision: Decision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Record {
    Requested(Box<Request>),
    Decided(Terminal),
}

pub fn repository_key(path: &std::path::Path) -> String {
    hex_digest(path.as_os_str().as_encoded_bytes())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn content_digest(request: &Request) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        &request.id,
        &request.owner,
        &request.repository_scope,
        &request.repository_key,
        &request.root,
        &request.versions,
        &request.tests,
    ))
    .map_err(|error| error.to_string())?;
    Ok(hex_digest(&bytes))
}

impl Request {
    pub fn capture(
        owner: &str,
        scope: &str,
        key: &str,
        root: Definition,
        scratch: &Library,
        pin: &RepositoryPin,
    ) -> Result<Self, String> {
        if pin.owner != owner {
            return Err("repository pin belongs to another run".into());
        }
        pin.verify()?;
        let mut versions = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        capture_closure(&root, scratch, pin, &mut versions, &mut visiting)?;
        let mut request = Self {
            id: uuid::Uuid::now_v7().to_string(),
            owner: owner.to_owned(),
            repository_scope: scope.to_owned(),
            repository_key: key.to_owned(),
            tests: scratch.tests(root.version()).to_vec(),
            root,
            versions,
            digest: String::new(),
        };
        request.digest = content_digest(&request)?;
        request.verify(scratch, pin, owner, scope, key)?;
        Ok(request)
    }

    /// Rebuild the closure from persisted definitions; request JSON cannot
    /// substitute source, dependencies, or test evidence at approval time.
    pub fn verify(
        &self,
        scratch: &Library,
        pin: &RepositoryPin,
        owner: &str,
        scope: &str,
        key: &str,
    ) -> Result<(), String> {
        if uuid::Uuid::parse_str(&self.id).is_err()
            || self.owner != owner
            || self.repository_scope != scope
            || self.repository_key != key
            || pin.owner != owner
            || scope.is_empty()
            || key.len() != 64
        {
            return Err(
                "promotion request has a wrong ID, owner, or repository".into(),
            );
        }
        self.root.verify()?;
        let saved_root = scratch
            .versions()
            .get(self.root.version())
            .ok_or("promotion root is missing from persisted scratch")?;
        if saved_root != &self.root {
            return Err("promotion root differs from persisted scratch".into());
        }
        let mut expected = BTreeMap::new();
        capture_closure(
            &self.root,
            scratch,
            pin,
            &mut expected,
            &mut BTreeSet::new(),
        )?;
        if expected != self.versions {
            return Err("promotion dependency closure differs from persisted definitions".into());
        }
        if self.tests.as_slice() != scratch.tests(self.root.version()) {
            return Err(
                "promotion tests differ from exact persisted root evidence"
                    .into(),
            );
        }
        if self.digest != content_digest(self)? {
            return Err(
                "promotion request digest differs from saved content".into()
            );
        }
        if serde_json::to_vec(self)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_REQUEST_BYTES
        {
            return Err("promotion request exceeds 16 MiB".into());
        }
        Ok(())
    }
}

fn capture_closure(
    definition: &Definition,
    scratch: &Library,
    pin: &RepositoryPin,
    captured: &mut BTreeMap<String, Definition>,
    visiting: &mut BTreeSet<String>,
) -> Result<(), String> {
    definition.verify()?;
    if visiting.contains(definition.version()) {
        return Err("promotion dependency cycle".into());
    }
    if captured.contains_key(definition.version()) {
        return Ok(());
    }
    if captured.len() + visiting.len() >= MAX_CLOSURE {
        return Err("promotion has too many dependency versions".into());
    }
    visiting.insert(definition.version().to_owned());
    for (name, version) in definition.dependencies() {
        let dependency = scratch
            .versions()
            .get(version)
            .filter(|item| item.name() == name)
            .or_else(|| pin.versions.get(version).filter(|item| item.name() == name))
            .ok_or_else(|| format!("promotion dependency {name}@{version} is missing or mismatched"))?;
        capture_closure(dependency, scratch, pin, captured, visiting)?;
    }
    visiting.remove(definition.version());
    captured.insert(definition.version().to_owned(), definition.clone());
    Ok(())
}

/// Parse all matching records strictly. A duplicate request ID is invalid
/// even when its body is byte-for-byte identical.
pub fn find_request(
    records: &[serde_json::Value],
    id: &str,
) -> Result<(Request, Option<Decision>), String> {
    let mut request = None;
    let mut terminals = Vec::new();
    for value in records {
        if value.get("kind").and_then(serde_json::Value::as_str)
            != Some("promotion")
        {
            continue;
        }
        let matches = value
            .get("id")
            .or_else(|| value.get("request_id"))
            .and_then(serde_json::Value::as_str)
            == Some(id);
        if !matches {
            continue;
        }
        let record: crate::store::Record =
            serde_json::from_value(value.clone())
                .map_err(|_| "malformed promotion record".to_owned())?;
        match record {
            crate::store::Record::Promotion(Record::Requested(found)) => {
                if request.replace(*found).is_some() {
                    return Err("duplicate promotion request ID".into());
                }
            }
            crate::store::Record::Promotion(Record::Decided(terminal)) => {
                terminals.push(terminal)
            }
            _ => return Err("malformed promotion record".into()),
        }
    }
    let request: Request = request.ok_or("promotion request not found")?;
    if terminals.len() > 1 {
        return Err("conflicting promotion decisions".into());
    }
    let decision = terminals
        .pop()
        .map(|terminal| {
            if terminal.owner != request.owner
                || terminal.digest != request.digest
                || terminal.request_id != request.id
            {
                Err("promotion decision differs from request".to_owned())
            } else {
                Ok(terminal.decision)
            }
        })
        .transpose()?;
    Ok((request, decision))
}

pub fn request_count(records: &[serde_json::Value]) -> usize {
    records
        .iter()
        .filter(|value| {
            value.get("kind").and_then(serde_json::Value::as_str)
                == Some("promotion")
                && value.get("op").and_then(serde_json::Value::as_str)
                    == Some("requested")
        })
        .count()
}

pub fn request_bytes(records: &[serde_json::Value]) -> usize {
    records
        .iter()
        .filter(|value| {
            value.get("kind").and_then(serde_json::Value::as_str)
                == Some("promotion")
                && value.get("op").and_then(serde_json::Value::as_str)
                    == Some("requested")
        })
        .map(|value| value.to_string().len())
        .sum()
}
